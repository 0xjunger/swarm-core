//! Derived replicated state (`DESIGN.md` §5's `state/`): the task-claim CRDT.
//!
//! `DESIGN.md` §4.2 names three data types and three difficulty levels. M3
//! implements the third — task claims, `Map<TaskId, ORSet<Claim>>` with the
//! deterministic winner `min by (priority, logical_clock, node_id)`. The LWW
//! telemetry register and the sensor-track OR-set are not in M3's milestone
//! text and are not written here (`docs/spec.md` §14).
//!
//! Everything in this module is a fold over entries. Nothing here reads a
//! clock, draws a random number, or depends on arrival order — which is what
//! makes invariant I3 structural rather than argued (`docs/spec.md` §13).

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
/// assumption (`docs/spec.md` §10.5). Deriving it means there is no second
/// place where the comparison could drift away from the spec.
///
/// `(node, seq)` is also the OR-set tag: it is the identity of the entry that
/// carried this claim, so it is unique without generating anything.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Claim {
    /// Lower is better. Fixed at 1 for autonomously authored claims
    /// (`docs/spec.md` §10.6); real priorities arrive with a real mission.
    pub priority: u8,
    /// The derived logical clock: how many entries the author had applied
    /// when it wrote this claim (`docs/spec.md` §10.2). Lower means causally
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
/// not implemented — see [`Claims::observe`] and `docs/spec.md` §10.4.
#[derive(Clone, Debug, Default)]
pub struct Claims {
    by_task: BTreeMap<TaskId, BTreeSet<Claim>>,
    withdrawn: BTreeSet<(TaskId, NodeId)>,
    /// The observing node's own id. Only read by [`Claims::winner`] under
    /// the `mutant-i3` feature (docs/spec.md §15, §1) to simulate a
    /// self-preferring tie-break bug; otherwise unused, since the real
    /// winner rule is `Claim`'s `Ord` alone and has no notion of "self".
    #[cfg(feature = "mutant-i3")]
    owner: Option<NodeId>,
}

/// Equality is over observed claims and withdrawals only — `owner` (present
/// only under `mutant-i3`) is bookkeeping about who is asking, not part of
/// the derived state, and must not by itself make two nodes' `Claims` look
/// unequal to `swarm-verify`'s I3 check.
impl PartialEq for Claims {
    fn eq(&self, other: &Self) -> bool {
        self.by_task == other.by_task && self.withdrawn == other.withdrawn
    }
}
impl Eq for Claims {}

impl Claims {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records which node owns this `Claims`, for [`Claims::winner`]'s
    /// `mutant-i3` tie-break (docs/spec.md §15, §1). Does not exist
    /// outside that feature.
    #[cfg(feature = "mutant-i3")]
    pub(crate) fn set_owner(&mut self, me: NodeId) {
        self.owner = Some(me);
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
    /// (`docs/spec.md` §13).
    ///
    /// Note what `Withdraw` does **not** do: it does not remove the author's
    /// claim. The claim set is grow-only, so the winner is a pure `min` over
    /// it and "losing is monotone" holds (`docs/spec.md` §10.4-10.5).
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
            Body::Spend { .. } => { /* not claim-related */ }
        }
    }

    /// The deterministic winner of `task`, or `None` if nobody has claimed it.
    ///
    /// `BTreeSet::first()` is the minimum under `Claim`'s `Ord`, which is the
    /// winner rule itself — the rule is not restated here, so it cannot be
    /// restated wrongly.
    pub fn winner(&self, task: TaskId) -> Option<Claim> {
        let set = self.by_task.get(&task)?;
        // I3 negative control (docs/spec.md §15, §1): a self-preferring
        // tie-break. Two nodes holding the *same* entry set now derive
        // *different* winners whenever both claimed the task — genuine,
        // node-dependent divergence, not a hand-built fake. Never built into
        // a normal binary; `mutant-i3` is off by default.
        #[cfg(feature = "mutant-i3")]
        if let Some(owner) = self.owner {
            if let Some(mine) = set.iter().find(|c| c.node == owner) {
                return Some(*mine);
            }
        }
        set.first().copied()
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
    /// withdrawal (`docs/spec.md` §10.6).
    pub fn has_claimed(&self, task: TaskId, node: NodeId) -> bool {
        self.claims(task).any(|c| c.node == node)
    }
}

/// The escrow counter (`DESIGN.md` §M5): per-node spending capped by a fixed
/// mission-start allocation.
///
/// A node's budget is immutable once set. Spending is cumulative — each
/// `Spend` entry increases the node's total. The local check is `spent[node] +
/// amount <= allocations[node]`. Because every node has its own cap, the
/// global invariant I4 ("total spendable rights across all partitions ≤
/// authorised total") holds structurally — no consensus, no quorum, no
/// handshake.
///
/// Budget transfers (which *would* require a handshake) are not in M5's scope
/// (`docs/spec.md` §13).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Escrow {
    allocations: BTreeMap<NodeId, u64>,
    spent: BTreeMap<NodeId, u64>,
}

impl Escrow {
    pub fn new(budgets: BTreeMap<NodeId, u64>) -> Self {
        Self {
            allocations: budgets,
            spent: BTreeMap::new(),
        }
    }

    pub fn observe(&mut self, entry: &VerifiedEntry) {
        let e = entry.entry();
        if let Body::Spend { amount } = e.body {
            let total = self.spent.entry(e.node).or_insert(0);
            *total = total.saturating_add(amount);
        }
    }

    pub fn remaining(&self, node: NodeId) -> u64 {
        let alloc = self.allocations.get(&node).copied().unwrap_or(0);
        let spent = self.spent.get(&node).copied().unwrap_or(0);
        alloc.saturating_sub(spent)
    }

    pub fn can_spend(&self, node: NodeId, amount: u64) -> bool {
        self.remaining(node) >= amount
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
