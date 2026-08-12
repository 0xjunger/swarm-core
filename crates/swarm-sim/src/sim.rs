//! The tick loop.
//!
//! `DESIGN.md` D-002 describes this as "a plain `for` loop of about 150 lines: one
//! message queue, one node list, one seeded random number generator". That is what
//! this is. It is written *before* any protocol exists, because a simulator written
//! afterwards would be a simulator wrapped around code that already assumed sockets
//! and sleeps — and that cannot be undone.
//!
//! The ordering below is fixed and total, and the simulator's own determinism
//! depends on it. It is not an implementation detail: determinism is not a
//! consequence of this code being single-threaded, it is a consequence of
//! this order being fixed and total (`SPEC.md` §10, "the channel model is not
//! part of `V`").

use ed25519_dalek::SigningKey;
use std::collections::{BTreeMap, BTreeSet};
use swarm_core::wire::{Body, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Effect, Envelope, Event, LogicalTime, NodeId, State};

use crate::net::{Msg, Network};
use crate::partition::Partition;
use crate::rng::SimRng;
use crate::trace::{Trace, TraceRecord};

/// Everything that determines a run. Same config plus same seed means same trace.
#[derive(Clone, Debug)]
pub struct SimConfig {
    pub nodes: u8,
    pub seed: u64,
    pub ticks: u64,
    /// Message loss, in parts per thousand. An integer, never a float.
    pub loss_permille: u32,
    /// Must be >= 1. See rule R1.
    pub delay_min: u64,
    pub delay_max: u64,
    pub queue_cap: usize,
    /// A node creates and broadcasts a new entry when `tick % entry_period
    /// == 0`. Replaces M0/M1's `beacon_period`.
    pub entry_period: u64,
    /// A node advertises its version vector when `tick % anti_entropy_period
    /// == 0`.
    pub anti_entropy_period: u64,
    /// Bound on each node's own hash chain (`SPEC.md` §4.2).
    pub log_cap: usize,
    /// Bound on each node's causal buffer (`DESIGN.md` D-013).
    pub buffer_cap: usize,
    /// Scripted partition changes, applied at the top of the named tick. M5's
    /// randomised churn will be a seeded *generator of this same script*; the loop
    /// below does not change.
    pub partitions: Vec<(u64, Partition)>,
    /// A deliberately faulty node, if any (`DESIGN.md` D-007): the channel
    /// still only drops and delays — "Byzantine transport" stays out of
    /// scope — the *node* forges, at the protocol layer, by having its
    /// genesis entry re-signed differently for each listed victim.
    pub equivocation: Option<Equivocation>,
    /// Per-node escrow allocation (`SPEC.md` §6.4). Defaults to 3, M5's
    /// original convention. Nodes issue one `Spend { amount: 1 }` per
    /// `entry_period` tick while budget remains.
    pub budget_per_node: u64,
}

/// A node that signs two different genesis entries — one real, one forged
/// per victim — and sends each victim the one meant for it (`DESIGN.md`
/// D-007's demo scenario).
#[derive(Clone, Debug)]
pub struct Equivocation {
    pub node: NodeId,
    pub victims: BTreeSet<NodeId>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            nodes: 5,
            seed: 0,
            ticks: 200,
            loss_permille: 0,
            delay_min: 1,
            delay_max: 3,
            queue_cap: 64,
            entry_period: 10,
            anti_entropy_period: 15,
            log_cap: 1000,
            buffer_cap: 32,
            partitions: Vec::new(),
            equivocation: None,
            budget_per_node: 3,
        }
    }
}

impl SimConfig {
    pub fn roster(&self) -> Vec<NodeId> {
        (0..self.nodes).map(NodeId).collect()
    }

    /// The per-node budget map handed to `State::with_budgets` and to
    /// `swarm-verify::check_invariants` — one source of truth so a caller
    /// checking I4 cannot reconstruct it inconsistently.
    pub fn budgets(&self) -> BTreeMap<NodeId, u64> {
        self.roster()
            .into_iter()
            .map(|n| (n, self.budget_per_node))
            .collect()
    }
}

