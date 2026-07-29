//! The coordination state machine: pure, deterministic, and free of all I/O.
//!
//! Everything here is a function of its arguments. There is no network, no clock,
//! and no randomness inside this crate — time enters as the `now` parameter and
//! nothing else enters at all. `DESIGN.md` §11.1 states this rule as
//! non-negotiable, and `docs/spec.md` §3 records how it is enforced.
//!
//! `#![no_std]` is not decorative. It makes `std::collections::HashMap`
//! unreachable, and `HashMap`'s hasher is seeded per process, so its iteration
//! order differs between two runs of the same binary. That is exactly the class of
//! bug M0's byte-identical-trace criterion exists to catch. See `docs/spec.md` §3.1.
//!
//! # M0 scope
//!
//! There is no protocol here yet. Node behaviour is a placeholder — count what
//! arrives, echo it back — because M0 tests the *channel*, not the protocol. The
//! `Payload` type becomes `Entry` at M1.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(test)]
extern crate std;

use alloc::vec::Vec;

/// How many times a message may be echoed before it dies.
///
/// Without a limit the echo would bounce forever and the number of messages in
/// flight would grow without bound. `DESIGN.md` §7 requires every structure in this
/// system to have a stated bound; the habit starts here.
pub const MAX_HOPS: u8 = 4;

/// A member of the roster.
///
/// The roster (the swarm's member list) is fixed at mission start for the whole of
/// Phase 1 — `DESIGN.md` §7 notes that dynamic membership is where 90% of the
/// complexity comes from. `u8` is sufficient: §4.5 caps certificate rosters at
/// N <= 20.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct NodeId(pub u8);

/// The only notion of time in this system.
///
/// There is no wall clock at any layer. `DESIGN.md` §7 forbids tie-breaking on
/// wall-clock time because GPS time can be spoofed, which would hand claim races to
/// an attacker. M0 has nothing to tie-break yet; the point is that a wall-clock
/// dependency cannot be introduced later by accident, because there is no clock to
/// reach for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct LogicalTime(pub u64);

/// The M0 placeholder message. Becomes `Entry` (`DESIGN.md` §3) at M1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Payload {
    /// Node that first emitted this token.
    pub origin: NodeId,
    /// Per-origin counter, so a token is identifiable in the trace.
    pub seq: u64,
    /// Echo count so far; the message dies at `MAX_HOPS`.
    pub hops: u8,
}

/// Something that happened *to* a node. The only input to [`step`].
///
/// Per `DESIGN.md` §11.4, no variant is added before a test exercises it. Both of
/// these are exercised below. `AntiEntropy` arrives at M2.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// The clock advanced. Carries no data — `now` is a separate parameter.
    Tick,
    /// A message arrived from `from`.
    Recv { from: NodeId, payload: Payload },
}

/// Something a node wants the outside world to do. The only output of [`step`].
///
/// The core never performs an effect; it describes one and hands it back. This is
/// what keeps the crate sans-I/O and what makes replay possible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    Send { to: NodeId, payload: Payload },
}

/// Everything a node knows.
///
/// Grows into the per-node log, version vector and CRDTs over M1–M5. It must stay
/// `Clone`, because [`step`] is pure — see the note on the signature below.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct State {
    /// This node's own identity.
    pub me: NodeId,
    /// The mission roster, held sorted ascending. Iteration order is part of the
    /// determinism contract (`docs/spec.md` §6, rule R4), so this invariant matters.
    roster: Vec<NodeId>,
    /// Emit a beacon when `now % beacon_period == 0`. Zero disables beacons.
    beacon_period: u64,
    /// Messages received. M0's only observable behaviour.
    pub recv_count: u64,
    /// Effects emitted.
    pub sent_count: u64,
    /// Next value for `Payload::seq`.
    next_seq: u64,
}

impl State {
    /// Creates a node's initial state.
    ///
    /// `roster` is sorted here rather than trusted, because ascending-by-`NodeId`
    /// iteration is a determinism rule and a caller-supplied order would silently
    /// break it.
    ///
    /// # Panics
    ///
    /// If `me` is not in `roster`. A node absent from its own mission roster is a
    /// configuration error, and it is one that would otherwise stay invisible:
    /// the node would run, send and receive normally, while every peer treated it
    /// as a non-member.
    pub fn new(me: NodeId, roster: &[NodeId], beacon_period: u64) -> Self {
        assert!(
            roster.contains(&me),
            "node must be a member of its own roster"
        );
        let mut roster: Vec<NodeId> = roster.to_vec();
        roster.sort_unstable();
        roster.dedup();
        Self {
            me,
            roster,
            beacon_period,
            recv_count: 0,
            sent_count: 0,
            next_seq: 0,
        }
    }

    /// The mission roster, ascending by `NodeId`.
    pub fn roster(&self) -> &[NodeId] {
        &self.roster
    }
}

