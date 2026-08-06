//! The tick loop.
//!
//! `DESIGN.md` §M0 describes this as "a plain `for` loop of about 150 lines: one
//! message queue, one node list, one seeded random number generator". That is what
//! this is. It is written *before* any protocol exists, because a simulator written
//! afterwards would be a simulator wrapped around code that already assumed sockets
//! and sleeps — and that cannot be undone.
//!
//! The ordering below is normative and is specified in `docs/spec.md` §6. It is not
//! an implementation detail: determinism is not a consequence of this code being
//! single-threaded, it is a consequence of this order being fixed and total.

use ed25519_dalek::SigningKey;
use std::collections::{BTreeMap, BTreeSet};
use swarm_core::wire::{Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Effect, Event, LogicalTime, NodeId, State};

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
    /// Message loss, in parts per thousand. An integer, never a float — see
    /// `docs/spec.md` §5.3.
    pub loss_permille: u32,
    /// Must be >= 1. See rule R1.
    pub delay_min: u64,
    pub delay_max: u64,
    pub queue_cap: usize,
    /// A node creates and broadcasts a new entry when `tick % entry_period
    /// == 0` (`docs/spec-m2.md` §6). Replaces M0/M1's `beacon_period`.
    pub entry_period: u64,
    /// A node advertises its version vector when `tick % anti_entropy_period
    /// == 0` (`docs/spec-m2.md` §6).
    pub anti_entropy_period: u64,
    /// Bound on each node's own hash chain (`docs/spec-m1.md` §6).
    pub log_cap: usize,
    /// Bound on each node's causal buffer (`docs/spec-m2.md` §5).
    pub buffer_cap: usize,
    /// Scripted partition changes, applied at the top of the named tick. M5's
    /// randomised churn will be a seeded *generator of this same script*; the loop
    /// below does not change.
    pub partitions: Vec<(u64, Partition)>,
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
        }
    }
}

impl SimConfig {
    pub fn roster(&self) -> Vec<NodeId> {
        (0..self.nodes).map(NodeId).collect()
    }
}

/// A deterministic per-node signing key, derived from `NodeId` alone — never
/// drawn from `SimRng`, so it cannot perturb rule R2's draw-count contract
/// (`docs/spec.md` §6). Same convention as `swarm-core`'s own test keys.
fn node_key(node: NodeId) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = node.0.wrapping_add(1);
    SigningKey::from_bytes(&bytes)
}

/// The shared mission roster: every node's identity and verifying key,
/// fixed for the whole run (`DESIGN.md` §7).
fn build_roster(nodes: &[NodeId]) -> Roster {
    let keys = nodes
        .iter()
        .map(|&n| (n, node_key(n).verifying_key()))
        .collect();
    Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys)
}

/// Runs a simulation to completion and returns its trace.
pub fn run(cfg: &SimConfig) -> Trace {
    run_with_states(cfg).0
}

/// Runs a simulation to completion and returns its trace *and* every node's
/// final `State` — the M2 acceptance test and the `converge` example need to
/// inspect each node's entries and version vector directly, not just the
/// trace (`docs/spec-m2.md` §7).
pub fn run_with_states(cfg: &SimConfig) -> (Trace, BTreeMap<NodeId, State>) {
    // R1. Without this an effect produced during tick N could be delivered during
    // tick N, and the resulting order would depend on iteration sequence rather
    // than on any stated rule.
    assert!(
        cfg.delay_min >= 1,
        "delay_min must be >= 1 (docs/spec.md §6 R1)"
    );
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
                ),
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
                emit(&mut trace, &mut net, &mut rng, cfg, dest, tick, fx);
            }
        }

        // 4. TICK phase — nodes ascending by NodeId (rule R4).
        for &node in &roster_ids {
            let (next, fx) = step(&states[&node], Event::Tick, now);
            trace_state_diff(&mut trace, node, tick, &states[&node], &next);
            states.insert(node, next);
            emit(&mut trace, &mut net, &mut rng, cfg, node, tick, fx);
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
/// node's `State` before and after one `step` call (`docs/spec-m2.md` §7).
/// `step` itself stays pure and returns only `Effect`s — this is bookkeeping
/// the simulator does on the side, not a change to the core's contract
/// (`docs/spec.md` §3.2).
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
    // evicted for space, not delivered (`docs/spec-m2.md` §5).
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
}

/// Hands the core's effects to the channel.
fn emit(
    trace: &mut Trace,
    net: &mut Network,
    rng: &mut SimRng,
    cfg: &SimConfig,
    from: NodeId,
    tick: u64,
    effects: Vec<Effect>,
) {
    for e in effects {
        let Effect::Send { to, payload } = e;
        trace.push(TraceRecord::Send {
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
        let r_loss = rng.next_u32();
        let r_delay = rng.next_u32();

        if r_loss % 1000 < cfg.loss_permille {
            trace.push(TraceRecord::DropLoss { at: tick, from, to });
            continue;
        }

        // R3: integer arithmetic throughout.
        let span = cfg.delay_max - cfg.delay_min + 1;
        let due = tick + cfg.delay_min + u64::from(r_delay) % span;

        let res = net.enqueue(due, Msg { from, to, payload });
        if let Some(evicted) = res.evicted {
            trace.push(TraceRecord::DropOverflow {
                at: tick,
                to,
                seq: evicted,
            });
        }
        trace.push(TraceRecord::Enqueue {
            at: tick,
            due,
            from,
            to,
            seq: res.seq,
        });
    }
}
