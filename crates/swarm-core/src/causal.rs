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
}
