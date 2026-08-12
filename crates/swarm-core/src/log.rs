//! The per-node hash chain (`DESIGN.md` D-008, D-002): the write path.
//!
//! The proof path — the MMR — arrives later (`DESIGN.md` D-008). Until it
//! does, the chain keeps every entry, and its bound is enforced by refusing
//! to grow rather than by evicting (`SPEC.md` §4.2).

use alloc::vec::Vec;

use ed25519_dalek::SigningKey;

use crate::causal::VersionVector;
use crate::wire::{
    Body, Entry, Hash, Roster, UnsignedEntry, VerifiedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID,
};
use crate::NodeId;

/// A node's hash chain: append-only, signed, linked.
///
/// `PartialEq`/`Eq` derive cleanly: `SigningKey` implements both (a
/// constant-time comparison), which is what lets [`crate::State`] keep
/// deriving them once it embeds a `Log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Log {
    me: NodeId,
    key: SigningKey,
    mission_id: [u8; 32],
    epoch: u32,
    entries: Vec<Entry>,
    cap: usize,
}

/// Why an append failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogError {
    /// The chain has reached its stated bound. Eviction is not safe until the
    /// MMR exists (`DESIGN.md` D-008), so a full log refuses rather than drops
    /// (`SPEC.md` §4.2).
    Full,
}

