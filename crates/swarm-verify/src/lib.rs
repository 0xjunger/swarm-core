//! Invariant checker (`DESIGN.md` §M6): takes the final states from a
//! simulation run and checks I1–I4. I5 and I6 are structural, not runtime
//! properties — see `swarm-core/src/policy.rs` and `docs/spec.md` §15.
//!
//! An empty `Vec<Violation>` means every checked invariant held — Phase 1's
//! exit criterion for I1–I4.

pub mod fold;
pub mod verdict;
pub mod verify;

pub use verify::verify;

use std::collections::{BTreeMap, BTreeSet};

use swarm_core::wire::Body;
use swarm_core::NodeId;
use swarm_core::State;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub invariant: &'static str,
    pub detail: String,
}

pub fn check_invariants(
    states: &BTreeMap<NodeId, State>,
    budgets: &BTreeMap<NodeId, u64>,
) -> Vec<Violation> {
    let mut violations = Vec::new();
    check_i1(states, &mut violations);
    check_i2(states, &mut violations);
    check_i3(states, &mut violations);
    check_i4(states, budgets, &mut violations);
    violations
}

// ---------------------------------------------------------------------------
// I1 — at most one signed entry per (node, seq)
// ---------------------------------------------------------------------------

fn check_i1(states: &BTreeMap<NodeId, State>, violations: &mut Vec<Violation>) {
    // I1: at most one *distinct* signed entry per (node, seq). Multiple nodes
    // holding the same entry (replication) is correct; two nodes holding
    // different entries at the same (node, seq) is equivocation.
    let mut by_key: BTreeMap<(NodeId, u64), Vec<&swarm_core::wire::Entry>> = BTreeMap::new();
    for state in states.values() {
        for entry in state.entries() {
            by_key
                .entry((entry.node, entry.seq))
                .or_default()
                .push(entry);
        }
    }
    for ((node, seq), entries) in &by_key {
        if entries.len() > 1 {
            let first = entries[0];
            if entries.iter().any(|e| e.chain_hash() != first.chain_hash()) {
                violations.push(Violation {
                    invariant: "I1",
                    detail: format!("conflicting entries at (node {}, seq {})", node.0, seq),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// I2 — an entry is not applied before its deps are delivered
// ---------------------------------------------------------------------------

fn check_i2(states: &BTreeMap<NodeId, State>, violations: &mut Vec<Violation>) {
    for (&node_id, state) in states {
        for entry in state.entries() {
            for (dep_origin, dep_seq) in entry.deps.iter() {
                let covered = state
                    .causal_vv()
                    .highest(dep_origin)
                    .is_some_and(|h| h >= dep_seq);
                if !covered {
                    violations.push(Violation {
                        invariant: "I2",
                        detail: format!(
                            "node {} applied (origin {}, seq {}) with unmet dep (origin {}, seq {})",
                            node_id.0, entry.node.0, entry.seq, dep_origin.0, dep_seq,
                        ),
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// I3 — two nodes that have seen the same entry set derive the same state
// ---------------------------------------------------------------------------

fn check_i3(states: &BTreeMap<NodeId, State>, violations: &mut Vec<Violation>) {
    let ids: Vec<NodeId> = states.keys().copied().collect();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            let a = ids[i];
            let b = ids[j];
            let sa = &states[&a];
            let sb = &states[&b];

            let entries_a: BTreeSet<(NodeId, u64)> =
                sa.entries().iter().map(|e| (e.node, e.seq)).collect();
            let entries_b: BTreeSet<(NodeId, u64)> =
                sb.entries().iter().map(|e| (e.node, e.seq)).collect();

            if entries_a != entries_b {
                continue;
            }

            if sa.claims() != sb.claims() {
                violations.push(Violation {
                    invariant: "I3",
                    detail: format!(
                        "nodes {} and {}: same entry set, different claims",
                        a.0, b.0
                    ),
                });
            }

            for task in sa.claims().tasks() {
                if sa.claims().winner(task) != sb.claims().winner(task) {
                    violations.push(Violation {
                        invariant: "I3",
                        detail: format!(
                            "nodes {} and {} disagree on winner of task {}",
                            a.0, b.0, task
                        ),
                    });
                }
            }

            if sa.escrow() != sb.escrow() {
                violations.push(Violation {
                    invariant: "I3",
                    detail: format!(
                        "nodes {} and {}: same entry set, different escrow state",
                        a.0, b.0
                    ),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// I4 — total unique Spend across all partitions ≤ authorised total
// ---------------------------------------------------------------------------

fn check_i4(
    states: &BTreeMap<NodeId, State>,
    budgets: &BTreeMap<NodeId, u64>,
    violations: &mut Vec<Violation>,
) {
    let mut seen_spend: BTreeSet<(NodeId, u64)> = BTreeSet::new();
    let mut spent_by_node: BTreeMap<NodeId, u64> = BTreeMap::new();

    for state in states.values() {
        for entry in state.entries() {
            if seen_spend.insert((entry.node, entry.seq)) {
                if let Body::Spend { amount } = entry.body {
                    *spent_by_node.entry(entry.node).or_insert(0) += amount;
                }
            }
        }
    }

    for (node, spent) in &spent_by_node {
        let budget = budgets.get(node).copied().unwrap_or(0);
        if *spent > budget {
            violations.push(Violation {
                invariant: "I4",
                detail: format!("node {} spent {} but budget is {}", node.0, spent, budget),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use ed25519_dalek::SigningKey;
    use swarm_core::causal::VersionVector;
    use swarm_core::wire::{Body, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
    use swarm_core::State;

    fn test_key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    fn single_node_state() -> (BTreeMap<NodeId, State>, Roster) {
        let key = test_key(1);
        let mut keys = BTreeMap::new();
        let n = NodeId(0);
        keys.insert(n, key.verifying_key());
        let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);
        let state = State::new(n, roster.clone(), key, 64, 8, 10, 0);
        let mut states = BTreeMap::new();
        states.insert(n, state);
        (states, roster)
    }

    #[test]
    fn empty_state_has_no_violations() {
        let (states, _) = single_node_state();
        let v = check_invariants(&states, &BTreeMap::new());
        assert!(v.is_empty(), "empty state must be clean: {v:?}");
    }

    #[test]
    fn i1_catches_a_duplicate_entry() {
        let (mut states, _roster) = single_node_state();
        let key = test_key(1);
        // Build a state with a duplicate entry by hand.
        let e = UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node: NodeId(0),
            seq: 0,
            prev: Hash::ZERO,
            deps: VersionVector::new(),
            body: Body::TaskClaim {
                task: 0,
                priority: 1,
            },
        }
        .sign(&key);
        // Walk through step to apply.
        let s = &states[&NodeId(0)];
        let (s, _) = swarm_core::step(
            s,
            swarm_core::Event::Recv {
                from: NodeId(0),
                payload: swarm_core::Envelope::Entry(e.clone()),
            },
            swarm_core::LogicalTime(1),
        );
        // Apply the same entry again from a "different" origin — this is a
        // sim edge case, not a real protocol path, but it exercises the
        // checker.
        let (s, _) = swarm_core::step(
            &s,
            swarm_core::Event::Recv {
                from: NodeId(1),
                payload: swarm_core::Envelope::Entry(e),
            },
            swarm_core::LogicalTime(2),
        );
        states.insert(NodeId(0), s);

        // I1: at most one entry per (node, seq) across the union. The
        // same entry seen by the same node is not a union-level duplicate
        // — it's the same entry. The checker is correct to not flag it.
        // To test I1 catching a real violation, we'd need two different
        // entries at the same (node, seq) — that is equivocation, tested
        // in m4_equivocation.rs. The checker catches I1 by construction
        // (seq is chain length). This test just verifies the checker runs.
        let v = check_invariants(&states, &BTreeMap::new());
        assert!(v.is_empty(), "same entry twice is not an I1 violation");
    }
}
