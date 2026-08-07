//! Equivocation detection (`DESIGN.md` §4.4): the proof is the two signatures.
//!
//! A [`Poe`] (proof of equivocation) needs nothing beyond itself and the
//! roster to verify. That is the whole point (`DESIGN.md` §4.4): "kanıt
//! kendi kendini doğruladığı için suçlu node'u dışlamak konsensüs
//! gerektirmez" — no consensus, no context, no witness other than the two
//! signatures being checked against the roster key of the node they accuse.

use crate::wire::{Entry, Roster};

/// Two signed entries at the same `(node, seq)` with different content.
///
/// Ordered canonically by full encoding so that two nodes independently
/// constructing a proof for the same pair store byte-identical structures
/// (`a` is always the lexicographically smaller encoding).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Poe {
    a: Entry,
    b: Entry,
}

impl Poe {
    /// Builds a proof from two entries, or `None` if they do not actually
    /// conflict: same author, same `seq`, different bytes. Two deliveries of
    /// the identical entry are not equivocation (`docs/spec.md` §9.3) — that
    /// is honest re-delivery, not a proof of anything.
    pub fn new(x: Entry, y: Entry) -> Option<Self> {
        if x.node != y.node || x.seq != y.seq {
            return None;
        }
        if x.encoded() == y.encoded() {
            return None;
        }
        if x.encoded() <= y.encoded() {
            Some(Poe { a: x, b: y })
        } else {
            Some(Poe { a: y, b: x })
        }
    }

    pub fn node(&self) -> crate::NodeId {
        self.a.node
    }

    pub fn seq(&self) -> u64 {
        self.a.seq
    }

    pub fn a(&self) -> &Entry {
        &self.a
    }

    pub fn b(&self) -> &Entry {
        &self.b
    }
}

/// Why a claimed proof does not hold up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoeError {
    /// The two entries do not name the same author.
    NodeMismatch,
    /// The two entries do not name the same `seq`.
    SeqMismatch,
    /// The two entries are byte-identical — not a conflict.
    NotDistinct,
    /// The accused `node` is not in the roster the verifier is checking
    /// against.
    UnknownNode,
    /// One of the two signatures does not verify under the roster key.
    BadSignature,
}

/// Verifies a [`Poe`] against `roster` alone — no log, no peer, no context
/// beyond the roster's public keys (`DESIGN.md` §4.4). Any third node holding
/// the same roster reaches the same verdict unilaterally.
pub fn verify_poe(roster: &Roster, poe: &Poe) -> Result<(), PoeError> {
    if poe.a.node != poe.b.node {
        return Err(PoeError::NodeMismatch);
    }
    if poe.a.seq != poe.b.seq {
        return Err(PoeError::SeqMismatch);
    }
    if poe.a.encoded() == poe.b.encoded() {
        return Err(PoeError::NotDistinct);
    }
    let Some(key) = roster.key(poe.a.node) else {
        return Err(PoeError::UnknownNode);
    };
    if key
        .verify_strict(&poe.a.signing_bytes(), &poe.a.sig)
        .is_err()
    {
        return Err(PoeError::BadSignature);
    }
    if key
        .verify_strict(&poe.b.signing_bytes(), &poe.b.sig)
        .is_err()
    {
        return Err(PoeError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::causal::VersionVector;
    use crate::wire::{Body, Hash, PHASE1_EPOCH, PHASE1_MISSION_ID};
    use crate::NodeId;
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    fn roster_of(node: NodeId, key: &SigningKey) -> Roster {
        let mut keys = alloc::collections::BTreeMap::new();
        keys.insert(node, key.verifying_key());
        Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys)
    }

    fn entry_at(node: NodeId, seq: u64, task: u64, k: &SigningKey) -> Entry {
        crate::wire::UnsignedEntry {
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

    #[test]
    fn a_conflicting_pair_builds_and_verifies() {
        let k = key(1);
        let x = entry_at(NodeId(0), 3, 1, &k);
        let y = entry_at(NodeId(0), 3, 2, &k);
        let poe = Poe::new(x, y).expect("distinct entries at the same (node, seq)");
        assert!(verify_poe(&roster_of(NodeId(0), &k), &poe).is_ok());
    }

    #[test]
    fn construction_is_order_independent() {
        let k = key(1);
        let x = entry_at(NodeId(0), 3, 1, &k);
        let y = entry_at(NodeId(0), 3, 2, &k);
        let p1 = Poe::new(x.clone(), y.clone()).unwrap();
        let p2 = Poe::new(y, x).unwrap();
        assert_eq!(p1, p2);
    }

    #[test]
    fn the_same_entry_twice_is_not_equivocation() {
        let k = key(1);
        let x = entry_at(NodeId(0), 3, 1, &k);
        assert!(Poe::new(x.clone(), x).is_none());
    }

    #[test]
    fn different_seq_is_not_equivocation() {
        let k = key(1);
        let x = entry_at(NodeId(0), 3, 1, &k);
        let y = entry_at(NodeId(0), 4, 1, &k);
        assert!(Poe::new(x, y).is_none());
    }

    #[test]
    fn different_author_is_not_equivocation() {
        let k1 = key(1);
        let k2 = key(2);
        let x = entry_at(NodeId(0), 3, 1, &k1);
        let y = entry_at(NodeId(1), 3, 1, &k2);
        assert!(Poe::new(x, y).is_none());
    }

    #[test]
    fn a_tampered_signature_fails_verification() {
        let k = key(1);
        let x = entry_at(NodeId(0), 3, 1, &k);
        let mut y = entry_at(NodeId(0), 3, 2, &k);
        let mut sig = y.sig.to_bytes();
        sig[0] ^= 1;
        y.sig = ed25519_dalek::Signature::from_bytes(&sig);
        let poe = Poe::new(x, y).unwrap();
        assert_eq!(
            verify_poe(&roster_of(NodeId(0), &k), &poe),
            Err(PoeError::BadSignature)
        );
    }

    #[test]
    fn an_unknown_node_is_rejected() {
        let k = key(1);
        let other_key = key(9);
        let x = entry_at(NodeId(0), 3, 1, &k);
        let y = entry_at(NodeId(0), 3, 2, &k);
        let poe = Poe::new(x, y).unwrap();
        // A roster that does not contain node 0 at all.
        assert_eq!(
            verify_poe(&roster_of(NodeId(9), &other_key), &poe),
            Err(PoeError::UnknownNode)
        );
    }

    #[test]
    fn a_forged_second_entry_under_the_wrong_key_fails() {
        // A peer cannot frame an honest node: a "conflicting" entry claimed
        // to be from `node` but signed by someone else's key does not verify.
        let honest = key(1);
        let attacker = key(2);
        let x = entry_at(NodeId(0), 3, 1, &honest);
        let mut forged = entry_at(NodeId(0), 3, 2, &attacker);
        forged.node = NodeId(0);
        let poe = Poe::new(x, forged).unwrap();
        assert_eq!(
            verify_poe(&roster_of(NodeId(0), &honest), &poe),
            Err(PoeError::BadSignature)
        );
    }
}
