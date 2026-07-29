//! The channel: delay, loss, and a bounded queue per destination.
//!
//! See `docs/spec.md` §5. Partition handling deliberately lives elsewhere — the
//! queue does not know about reachability, because reachability is evaluated at
//! delivery time, not at enqueue time (§5.4).

use std::collections::BTreeMap;
use swarm_core::{NodeId, Payload};

/// A message in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Msg {
    pub from: NodeId,
    pub to: NodeId,
    pub payload: Payload,
}

/// Result of accepting a message into the channel.
pub struct Enqueued {
    /// The global enqueue sequence assigned to this message.
    pub seq: u64,
    /// The sequence number evicted to make room, if the queue was full.
    pub evicted: Option<u64>,
}

/// In-flight messages, one bounded queue per destination.
pub struct Network {
    /// Keyed `(due_tick, enqueue_seq)`. Because `enqueue_seq` is globally unique,
    /// no two messages ever compare equal, so this order is *total* and needs no
    /// tie-break rule. That matters: `DESIGN.md` §7 warns that tie-breaks reached
    /// for casually are where wall-clock dependencies sneak in.
    queues: BTreeMap<NodeId, BTreeMap<(u64, u64), Msg>>,
    cap: usize,
    next_seq: u64,
}

impl Network {
    pub fn new(cap: usize) -> Self {
        assert!(cap >= 1, "queue_cap must be at least 1");
        Self {
            queues: BTreeMap::new(),
            cap,
            next_seq: 0,
        }
    }

    /// Schedules a message for delivery at `due`.
    ///
    /// If the destination queue is full, the message that would have been
    /// delivered next is dropped to make room — "drop the oldest", per
    /// `DESIGN.md` §4.1 and §7. Every queue in this system is bounded; the
    /// discipline starts here so it is already habitual when the causal buffer
    /// arrives at M2.
    pub fn enqueue(&mut self, due: u64, msg: Msg) -> Enqueued {
        let seq = self.next_seq;
        self.next_seq += 1;

        let cap = self.cap;
        let q = self.queues.entry(msg.to).or_default();

        let evicted = if q.len() >= cap {
            q.pop_first().map(|((_, ev_seq), _)| ev_seq)
        } else {
            None
        };

        q.insert((due, seq), msg);
        Enqueued { seq, evicted }
    }

    /// Removes and returns every message for `dest` that is due at or before
    /// `now`, in `(due, enqueue_seq)` order.
    pub fn take_due(&mut self, dest: NodeId, now: u64) -> Vec<Msg> {
        let Some(q) = self.queues.get_mut(&dest) else {
            return Vec::new();
        };
        // `split_off` keeps keys < (now+1, 0) in `q` — exactly those with due <= now.
        let rest = q.split_off(&(now + 1, 0));
        let due = std::mem::replace(q, rest);
        due.into_values().collect()
    }

    /// Current queue depth for a destination. Used by the overflow test.
    pub fn depth(&self, dest: NodeId) -> usize {
        self.queues.get(&dest).map_or(0, |q| q.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: NodeId = NodeId(0);
    const B: NodeId = NodeId(1);

    fn msg(seq: u64) -> Msg {
        Msg {
            from: A,
            to: B,
            payload: Payload {
                origin: A,
                seq,
                hops: 0,
            },
        }
    }

    #[test]
    fn nothing_is_due_before_its_time() {
        let mut n = Network::new(8);
        n.enqueue(5, msg(0));
        assert!(n.take_due(B, 4).is_empty());
        assert_eq!(n.take_due(B, 5).len(), 1);
    }

    #[test]
    fn due_messages_come_out_in_due_then_seq_order() {
        let mut n = Network::new(8);
        // Enqueued out of order, and two share a due tick.
        n.enqueue(7, msg(100));
        n.enqueue(5, msg(200));
        n.enqueue(7, msg(300));

        let got: Vec<u64> = n.take_due(B, 9).iter().map(|m| m.payload.seq).collect();
        // due=5 first; then the two due=7 in enqueue order, never reversed.
        assert_eq!(got, [200, 100, 300]);
    }

    #[test]
    fn full_queue_evicts_the_oldest() {
        let mut n = Network::new(2);
        let first = n.enqueue(5, msg(1));
        n.enqueue(6, msg(2));
        assert_eq!(n.depth(B), 2);

        let third = n.enqueue(7, msg(3));
        assert_eq!(third.evicted, Some(first.seq));
        assert_eq!(n.depth(B), 2, "bound must hold after eviction");

        let survivors: Vec<u64> = n.take_due(B, 99).iter().map(|m| m.payload.seq).collect();
        assert_eq!(survivors, [2, 3]);
    }

    #[test]
    fn queues_are_independent_per_destination() {
        let mut n = Network::new(8);
        n.enqueue(5, msg(1));
        assert_eq!(n.depth(A), 0);
        assert!(n.take_due(A, 99).is_empty());
        assert_eq!(n.depth(B), 1);
    }
}
