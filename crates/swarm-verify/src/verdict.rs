//! `Verdict` and `Witness` (`SPEC.md` §4.6): what `verify` (§7.3)
//! hands back.
//!
//! Every `Violated` result carries a [`Witness`] — the minimal raw signed
//! [`Entry`] values that show the violation. A reader checks a witness
//! independently, against the roster alone, never taking `verify`'s word for
//! it: that is the entire difference between a *verifier* and an *oracle*
//! (`DESIGN.md` D-010).

use swarm_core::fault::Poe;
use swarm_core::log::ChainError;
use swarm_core::state::{Claim, TaskId};
use swarm_core::wire::Entry;
use swarm_core::NodeId;

/// The outcome of checking one invariant.
#[derive(Clone, Debug, PartialEq)]
pub enum InvariantResult {
    /// The invariant held over every observer's view in the bundle.
    Satisfied,
    /// The invariant did not hold; `Witness` is independently checkable
    /// evidence, not a description. Boxed: `Poe` alone carries two full
    /// `Entry` values, and `clippy::large_enum_variant` is right that
    /// leaving that inline would make every `InvariantResult` pay for the
    /// biggest witness even when it is `Satisfied`.
    Violated(Box<Witness>),
    /// The bundle does not contain enough evidence to say either way. The
    /// string names what was missing (`SPEC.md` §4.6) — e.g. an
    /// invariant that needs two observers of the same entry set, and the
    /// bundle holds only one.
    Undetermined(&'static str),
}

/// Self-contained evidence of a specific violation. Every variant carries
/// raw signed `Entry` values, never a summary, a hash, or a derived value —
/// the reader must be able to check the signatures against the roster
/// themselves (`SPEC.md` §4.6).
#[derive(Clone, Debug, PartialEq)]
pub enum Witness {
    /// I1. Self-verifying: roster plus the two signatures is enough
    /// (`swarm_core::fault::verify_poe`).
    Equivocation(Poe),
    /// I2. The entry that was applied, and the `(node, seq)` its `deps`
    /// named that had not yet arrived at `observer` when it was applied.
    UnmetDependency {
        observer: NodeId,
        entry: Entry,
        missing: (NodeId, u64),
    },
    /// I3. Two observers holding the same `(author, seq)` entry set derived
    /// different winners for `task`.
    Divergence {
        a: NodeId,
        b: NodeId,
        task: TaskId,
        winner_a: Option<Claim>,
        winner_b: Option<Claim>,
    },
    /// I4. The node, the budget it was allocated, and every `Spend` entry
    /// of its that pushed the total over that budget.
    Overspend {
        node: NodeId,
        budget: u64,
        entries: Vec<Entry>,
    },
}

/// Why a chain was rejected before any invariant check ran against it:
/// either `swarm_core::log::verify_chain`'s own seven checks (§4.2), or a
/// chain longer than `spec.log_cap` — a `Spec`-level rule (§4.5) foreign to
/// §4.2's chain verification itself, so it gets its own variant rather than
/// being folded into `ChainError`.
#[derive(Clone, Debug, PartialEq)]
pub enum ChainProblem {
    Chain(ChainError),
    TooLong {
        cap: u32,
        actual: usize,
    },
    /// The chain was filed under an author key its entries do not claim.
    /// Not a `ChainError`: §4.2's chain verification only sees a slice of
    /// entries and has no notion of the key they were filed under, so this
    /// is a `LogBundle`-level defect (§4.4), reported here.
    Misfiled {
        declared: NodeId,
        actual: NodeId,
    },
}

/// A chain that failed structural verification.
///
/// Not one of I1–I4: a chain that does not verify at all is not evidence
/// *for* or *against* an invariant, it is malformed evidence, and `verify`
/// says so directly rather than forcing it into one of the four slots.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainFinding {
    pub observer: NodeId,
    pub author: NodeId,
    pub error: ChainProblem,
    /// The chain as held, exactly as decoded — the reader re-runs
    /// `verify_chain` themselves against the same roster to confirm.
    pub entries: Vec<Entry>,
}