impl Log {
    /// Creates an empty chain for `me`, bounded by `cap`.
    ///
    /// Phase 1 fixes `mission_id` and `epoch` (`SPEC.md` §4.1); the
    /// fields exist now so that real values later change nothing structural.
    ///
    /// # Panics
    ///
    /// If `cap` is zero: a chain that can never hold an entry is a
    /// configuration error, and every structure in this system has a stated,
    /// usable bound (`DESIGN.md` D-013).
    pub fn new(me: NodeId, key: SigningKey, cap: usize) -> Self {
        assert!(cap >= 1, "log cap must be at least 1");
        Log {
            me,
            key,
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            entries: Vec::new(),
            cap,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The `seq` the next appended entry will carry.
    ///
    /// It is derived from the chain length, so a `seq` can never be reused:
    /// crash monotonicity (`DESIGN.md` D-008) holds structurally here, because
    /// a pure state machine has no persistent tail to lose.
    pub fn next_seq(&self) -> u64 {
        self.entries.len() as u64
    }

    /// Appends a new entry: links it to the current head, signs it, records
    /// it. Returns the recorded entry.
    ///
    /// `deps` is the caller's causal snapshot at the moment of authorship
    /// (`SPEC.md` §4.3) — empty at M1, populated from M2 onward. The
    /// field is activated here, not reinterpreted: M1 callers pass
    /// `VersionVector::new()` and their behaviour is unchanged.
    pub fn append(&mut self, body: Body, deps: VersionVector) -> Result<&Entry, LogError> {
        if self.entries.len() == self.cap {
            return Err(LogError::Full);
        }
        let prev = self.entries.last().map_or(Hash::ZERO, Entry::chain_hash);
        let entry = UnsignedEntry {
            mission_id: self.mission_id,
            epoch: self.epoch,
            node: self.me,
            seq: self.next_seq(),
            prev,
            deps,
            body,
        }
        .sign(&self.key);
        self.entries.push(entry);
        Ok(self.entries.last().expect("entry pushed immediately above"))
    }
}

/// The first reason a chain failed to verify, with the offending index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChainError {
    /// The author is not a member of the roster.
    UnknownNode { index: usize, node: NodeId },
    /// A chain belongs to exactly one node: this entry's author differs from
    /// the first entry's.
    ChainNodeMismatch {
        index: usize,
        expected: NodeId,
        found: NodeId,
    },
    /// Cross-mission replay: the entry names a different mission than the
    /// roster's (`DESIGN.md` D-008).
    WrongMission { index: usize },
    /// The entry names a different roster version than the roster's.
    WrongEpoch { index: usize },
    /// A gap or a duplicate in `seq`. At M1 this is invariant I1: two entries
    /// can never share a `(node, seq)`.
    BadSeq {
        index: usize,
        expected: u64,
        found: u64,
    },
    /// `prev` does not match the predecessor's chain hash (or `ZERO` for the
    /// first entry).
    BadPrevLink { index: usize },
    /// The Ed25519 signature does not verify under the author's roster key.
    BadSignature { index: usize },
}

/// Verifies a single entry against a specific expected position in some
/// chain (`SPEC.md` §4.2, minus the single-author check, which only
/// makes sense across a batch — see [`verify_chain`]).
///
/// The check order is fixed: membership, mission, epoch, seq, link,
/// signature. `index` is only used to label errors; callers outside a batch
/// (M2's causal delivery, `SPEC.md` §4.3) may pass `0`.
pub fn verify_next(
    roster: &Roster,
    index: usize,
    expected_seq: u64,
    expected_prev: Hash,
    entry: &Entry,
) -> Result<VerifiedEntry, ChainError> {
    let Some(key) = roster.key(entry.node) else {
        return Err(ChainError::UnknownNode {
            index,
            node: entry.node,
        });
    };
    if entry.mission_id != roster.mission_id {
        return Err(ChainError::WrongMission { index });
    }
    if entry.epoch != roster.epoch {
        return Err(ChainError::WrongEpoch { index });
    }
    if entry.seq != expected_seq {
        return Err(ChainError::BadSeq {
            index,
            expected: expected_seq,
            found: entry.seq,
        });
    }
    if entry.prev != expected_prev {
        return Err(ChainError::BadPrevLink { index });
    }
    if key
        .verify_strict(&entry.signing_bytes(), &entry.sig)
        .is_err()
    {
        return Err(ChainError::BadSignature { index });
    }

    Ok(VerifiedEntry::from_verified(entry.clone()))
}

/// Verifies a chain end to end (`SPEC.md` §4.2) and returns its
/// entries as [`VerifiedEntry`], or the first failure.
///
/// The check order is fixed: membership, single-author, mission, epoch, seq,
/// link, signature. An empty chain verifies to an empty result. Per-entry
/// checks (everything but single-author) are delegated to [`verify_next`].
pub fn verify_chain(roster: &Roster, entries: &[Entry]) -> Result<Vec<VerifiedEntry>, ChainError> {
    let mut verified = Vec::with_capacity(entries.len());
    let mut expected_prev = Hash::ZERO;

    for (index, entry) in entries.iter().enumerate() {
        // Membership is checked here too (not only inside `verify_next`) so
        // that its error takes precedence over single-author, matching the
        // fixed check order membership -> single-author -> ... (unchanged
        // from M1).
        if roster.key(entry.node).is_none() {
            return Err(ChainError::UnknownNode {
                index,
                node: entry.node,
            });
        }
        if index > 0 && entry.node != entries[0].node {
            return Err(ChainError::ChainNodeMismatch {
                index,
                expected: entries[0].node,
                found: entry.node,
            });
        }
        // seq must be contiguous from 0, so the expected value at `index` is
        // `index` itself. Enforcing it here is invariant I1 at M1.
        let v = verify_next(roster, index, index as u64, expected_prev, entry)?;
        expected_prev = v.entry().chain_hash();
        verified.push(v);
    }

    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn claim(task: u64) -> Body {
        Body::TaskClaim { task, priority: 1 }
    }

    #[test]
    fn the_genesis_entry_links_to_zero() {
        let mut log = Log::new(NodeId(0), key(0), 4);
        log.append(claim(0), VersionVector::new()).unwrap();
        assert_eq!(log.entries()[0].prev, Hash::ZERO);
        assert_eq!(log.entries()[0].seq, 0);
    }

    #[test]
    fn each_entry_links_to_its_predecessors_full_encoding() {
        let mut log = Log::new(NodeId(0), key(0), 4);
        log.append(claim(0), VersionVector::new()).unwrap();
        log.append(claim(1), VersionVector::new()).unwrap();
        assert_eq!(log.entries()[1].prev, log.entries()[0].chain_hash());
    }

    #[test]
    fn a_built_chain_verifies() {
        let k = key(0);
        let mut log = Log::new(NodeId(0), k.clone(), 4);
        for i in 0..4 {
            log.append(claim(i), VersionVector::new()).unwrap();
        }
        assert_eq!(
            verify_chain(&roster_of(NodeId(0), &k), log.entries())
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn a_full_log_refuses_to_grow() {
        let mut log = Log::new(NodeId(0), key(0), 2);
        log.append(claim(0), VersionVector::new()).unwrap();
        log.append(claim(1), VersionVector::new()).unwrap();
        assert_eq!(
            log.append(claim(2), VersionVector::new()),
            Err(LogError::Full)
        );
        assert_eq!(log.len(), 2);
    }

    #[test]
    #[should_panic(expected = "log cap must be at least 1")]
    fn zero_capacity_is_rejected() {
        Log::new(NodeId(0), key(0), 0);
    }
}
