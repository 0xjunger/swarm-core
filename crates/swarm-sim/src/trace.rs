//! The canonical run trace — M0's deliverable.
//!
//! Two runs of the same configuration must produce byte-identical traces. That is
//! the whole acceptance criterion for this milestone (`DESIGN.md` §M0), so the
//! encoding rules matter as much as the contents. See `docs/spec.md` §7.
//!
//! This is also the ancestor of the replay capability in `DESIGN.md` §5.2: the
//! black-box claim is that a recorded run can be fed back and produce the same
//! decisions.

use std::fmt::Write as _;
use swarm_core::{NodeId, Payload};

/// One observable event.
///
/// Every field that appears here is either an integer or a `NodeId`. Deliberately
/// absent: floating point, pointers, wall-clock timestamps, source locations, and
/// anything whose ordering comes from a hash map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceRecord {
    Tick {
        at: u64,
    },
    /// The core emitted an effect. Records what the *node* decided, before the
    /// channel has had any say.
    Send {
        at: u64,
        from: NodeId,
        to: NodeId,
        payload: Payload,
    },
    /// The channel accepted the message and scheduled it.
    Enqueue {
        at: u64,
        due: u64,
        from: NodeId,
        to: NodeId,
        seq: u64,
    },
    Deliver {
        at: u64,
        from: NodeId,
        to: NodeId,
        payload: Payload,
    },
    DropLoss {
        at: u64,
        from: NodeId,
        to: NodeId,
    },
    DropPartition {
        at: u64,
        from: NodeId,
        to: NodeId,
    },
    DropOverflow {
        at: u64,
        to: NodeId,
        seq: u64,
    },
    Partition {
        at: u64,
        groups: String,
    },
    Final {
        node: NodeId,
        recv: u64,
        sent: u64,
    },
}

impl TraceRecord {
    /// Canonical encoding: fixed field order, zero-padded integers so lexicographic
    /// order matches numeric order.
    fn render(&self, out: &mut String) {
        // `write!` to a String cannot fail; the unwraps below are unreachable.
        match self {
            Self::Tick { at } => {
                let _ = writeln!(out, "t={at:012} TICK");
            }
            Self::Send {
                at,
                from,
                to,
                payload,
            } => {
                let _ = writeln!(
                    out,
                    "t={at:012} SEND from={:03} to={:03} {}",
                    from.0,
                    to.0,
                    render_payload(payload)
                );
            }
            Self::Enqueue {
                at,
                due,
                from,
                to,
                seq,
            } => {
                let _ = writeln!(
                    out,
                    "t={at:012} ENQUEUE due={due:012} from={:03} to={:03} eseq={seq:012}",
                    from.0, to.0
                );
            }
            Self::Deliver {
                at,
                from,
                to,
                payload,
            } => {
                let _ = writeln!(
                    out,
                    "t={at:012} DELIVER from={:03} to={:03} {}",
                    from.0,
                    to.0,
                    render_payload(payload)
                );
            }
            Self::DropLoss { at, from, to } => {
                let _ = writeln!(
                    out,
                    "t={at:012} DROP_LOSS from={:03} to={:03}",
                    from.0, to.0
                );
            }
            Self::DropPartition { at, from, to } => {
                let _ = writeln!(
                    out,
                    "t={at:012} DROP_PARTITION from={:03} to={:03}",
                    from.0, to.0
                );
            }
            Self::DropOverflow { at, to, seq } => {
                let _ = writeln!(
                    out,
                    "t={at:012} DROP_OVERFLOW to={:03} eseq={seq:012}",
                    to.0
                );
            }
            Self::Partition { at, groups } => {
                let _ = writeln!(out, "t={at:012} PARTITION {groups}");
            }
            Self::Final { node, recv, sent } => {
                let _ = writeln!(
                    out,
                    "FINAL node={:03} recv={recv:012} sent={sent:012}",
                    node.0
                );
            }
        }
    }
}

fn render_payload(p: &Payload) -> String {
    format!(
        "origin={:03} seq={:012} hops={:03}",
        p.origin.0, p.seq, p.hops
    )
}

/// An ordered sequence of records: everything that happened in one run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    records: Vec<TraceRecord>,
}

impl Trace {
    pub fn push(&mut self, r: TraceRecord) {
        self.records.push(r);
    }

    pub fn records(&self) -> &[TraceRecord] {
        &self.records
    }

    /// Number of records matching a predicate. Used by tests that need to assert
    /// something actually happened, not merely that it happened reproducibly.
    pub fn count(&self, f: impl Fn(&TraceRecord) -> bool) -> usize {
        self.records.iter().filter(|r| f(r)).count()
    }

    /// The full text. Comparing this rather than the digest means a failing
    /// assertion produces a readable diff instead of "two 32-byte arrays differ".
    pub fn render(&self) -> String {
        let mut s = String::new();
        for r in &self.records {
            r.render(&mut s);
        }
        s
    }

    /// A one-line fingerprint of the run, for comparing many runs cheaply.
    ///
    /// Note what this is a fingerprint *of*: not just the seed, but the whole
    /// model. Changing the tick-loop order or the number of RNG draws per effect
    /// changes it — which is exactly what `trace_is_sensitive` relies on.
    pub fn digest(&self) -> [u8; 32] {
        *blake3::hash(self.render().as_bytes()).as_bytes()
    }

    pub fn digest_hex(&self) -> String {
        self.digest().iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Payload {
        Payload {
            origin: NodeId(1),
            seq: 7,
            hops: 2,
        }
    }

    #[test]
    fn rendering_is_stable_and_padded() {
        let mut t = Trace::default();
        t.push(TraceRecord::Tick { at: 12 });
        t.push(TraceRecord::Deliver {
            at: 12,
            from: NodeId(1),
            to: NodeId(3),
            payload: payload(),
        });

        assert_eq!(
            t.render(),
            "t=000000000012 TICK\n\
             t=000000000012 DELIVER from=001 to=003 origin=001 seq=000000000007 hops=002\n"
        );
    }

    #[test]
    fn digest_tracks_content() {
        let mut a = Trace::default();
        a.push(TraceRecord::Tick { at: 1 });
        let mut b = Trace::default();
        b.push(TraceRecord::Tick { at: 1 });
        assert_eq!(a.digest(), b.digest());

        b.push(TraceRecord::Tick { at: 2 });
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn order_is_significant() {
        // Two traces with the same records in different orders must differ:
        // ordering is the property M0 exists to pin down.
        let (x, y) = (TraceRecord::Tick { at: 1 }, TraceRecord::Tick { at: 2 });
        let mut a = Trace::default();
        a.push(x.clone());
        a.push(y.clone());
        let mut b = Trace::default();
        b.push(y);
        b.push(x);
        assert_ne!(a.digest(), b.digest());
    }
}
