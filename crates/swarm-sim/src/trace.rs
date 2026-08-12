//! The canonical run trace — M0's deliverable.
//!
//! Two runs of the same configuration must produce byte-identical traces. That is
//! the whole acceptance criterion for this milestone, so the encoding rules matter
//! as much as the contents.
//!
//! This is also the ancestor of the replay capability `DESIGN.md` D-002 lists among
//! the sans-I/O boundary's goals: the black-box claim is that a recorded run can be
//! fed back and produce the same decisions.

use std::fmt::Write as _;
use swarm_core::causal::VersionVector;
use swarm_core::wire::Body;
use swarm_core::{Envelope, NodeId};

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
        payload: Envelope,
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
        payload: Envelope,
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
    /// A node applied entry `(origin, seq)` to its state (`SPEC.md` §4.3).
    /// Derived by diffing `State` before/after a `step` call — `step`
    /// itself stays pure and returns only `Effect`s (`DESIGN.md` D-002).
    Apply {
        at: u64,
        node: NodeId,
        origin: NodeId,
        seq: u64,
    },
    /// A node buffered entry `(origin, seq)`: received, but its causal
    /// dependencies are not yet satisfied (`SPEC.md` §4.3).
    Buffer {
        at: u64,
        node: NodeId,
        origin: NodeId,
        seq: u64,
    },
    /// A node's causal buffer was full and `(origin, seq)` was evicted to
    /// make room (`DESIGN.md` D-013). Recoverable: the next anti-entropy
    /// round re-offers it.
    DropCausalOverflow {
        at: u64,
        node: NodeId,
        origin: NodeId,
        seq: u64,
    },
    Final {
        node: NodeId,
        recv: u64,
        sent: u64,
    },
    /// `witness` independently verified a proof that `accused` signed two
    /// different entries at `(accused, seq)` (`DESIGN.md` D-007).
    /// Derived the same way as `Apply`/`Buffer`: by diffing `State` before
    /// and after a `step` call.
    Equivocation {
        at: u64,
        witness: NodeId,
        accused: NodeId,
        seq: u64,
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
                    render_envelope(payload)
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
                    render_envelope(payload)
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
            Self::Apply {
                at,
                node,
                origin,
                seq,
            } => {
                let _ = writeln!(
                    out,
                    "t={at:012} APPLY node={:03} origin={:03} seq={seq:012}",
                    node.0, origin.0
                );
            }
            Self::Buffer {
                at,
                node,
                origin,
                seq,
            } => {
                let _ = writeln!(
                    out,
                    "t={at:012} BUFFER node={:03} origin={:03} seq={seq:012}",
                    node.0, origin.0
                );
            }
            Self::DropCausalOverflow {
                at,
                node,
                origin,
                seq,
            } => {
                let _ = writeln!(
                    out,
                    "t={at:012} DROP_CAUSAL_OVERFLOW node={:03} origin={:03} seq={seq:012}",
                    node.0, origin.0
                );
            }
            Self::Final { node, recv, sent } => {
                let _ = writeln!(
                    out,
                    "FINAL node={:03} recv={recv:012} sent={sent:012}",
                    node.0
                );
            }
            Self::Equivocation {
                at,
                witness,
                accused,
                seq,
            } => {
                let _ = writeln!(
                    out,
                    "t={at:012} EQUIVOCATION witness={:03} accused={:03} seq={seq:012}",
                    witness.0, accused.0
                );
            }
        }
    }
}

fn render_envelope(e: &Envelope) -> String {
    match e {
        Envelope::Entry(entry) => {
            format!(
                "kind=ENTRY origin={:03} seq={:012} {}",
                entry.node.0,
                entry.seq,
                render_body(&entry.body)
            )
        }
        Envelope::AntiEntropy(vv) => format!("kind=ANTI_ENTROPY vv=[{}]", render_vv(vv)),
    }
}

/// The entry's meaning. M3 is the first milestone in
/// which the body carries anything, and a trace a human cannot read is a trace
/// a human cannot debug. Same canonical rules as everything else here: fixed
/// field order, zero-padded integers, no floats.
fn render_body(b: &Body) -> String {
    match b {
        Body::TaskClaim { task, priority } => {
            format!("body=CLAIM task={task:012} prio={priority:03}")
        }
        Body::Withdraw { task } => format!("body=WITHDRAW task={task:012}"),
        Body::Spend { amount } => format!("body=SPEND amount={amount}"),
    }
}

/// `(node, seq)` pairs ascending by `NodeId`, comma-joined — the same idiom
/// as `Partition::render`.
fn render_vv(vv: &VersionVector) -> String {
    vv.iter()
        .map(|(n, s)| format!("{:03}:{s:012}", n.0))
        .collect::<Vec<_>>()
        .join(",")
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
    use ed25519_dalek::SigningKey;
    use swarm_core::wire::{Body, Hash, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};

    /// A real signed entry, authored by node 1 at seq 7 — matches the
    /// M1/M0 tests' convention of a deterministic, test-only key.
    fn entry() -> swarm_core::wire::Entry {
        let mut bytes = [0u8; 32];
        bytes[0] = 1;
        let key = SigningKey::from_bytes(&bytes);
        UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node: NodeId(1),
            seq: 7,
            prev: Hash::ZERO,
            deps: VersionVector::new(),
            body: Body::TaskClaim {
                task: 3,
                priority: 1,
            },
        }
        .sign(&key)
    }

    #[test]
    fn rendering_is_stable_and_padded() {
        let mut t = Trace::default();
        t.push(TraceRecord::Tick { at: 12 });
        t.push(TraceRecord::Deliver {
            at: 12,
            from: NodeId(1),
            to: NodeId(3),
            payload: Envelope::Entry(entry()),
        });

        assert_eq!(
            t.render(),
            "t=000000000012 TICK\n\
             t=000000000012 DELIVER from=001 to=003 kind=ENTRY origin=001 \
             seq=000000000007 body=CLAIM task=000000000003 prio=001\n"
        );
    }

    /// A claim and a withdrawal for the same task must not render alike:
    /// the trace is a fingerprint of the *model*, and M3's model
    /// distinguishes these two.
    #[test]
    fn claim_and_withdrawal_render_distinctly() {
        let mut e = entry();
        let claim = Envelope::Entry(e.clone());
        e.body = Body::Withdraw { task: 3 };
        let withdraw = Envelope::Entry(e);

        let line = |p: Envelope| {
            let mut t = Trace::default();
            t.push(TraceRecord::Send {
                at: 1,
                from: NodeId(1),
                to: NodeId(2),
                payload: p,
            });
            t.render()
        };
        assert!(line(claim).contains("body=CLAIM task=000000000003 prio=001"));
        assert!(line(withdraw).contains("body=WITHDRAW task=000000000003"));
    }

    #[test]
    fn anti_entropy_renders_its_version_vector() {
        let mut vv = VersionVector::new();
        vv.bump(NodeId(0), 2);
        vv.bump(NodeId(2), 5);
        let mut t = Trace::default();
        t.push(TraceRecord::Send {
            at: 1,
            from: NodeId(0),
            to: NodeId(1),
            payload: Envelope::AntiEntropy(vv),
        });
        assert_eq!(
            t.render(),
            "t=000000000001 SEND from=000 to=001 kind=ANTI_ENTROPY vv=[000:000000000002,002:000000000005]\n"
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
