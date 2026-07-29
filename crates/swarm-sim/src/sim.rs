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

use std::collections::BTreeMap;
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
    pub beacon_period: u64,
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
            beacon_period: 10,
            partitions: Vec::new(),
        }
    }
}

impl SimConfig {
    pub fn roster(&self) -> Vec<NodeId> {
        (0..self.nodes).map(NodeId).collect()
    }
}

/// Runs a simulation to completion and returns its trace.
pub fn run(cfg: &SimConfig) -> Trace {
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

    let roster = cfg.roster();
    let mut states: BTreeMap<NodeId, State> = roster
        .iter()
        .map(|&n| (n, State::new(n, &roster, cfg.beacon_period)))
        .collect();

    let mut net = Network::new(cfg.queue_cap);
    let mut rng = SimRng::new(cfg.seed);
    let mut part = Partition::connected(&roster);
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
        for &dest in &roster {
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
                    payload: msg.payload,
                });

                let ev = Event::Recv {
                    from: msg.from,
                    payload: msg.payload,
                };
                let (next, fx) = step(&states[&dest], ev, now);
                states.insert(dest, next);
                emit(&mut trace, &mut net, &mut rng, cfg, dest, tick, &fx);
            }
        }

        // 4. TICK phase — nodes ascending by NodeId (rule R4).
        for &node in &roster {
            let (next, fx) = step(&states[&node], Event::Tick, now);
            states.insert(node, next);
            emit(&mut trace, &mut net, &mut rng, cfg, node, tick, &fx);
        }
    }

    for &n in &roster {
        let s = &states[&n];
        trace.push(TraceRecord::Final {
            node: n,
            recv: s.recv_count,
            sent: s.sent_count,
        });
    }

    trace
}

/// Hands the core's effects to the channel.
fn emit(
    trace: &mut Trace,
    net: &mut Network,
    rng: &mut SimRng,
    cfg: &SimConfig,
    from: NodeId,
    tick: u64,
    effects: &[Effect],
) {
    for e in effects {
        let Effect::Send { to, payload } = *e;
        trace.push(TraceRecord::Send {
            at: tick,
            from,
            to,
            payload,
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