/// A deterministic per-node signing key, derived from `NodeId` alone — never
/// drawn from `SimRng`, so it cannot perturb rule R2's draw-count contract.
/// Same convention as `swarm-core`'s own test keys.
fn node_key(node: NodeId) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = node.0.wrapping_add(1);
    SigningKey::from_bytes(&bytes)
}

/// The shared mission roster: every node's identity and verifying key,
/// fixed for the whole run (`DESIGN.md` D-005). Public so a test or example
/// can build the same roster a third party would hold to verify a proof of
/// equivocation without running the simulator itself (`DESIGN.md` D-007) —
/// nothing here is simulator-internal, it is just public keys.
pub fn build_roster(nodes: &[NodeId]) -> Roster {
    let keys = nodes
        .iter()
        .map(|&n| (n, node_key(n).verifying_key()))
        .collect();
    Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys)
}

/// A different, validly signed entry at the same `(node, seq, prev, deps)` as
/// `original` — the forged half of an equivocation (`DESIGN.md` D-007).
/// Re-signed with the same node's own key: the simulator forges *as* the
/// faulty node, not as the channel, keeping the "Byzantine transport"
/// boundary honest.
fn forge_alt_entry(original: &swarm_core::wire::Entry, node: NodeId) -> swarm_core::wire::Entry {
    let alt_body = match original.body {
        Body::TaskClaim { task, priority } => Body::TaskClaim {
            task: task.wrapping_add(1_000_000),
            priority: priority.wrapping_add(1).max(1),
        },
        Body::Withdraw { task } => Body::Withdraw {
            task: task.wrapping_add(1_000_000),
        },
        Body::Spend { amount } => Body::Spend {
            amount: amount.wrapping_add(1_000_000),
        },
    };
    UnsignedEntry {
        mission_id: original.mission_id,
        epoch: original.epoch,
        node: original.node,
        seq: original.seq,
        prev: original.prev,
        deps: original.deps.clone(),
        body: alt_body,
    }
    .sign(&node_key(node))
}

/// If `cfg` names `from` as the equivocator and `to` as one of its victims,
/// swaps the genesis entry for a differently signed copy at the same
/// `(node, seq)` (`DESIGN.md` D-007). Only the genesis entry is forged:
/// once a victim holds the forged copy, the equivocator's later entries fail
/// `BadPrevLink` at that victim by the ordinary chain-verification rule
/// (`SPEC.md` §4.2) — no further forging is needed to keep the two
/// sides apart. Every other `(from, to, payload)` triple passes through
/// unchanged.
fn maybe_forge(cfg: &SimConfig, from: NodeId, to: NodeId, payload: Envelope) -> Envelope {
    let Some(eq) = &cfg.equivocation else {
        return payload;
    };
    if from != eq.node || !eq.victims.contains(&to) {
        return payload;
    }
    match payload {
        Envelope::Entry(entry) if entry.seq == 0 => Envelope::Entry(forge_alt_entry(&entry, from)),
        other => other,
    }
}

/// Runs a simulation to completion and returns its trace.
pub fn run(cfg: &SimConfig) -> Trace {
    run_with_states(cfg).0
}

