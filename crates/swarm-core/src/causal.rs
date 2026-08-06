//! Causal dependencies: the version vector (`DESIGN.md` §4.1, §5).
//!
//! The vector itself arrives at M2, when nodes exchange what they have seen.
//! At M1 it exists because `Entry::deps` exists — fields are opened now and
//! filled later (`DESIGN.md`, "Alanları bugünden aç, doldurmayı ertele").
//! Its canonical encoding, however, is already frozen: the golden vector
//! pins it in its empty form.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::NodeId;

/// For each node, the highest `seq` seen from it.
///
/// `BTreeMap`, never `HashMap`: ascending-by-`NodeId` iteration is rule R4
/// (`docs/spec.md` §6), and `NodeId` deliberately does not derive `Hash`
/// (`docs/spec.md` §3.1.1).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct VersionVector {
    inner: BTreeMap<NodeId, u64>,
}

impl VersionVector {
    /// The M1 vector: no network, therefore no dependencies.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// The highest `seq` seen from `node`, or `None` if nothing has been
    /// seen from it yet.
    pub fn highest(&self, node: NodeId) -> Option<u64> {
        self.inner.get(&node).copied()
    }

    /// Records that `seq` from `node` has been seen. Defensive: never
    /// regresses an existing higher value, so a caller bug can only fail to
    /// advance the vector, never roll it back (`docs/spec-m2.md` §3).
    pub fn bump(&mut self, node: NodeId, seq: u64) {
        self.inner
            .entry(node)
            .and_modify(|v| *v = (*v).max(seq))
            .or_insert(seq);
    }

    /// The causal delivery gate (`docs/spec-m2.md` §3-4): `true` iff every
    /// component of `self` is present in `other` with a value `>=` its own.
    /// The empty vector is `≤` everything, including itself.
    pub fn le(&self, other: &Self) -> bool {
        self.inner
            .iter()
            .all(|(node, seq)| other.inner.get(node).is_some_and(|o| o >= seq))
    }

    /// Ascending by `NodeId` (rule R4, `docs/spec.md` §6) — `BTreeMap`
    /// iteration order, by construction.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, u64)> + '_ {
        self.inner.iter().map(|(&n, &s)| (n, s))
    }

    /// Canonical encoding (`docs/spec-m1.md` §3.2): `u16 BE` count, then
    /// `(node u8, seq u64 BE)` pairs ascending by `NodeId` — which is
    /// `BTreeMap` iteration order, so rule R4 holds by construction.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.inner.len() as u16).to_be_bytes());
        for (node, seq) in &self.inner {
            out.push(node.0);
            out.extend_from_slice(&seq.to_be_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_vector_encodes_as_a_zero_count() {
        let mut out = Vec::new();
        VersionVector::new().encode(&mut out);
        assert_eq!(out, [0, 0]);
    }

    #[test]
    fn the_m1_vector_is_empty() {
        assert!(VersionVector::new().is_empty());
    }

    #[test]
    fn highest_is_none_for_an_unseen_node() {
        assert_eq!(VersionVector::new().highest(NodeId(0)), None);
    }

    #[test]
    fn bump_records_and_never_regresses() {
        let mut vv = VersionVector::new();
        vv.bump(NodeId(0), 5);
        assert_eq!(vv.highest(NodeId(0)), Some(5));
        vv.bump(NodeId(0), 2); // lower value: no-op
        assert_eq!(vv.highest(NodeId(0)), Some(5));
        vv.bump(NodeId(0), 9);
        assert_eq!(vv.highest(NodeId(0)), Some(9));
    }

    #[test]
    fn le_holds_when_every_component_is_covered() {
        let mut a = VersionVector::new();
        a.bump(NodeId(0), 2);
        a.bump(NodeId(1), 1);
        let mut b = VersionVector::new();
        b.bump(NodeId(0), 2);
        b.bump(NodeId(1), 3);
        b.bump(NodeId(2), 7);
        assert!(a.le(&b));
        assert!(!b.le(&a));
    }

    #[test]
    fn the_empty_vector_is_le_everything() {
        let mut b = VersionVector::new();
        b.bump(NodeId(0), 1);
        assert!(VersionVector::new().le(&b));
        assert!(VersionVector::new().le(&VersionVector::new()));
    }

    #[test]
    fn le_fails_on_a_missing_component() {
        let mut a = VersionVector::new();
        a.bump(NodeId(5), 1);
        assert!(!a.le(&VersionVector::new()));
    }

    #[test]
    fn iter_is_ascending_by_node_id() {
        let mut vv = VersionVector::new();
        vv.bump(NodeId(2), 1);
        vv.bump(NodeId(0), 1);
        vv.bump(NodeId(1), 1);
        let nodes: Vec<NodeId> = vv.iter().map(|(n, _)| n).collect();
        assert_eq!(nodes, [NodeId(0), NodeId(1), NodeId(2)]);
    }
}