/// The result of `verify(bundle, spec)` (`SPEC.md` §4.6).
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// Chains that failed structural verification before any invariant
    /// check could run against them.
    pub chains: Vec<ChainFinding>,
    pub i1: InvariantResult,
    pub i2: InvariantResult,
    pub i3: InvariantResult,
    pub i4: InvariantResult,
    /// I5/I6 are structural properties of the source code
    /// (`swarm-core/src/policy.rs`), not something a log of signed entries
    /// can attest to either way — nothing at runtime to check, so nothing to
    /// put here beyond this note (`SPEC.md` §6.5–§6.6).
    pub structural_note: &'static str,
    /// Always `false` in Phase 1. Kept as a field, not a comment, so the
    /// epistemic ceiling is enforced by the type rather than by
    /// documentation someone could fail to read: "no rule violated" is not
    /// "the input was attested to be genuine" (`SPEC.md` §4.6).
    pub input_attestable: bool,
}

impl Verdict {
    /// `true` if every invariant is `Satisfied` and no chain failed
    /// structural verification — the CLI's exit-code-0 condition.
    /// `Undetermined` does not count as a failure here; it is reported, not
    /// treated as a violation.
    pub fn all_satisfied(&self) -> bool {
        self.chains.is_empty()
            && [&self.i1, &self.i2, &self.i3, &self.i4]
                .iter()
                .all(|r| matches!(r, InvariantResult::Satisfied))
    }

    /// `true` if any invariant is `Violated` or any chain failed to verify —
    /// the CLI's exit-code-1 condition.
    pub fn any_violated(&self) -> bool {
        !self.chains.is_empty()
            || [&self.i1, &self.i2, &self.i3, &self.i4]
                .iter()
                .any(|r| matches!(r, InvariantResult::Violated(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use std::collections::BTreeMap;
    use swarm_core::causal::VersionVector;
    use swarm_core::fault::verify_poe;
    use swarm_core::wire::{Body, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    fn entry_at(node: NodeId, seq: u64, task: u64, k: &SigningKey) -> Entry {
        UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node,
            seq,
            prev: Hash::ZERO,
            deps: VersionVector::new(),
            body: Body::TaskClaim { task, priority: 1 },
        }
        .sign(k)
    }

    /// The E4 acceptance criterion: a `Poe` built independently of any
    /// `verify` code and wrapped in `Witness::Equivocation` still verifies
    /// against the roster alone — the verdict-producing code is never
    /// touched.
    #[test]
    fn the_poe_inside_an_equivocation_witness_verifies_independently() {
        let k = key(1);
        let x = entry_at(NodeId(0), 3, 1, &k);
        let y = entry_at(NodeId(0), 3, 2, &k);
        let poe = Poe::new(x, y).expect("distinct entries at the same (node, seq)");

        let witness = Witness::Equivocation(poe.clone());
        let Witness::Equivocation(recovered) = witness else {
            unreachable!()
        };

        let mut keys = BTreeMap::new();
        keys.insert(NodeId(0), k.verifying_key());
        let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

        assert!(verify_poe(&roster, &recovered).is_ok());
    }

    #[test]
    fn all_satisfied_requires_every_invariant_satisfied_and_no_chain_findings() {
        let clean = Verdict {
            chains: Vec::new(),
            i1: InvariantResult::Satisfied,
            i2: InvariantResult::Satisfied,
            i3: InvariantResult::Satisfied,
            i4: InvariantResult::Satisfied,
            structural_note: "I5/I6 structural",
            input_attestable: false,
        };
        assert!(clean.all_satisfied());
        assert!(!clean.any_violated());
    }

    #[test]
    fn undetermined_is_neither_satisfied_nor_violated() {
        let mixed = Verdict {
            chains: Vec::new(),
            i1: InvariantResult::Satisfied,
            i2: InvariantResult::Undetermined("only one observer"),
            i3: InvariantResult::Undetermined("only one observer"),
            i4: InvariantResult::Satisfied,
            structural_note: "I5/I6 structural",
            input_attestable: false,
        };
        assert!(!mixed.all_satisfied());
        assert!(!mixed.any_violated());
    }
}