/// Runs a simulation to completion and returns its trace *and* every node's
/// final `State` — the M2 acceptance test and the `converge` example need to
/// inspect each node's entries and version vector directly, not just the
/// trace.
pub fn run_with_states(cfg: &SimConfig) -> (Trace, BTreeMap<NodeId, State>) {
    // R1. Without this an effect produced during tick N could be delivered during
    // tick N, and the resulting order would depend on iteration sequence rather
    // than on any stated rule.
    assert!(cfg.delay_min >= 1, "delay_min must be >= 1 (R1)");
    assert!(
        cfg.delay_min <= cfg.delay_max,
        "delay_min must not exceed delay_max"
    );
    assert!(
        cfg.loss_permille <= 1000,
        "loss_permille is parts per thousand"
    );
    assert!(cfg.nodes >= 1, "need at least one node");

    let roster_ids = cfg.roster();
    let roster = build_roster(&roster_ids);
    let budgets: BTreeMap<NodeId, u64> = cfg.budgets();
    let mut states: BTreeMap<NodeId, State> = roster_ids
        .iter()
        .map(|&n| {
            (
                n,
                State::new(
                    n,
                    roster.clone(),
                    node_key(n),
                    cfg.log_cap,
                    cfg.buffer_cap,
                    cfg.entry_period,
                    cfg.anti_entropy_period,
                )
                .with_budgets(budgets.clone()),
            )
        })
        .collect();

    let mut net = Network::new(cfg.queue_cap);
    let mut rng = SimRng::new(cfg.seed);
    let mut part = Partition::connected(&roster_ids);
    let mut trace = Trace::default();

    for tick in 1..=cfg.ticks {
        let now = LogicalTime(tick);
        trace.push(TraceRecord::Tick { at: tick });

        // 2. Apply any scheduled partition change.
        for (at, p) in &cfg.partitions {
            if *at == tick {
                part = p.clone();
                trace.push(TraceRecord::Partition {
                    at: tick,
                    groups: part.render(),
                });
            }
        }

        // 3. DELIVER phase — destinations ascending by NodeId (rule R4).
        for &dest in &roster_ids {
            for msg in net.take_due(dest, tick) {
                // Reachability is checked here, at delivery, not at send. A message
                // already in the air when the link drops does not arrive.
                if !part.reachable(msg.from, dest) {
                    trace.push(TraceRecord::DropPartition {
                        at: tick,
                        from: msg.from,
                        to: dest,
                    });
                    continue;
                }
                trace.push(TraceRecord::Deliver {
                    at: tick,
                    from: msg.from,
                    to: dest,
                    payload: msg.payload.clone(),
                });

                let ev = Event::Recv {
                    from: msg.from,
                    payload: msg.payload,
                };
                let (next, fx) = step(&states[&dest], ev, now);
                trace_state_diff(&mut trace, dest, tick, &states[&dest], &next);
                states.insert(dest, next);
                // Never authoring: only relaying (anti-entropy push replies,
                // §9.5) — see `emit`'s `authoring` parameter.
                emit(
                    &mut Runtime {
                        trace: &mut trace,
                        net: &mut net,
                        rng: &mut rng,
                    },
                    cfg,
                    dest,
                    tick,
                    fx,
                    false,
                );
            }
        }

        // 4. TICK phase — nodes ascending by NodeId (rule R4).
        for &node in &roster_ids {
            let (next, fx) = step(&states[&node], Event::Tick, now);
            trace_state_diff(&mut trace, node, tick, &states[&node], &next);
            states.insert(node, next);
            // The only phase in which a node can author a brand-new entry
            // (`author` is only called from `Event::Tick`) — so it is the
            // only phase in which equivocation's one-time forged genesis
            // broadcast can legitimately happen.
            emit(
                &mut Runtime {
                    trace: &mut trace,
                    net: &mut net,
                    rng: &mut rng,
                },
                cfg,
                node,
                tick,
                fx,
                true,
            );
        }
    }

    for &n in &roster_ids {
        let s = &states[&n];
        trace.push(TraceRecord::Final {
            node: n,
            recv: s.recv_count,
            sent: s.sent_count,
        });
    }

    (trace, states)
}