/// The one function. Verbatim from `DESIGN.md` §5.
///
/// Takes `&State` and returns a new `State` rather than mutating in place. That
/// costs a clone per event, and the cost is accepted on purpose: this is the shape
/// a folding scheme's step function has (`z_{i+1} = F(z_i, w_i)`), and Phase 4's
/// claim that `swarm-verify`'s replay becomes a proof without rewriting the circuit
/// depends on the signature already being this. `docs/spec.md` §3.2 records the cost
/// and the conditions for revisiting it.
///
/// Determinism: the returned effects are in a fixed order, and the function reads
/// nothing outside its arguments.
pub fn step(state: &State, ev: Event, now: LogicalTime) -> (State, Vec<Effect>) {
    let mut next = state.clone();
    let mut effects = Vec::new();

    match ev {
        Event::Recv { from, payload } => {
            next.recv_count += 1;
            // Echo it back, one hop older, until the token expires.
            if payload.hops < MAX_HOPS {
                effects.push(Effect::Send {
                    to: from,
                    payload: Payload {
                        hops: payload.hops + 1,
                        ..payload
                    },
                });
            }
        }
        Event::Tick => {
            // A silent network is trivially deterministic and would satisfy M0's
            // acceptance criterion while proving nothing. Beacons keep the channel
            // busy so that loss, delay and partition are actually exercised.
            if next.beacon_period != 0 && now.0.is_multiple_of(next.beacon_period) {
                for &peer in next.roster.iter() {
                    if peer == next.me {
                        continue;
                    }
                    effects.push(Effect::Send {
                        to: peer,
                        payload: Payload {
                            origin: next.me,
                            seq: next.next_seq,
                            hops: 0,
                        },
                    });
                    next.next_seq += 1;
                }
            }
        }
    }

    next.sent_count += effects.len() as u64;
    (next, effects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster3() -> [NodeId; 3] {
        [NodeId(0), NodeId(1), NodeId(2)]
    }

    fn token(origin: u8, hops: u8) -> Payload {
        Payload {
            origin: NodeId(origin),
            seq: 0,
            hops,
        }
    }

    #[test]
    fn roster_is_sorted_and_deduped_regardless_of_input_order() {
        let s = State::new(NodeId(1), &[NodeId(2), NodeId(0), NodeId(1), NodeId(2)], 10);
        assert_eq!(s.roster(), &[NodeId(0), NodeId(1), NodeId(2)]);
    }

    #[test]
    #[should_panic(expected = "own roster")]
    fn node_missing_from_its_own_roster_is_rejected() {
        State::new(NodeId(1), &[NodeId(0), NodeId(2)], 10);
    }

    #[test]
    fn recv_counts_and_echoes_back_to_sender() {
        let s = State::new(NodeId(1), &roster3(), 10);
        let (s2, fx) = step(
            &s,
            Event::Recv {
                from: NodeId(0),
                payload: token(0, 0),
            },
            LogicalTime(1),
        );

        assert_eq!(s2.recv_count, 1);
        assert_eq!(
            fx,
            [Effect::Send {
                to: NodeId(0),
                payload: token(0, 1)
            }]
        );
    }

    #[test]
    fn echo_dies_at_max_hops() {
        let s = State::new(NodeId(1), &roster3(), 10);
        let (s2, fx) = step(
            &s,
            Event::Recv {
                from: NodeId(0),
                payload: token(0, MAX_HOPS),
            },
            LogicalTime(1),
        );

        // Counted, but not echoed: this is what bounds messages in flight.
        assert_eq!(s2.recv_count, 1);
        assert!(fx.is_empty());
    }

    #[test]
    fn beacon_fires_on_period_to_every_peer_ascending() {
        let s = State::new(NodeId(1), &roster3(), 10);
        let (s2, fx) = step(&s, Event::Tick, LogicalTime(10));

        let dests: Vec<NodeId> = fx.iter().map(|Effect::Send { to, .. }| *to).collect();
        // Never to itself, and in ascending NodeId order (contract rule R4).
        assert_eq!(dests, [NodeId(0), NodeId(2)]);
        assert_eq!(s2.sent_count, 2);
    }

    #[test]
    fn beacon_is_silent_off_period() {
        let s = State::new(NodeId(1), &roster3(), 10);
        let (_, fx) = step(&s, Event::Tick, LogicalTime(11));
        assert!(fx.is_empty());
    }

    #[test]
    fn step_is_pure_and_reproducible() {
        let s = State::new(NodeId(1), &roster3(), 10);
        let (a, fx_a) = step(&s, Event::Tick, LogicalTime(10));
        let (b, fx_b) = step(&s, Event::Tick, LogicalTime(10));

        // Same input, same output — and the input is untouched.
        assert_eq!(a, b);
        assert_eq!(fx_a, fx_b);
        assert_eq!(s.sent_count, 0);
    }
}
