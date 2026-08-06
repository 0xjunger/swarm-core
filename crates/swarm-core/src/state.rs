//! Derived replicated state (`DESIGN.md` §5's `state/`): the task-claim CRDT.
//!
//! `DESIGN.md` §4.2 names three data types and three difficulty levels. M3
//! implements the third — task claims, `Map<TaskId, ORSet<Claim>>` with the
//! deterministic winner `min by (priority, logical_clock, node_id)`. The LWW
//! telemetry register and the sensor-track OR-set are not in M3's milestone
//! text and are not written here (`docs/spec-m3.md` §11).
//!
//! Everything in this module is a fold over entries. Nothing here reads a
//! clock, draws a random number, or depends on arrival order — which is what
//! makes invariant I3 structural rather than argued (`docs/spec-m3.md` §9).

use alloc::collections::{BTreeMap, BTreeSet};

use crate::wire::{Body, VerifiedEntry};
use crate::NodeId;

/// A task's identity. Deliberately abstract for the whole of Phase 1
/// (`DESIGN.md` §9: "Görev = soyut bir `TaskId`").
pub type TaskId = u64;

/// One node's bid for one task.
///
/// **The field order is the winner rule.** `Ord` is derived, so
/// `(priority, lc, node, seq)` compares in exactly the order `DESIGN.md` §4.2
/// specifies — `min by (priority, logical_clock, node_id)` — with `seq`
/// appended so the ordering is total by construction rather than by
/// assumption (`docs/spec-m3.md` §5). Deriving it means there is no second
/// place where the comparison could drift away from the spec.
///
/// `(node, seq)` is also the OR-set tag: it is the identity of the entry that
/// carried this claim, so it is unique without generating anything.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Claim {
    /// Lower is better. Fixed at 1 for autonomously authored claims
    /// (`docs/spec-m3.md` §6); real priorities arrive with a real mission.
    pub priority: u8,
    /// The derived logical clock: how many entries the author had applied
    /// when it wrote this claim (`docs/spec-m3.md` §3). Lower means causally
    /// earlier.
    pub lc: u64,
    /// The author. The last term of `DESIGN.md` §4.2's rule, and never a wall
    /// clock — §7 forbids that because GPS time can be spoofed.
    pub node: NodeId,
    /// The author's log index for this claim. Totality tie-break only.
    pub seq: u64,
}

/// Every task claim and withdrawal this node has observed.
///
/// `Map<TaskId, ORSet<Claim>>` per `DESIGN.md` §4.2. A `BTreeSet<Claim>` *is*
/// the OR-set here: `Claim` carries its own unique `(node, seq)` tag, so two
/// nodes bidding identically produce two elements rather than one. `remove` is
/// not implemented — see [`Claims::observe`] and `docs/spec-m3.md` §4.1.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Claims {
    by_task: BTreeMap<TaskId, BTreeSet<Claim>>,
    withdrawn: BTreeSet<(TaskId, NodeId)>,
}

impl Claims {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one entry in. The **only** way state enters this structure.
    ///
    /// Takes a [`VerifiedEntry`], never a raw `Entry`: unverified bytes must
    /// not reach state, and making that a signature rather than a convention
    /// turns "forgot to verify" into a compile error (`DESIGN.md`, "Entry ile
    /// nasıl çalışmalı", item 4).
    ///
    /// Both arms are set insertions, so folding is idempotent and
    /// commutative — the same entry set yields the same `Claims` whatever
    /// order it arrives in. That is invariant I3 discharged structurally
    /// (`docs/spec-m3.md` §9).
    ///
    /// Note what `Withdraw` does **not** do: it does not remove the author's
    /// claim. The claim set is grow-only, so the winner is a pure `min` over
    /// it and "losing is monotone" holds (`docs/spec-m3.md` §4.1, §5.1).
    pub fn observe(&mut self, entry: &VerifiedEntry) {
        let e = entry.entry();
        match e.body {
            Body::TaskClaim { task, priority } => {
                self.by_task.entry(task).or_default().insert(Claim {
                    priority,
                    lc: e.deps.entry_count(),
                    node: e.node,
                    seq: e.seq,
                });
            }
            Body::Withdraw { task } => {
                self.withdrawn.insert((task, e.node));
            }
        }
    }

    /// The deterministic winner of `task`, or `None` if nobody has claimed it.
    ///
    /// `BTreeSet::first()` is the minimum under `Claim`'s `Ord`, which is the
    /// winner rule itself — the rule is not restated here, so it cannot be
    /// restated wrongly.
    pub fn winner(&self, task: TaskId) -> Option<Claim> {
        self.by_task.get(&task)?.first().copied()
    }

    /// Every claim for `task`, ascending by the winner rule (best first).
    pub fn claims(&self, task: TaskId) -> impl Iterator<Item = Claim> + '_ {
        self.by_task
            .get(&task)
            .into_iter()
            .flat_map(|s| s.iter().copied())
    }

    /// Every task anyone has claimed, ascending by `TaskId`.
    pub fn tasks(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.by_task.keys().copied()
    }

    /// Whether `node` has published a withdrawal for `task`.
    pub fn has_withdrawn(&self, task: TaskId, node: NodeId) -> bool {
        self.withdrawn.contains(&(task, node))
    }

    /// Whether `node` has claimed `task` — the precondition for owing a
    /// withdrawal (`docs/spec-m3.md` §6).
    pub fn has_claimed(&self, task: TaskId, node: NodeId) -> bool {
        self.claims(task).any(|c| c.node == node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Claim` built directly. The integration tests in `tests/claims.rs`
    /// go through real signed entries; these unit tests exercise the ordering
    /// and the container, which need no signature.
    fn claim(priority: u8, lc: u64, node: u8, seq: u64) -> Claim {
        Claim {
            priority,
            lc,
            node: NodeId(node),
            seq,
        }
    }

    #[test]
    fn ord_follows_priority_then_lc_then_node_then_seq() {
        assert!(claim(1, 9, 9, 9) < claim(2, 0, 0, 0));
        assert!(claim(1, 1, 9, 9) < claim(1, 2, 0, 0));
        assert!(claim(1, 1, 1, 9) < claim(1, 1, 2, 0));
        assert!(claim(1, 1, 1, 1) < claim(1, 1, 1, 2));
    }

    #[test]
    fn an_empty_map_has_no_winner_and_no_tasks() {
        let c = Claims::new();
        assert_eq!(c.winner(0), None);
        assert_eq!(c.tasks().count(), 0);
        assert!(!c.has_claimed(0, NodeId(0)));
        assert!(!c.has_withdrawn(0, NodeId(0)));
    }
}