/// Derives `Apply`/`Buffer`/`DropCausalOverflow` trace records by diffing a
/// node's `State` before and after one `step` call. `step` itself stays pure
/// and returns only `Effect`s — this is bookkeeping the simulator does on
/// the side, not a change to the core's contract (`DESIGN.md` D-002).
fn trace_state_diff(trace: &mut Trace, node: NodeId, tick: u64, old: &State, new: &State) {
    // Every entry newly reflected in `causal_vv`, ascending by origin (R4)
    // then by seq — covers direct application and buffer-drain alike, since
    // both advance `causal_vv` the same way.
    let mut applied: BTreeSet<(NodeId, u64)> = BTreeSet::new();
    for (origin, new_highest) in new.causal_vv().iter() {
        let start = old.causal_vv().highest(origin).map_or(0, |v| v + 1);
        for seq in start..=new_highest {
            trace.push(TraceRecord::Apply {
                at: tick,
                node,
                origin,
                seq,
            });
            applied.insert((origin, seq));
        }
    }

    let old_buf: BTreeSet<(NodeId, u64)> = old.buffer_keys().collect();
    let new_buf: BTreeSet<(NodeId, u64)> = new.buffer_keys().collect();

    for &(origin, seq) in new_buf.difference(&old_buf) {
        trace.push(TraceRecord::Buffer {
            at: tick,
            node,
            origin,
            seq,
        });
    }
    // A key that left the buffer without being applied this tick was
    // evicted for space, not delivered (`DESIGN.md` D-013).
    for &(origin, seq) in old_buf.difference(&new_buf) {
        if !applied.contains(&(origin, seq)) {
            trace.push(TraceRecord::DropCausalOverflow {
                at: tick,
                node,
                origin,
                seq,
            });
        }
    }

    // A newly verified proof of equivocation (`DESIGN.md` D-007). At
    // most one proof is kept per accused node (`swarm-core`'s own bound), so
    // the diff is just "which accused nodes are new since last step".
    let old_faulty: BTreeSet<NodeId> = old.poes().map(|p| p.node()).collect();
    for poe in new.poes() {
        if !old_faulty.contains(&poe.node()) {
            trace.push(TraceRecord::Equivocation {
                at: tick,
                witness: node,
                accused: poe.node(),
                seq: poe.seq(),
            });
        }
    }
}

/// Hands the core's effects to the channel.
///
/// `authoring` is `true` only for effects produced by `Event::Tick` — the
/// one path that can call `author` and therefore the only point at which a
/// faulty node's one-time forged genesis broadcast may legitimately be
/// substituted (`DESIGN.md` D-007). Effects produced by
/// `Event::Recv` are always a relay of something already stored — an
/// anti-entropy push reply, possibly of the equivocator's own honestly-held
/// entry — and must pass through unforged, or a victim could never receive
/// the genuine article from anyone, including the equivocator's own later,
/// honest replies about its own log.
/// The three pieces of per-run mutable state `emit` needs, bundled so the
/// function takes one parameter for them instead of three.
struct Runtime<'a> {
    trace: &'a mut Trace,
    net: &'a mut Network,
    rng: &'a mut SimRng,
}

fn emit(
    rt: &mut Runtime,
    cfg: &SimConfig,
    from: NodeId,
    tick: u64,
    effects: Vec<Effect>,
    authoring: bool,
) {
    for e in effects {
        let Effect::Send { to, payload } = e;
        // A faulty node signs a different genesis entry per victim
        // (`DESIGN.md` D-007); an honest run's `cfg.equivocation` is
        // `None` and this is a no-op either way.
        let payload = if authoring {
            maybe_forge(cfg, from, to, payload)
        } else {
            payload
        };
        rt.trace.push(TraceRecord::Send {
            at: tick,
            from,
            to,
            payload: payload.clone(),
        });

        // R2: both values are drawn for every effect, in this order, even when the
        // message is about to be dropped. Drawing only for survivors would make the
        // random stream a function of loss and queue occupancy, so an unrelated
        // change to the partition schedule would scramble every subsequent draw and
        // make two traces incomparable.
        let r_loss = rt.rng.next_u32();
        let r_delay = rt.rng.next_u32();

        if r_loss % 1000 < cfg.loss_permille {
            rt.trace.push(TraceRecord::DropLoss { at: tick, from, to });
            continue;
        }

        // R3: integer arithmetic throughout.
        let span = cfg.delay_max - cfg.delay_min + 1;
        let due = tick + cfg.delay_min + u64::from(r_delay) % span;

        let res = rt.net.enqueue(due, Msg { from, to, payload });
        if let Some(evicted) = res.evicted {
            rt.trace.push(TraceRecord::DropOverflow {
                at: tick,
                to,
                seq: evicted,
            });
        }
        rt.trace.push(TraceRecord::Enqueue {
            at: tick,
            due,
            from,
            to,
            seq: res.seq,
        });
    }
}
