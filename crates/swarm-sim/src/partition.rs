//! Network partitions: which nodes can currently reach which.
//!
//! A partition assigns every node to a group. Two nodes can exchange messages if
//! and only if they share a group.

use std::collections::BTreeMap;
use swarm_core::NodeId;

/// A partition of the roster into mutually unreachable groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    /// `BTreeMap`, not `HashMap`: iteration order is part of the trace, and
    /// `HashMap`'s order is nondeterministic. See `DESIGN.md` D-003.
    groups: BTreeMap<NodeId, u8>,
}

impl Partition {
    /// The connected network: everyone in one group.
    pub fn connected(roster: &[NodeId]) -> Self {
        Self {
            groups: roster.iter().map(|&n| (n, 0u8)).collect(),
        }
    }

    /// Splits the roster into the given groups, e.g. `&[&[A, B], &[C]]`.
    ///
    /// Any node not listed keeps group 0; in practice callers list everyone.
    pub fn split(groups: &[&[NodeId]]) -> Self {
        let mut m = BTreeMap::new();
        for (gid, members) in groups.iter().enumerate() {
            for &n in members.iter() {
                m.insert(n, gid as u8);
            }
        }
        Self { groups: m }
    }

    /// Whether `a` and `b` can currently exchange messages.
    pub fn reachable(&self, a: NodeId, b: NodeId) -> bool {
        match (self.groups.get(&a), self.groups.get(&b)) {
            (Some(x), Some(y)) => x == y,
            // A node outside the partition map is unreachable rather than
            // universally reachable: an unknown node should fail closed.
            _ => false,
        }
    }

    /// Canonical single-line rendering for the trace. Ascending by `NodeId`.
    pub fn render(&self) -> String {
        self.groups
            .iter()
            .map(|(n, g)| format!("{:03}:{:03}", n.0, g))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: NodeId = NodeId(0);
    const B: NodeId = NodeId(1);
    const C: NodeId = NodeId(2);

    #[test]
    fn connected_reaches_everyone() {
        let p = Partition::connected(&[A, B, C]);
        assert!(p.reachable(A, C));
        assert!(p.reachable(C, A));
    }

    #[test]
    fn split_blocks_across_groups_only() {
        // The {A,B} | {C} split that M2's acceptance criterion uses.
        let p = Partition::split(&[&[A, B], &[C]]);
        assert!(p.reachable(A, B));
        assert!(!p.reachable(A, C));
        assert!(!p.reachable(C, B));
        assert!(p.reachable(C, C));
    }

    #[test]
    fn unknown_node_fails_closed() {
        let p = Partition::connected(&[A, B]);
        assert!(!p.reachable(A, NodeId(9)));
    }

    #[test]
    fn render_is_ordered_by_node_id() {
        let p = Partition::split(&[&[C, A], &[B]]);
        assert_eq!(p.render(), "000:000,001:001,002:000");
    }
}
