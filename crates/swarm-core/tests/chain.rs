//! M1 acceptance tests (`DESIGN.md` §M1, "Bitti sayılır"):
//!
//! 1. a chain of **1000 entries** is produced and verified end to end;
//! 2. one altered byte in a record in the **middle** of the chain breaks
//!    verification — the mandatory tamper test, and the only concrete
//!    evidence of the tamper-resistance claim.
//!
//! The remaining tests follow M0's discipline (`DESIGN.md` §M6): an
//! acceptance criterion that can be met vacuously is worth nothing, so every
//! failure mode of the verifier is shown to actually fire.

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, SigningKey};
use swarm_core::causal::VersionVector;
use swarm_core::log::{verify_chain, ChainError, Log, LogError};
use swarm_core::wire::{Body, Hash, Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;

fn test_key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    SigningKey::from_bytes(&bytes)
}

fn roster_of(node: NodeId, key: &SigningKey) -> Roster {
    let mut keys = BTreeMap::new();
    keys.insert(node, key.verifying_key());
    Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys)
}

fn claim(task: u64) -> Body {
    Body::TaskClaim { task, priority: 1 }
}

fn chain_of(key: &SigningKey, node: NodeId, len: usize) -> Log {
    let mut log = Log::new(node, key.clone(), len.max(1));
    for i in 0..len {
        log.append(claim(i as u64), VersionVector::new()).unwrap();
    }
    log
}

// ---------------------------------------------------------------------------
// The M1 criterion
// ---------------------------------------------------------------------------

#[test]
fn a_chain_of_1000_entries_verifies_end_to_end() {
    let key = test_key(1);
    let log = chain_of(&key, NodeId(0), 1000);

    let verified = verify_chain(&roster_of(NodeId(0), &key), log.entries()).unwrap();
    assert_eq!(verified.len(), 1000);
}

/// The mandatory test (`DESIGN.md` §M1): one byte of a middle record altered
/// by hand must break verification. `priority` occupies exactly one byte of
/// the canonical encoding, so bumping it alters exactly one byte.
#[test]
fn one_altered_byte_in_a_middle_entry_breaks_verification() {
    let key = test_key(1);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    let mid = &mut entries[500];
    // M3 added `Body::Withdraw`, so this destructure is no longer
    // irrefutable — which is the notice `docs/spec.md` §8.2 intended. It is
    // still `chain_of` that builds this chain and it writes claims only, so
    // the other variant is genuinely unreachable here.
    let Body::TaskClaim { task, priority } = mid.body else {
        panic!("chain_of writes TaskClaim entries only");
    };
    mid.body = Body::TaskClaim {
        task,
        priority: priority + 1,
    };

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(err, ChainError::BadSignature { index: 500 });
}

// ---------------------------------------------------------------------------
// Guards: every failure mode of the verifier must actually fire
// ---------------------------------------------------------------------------

#[test]
fn a_tampered_signature_is_rejected() {
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    let mut sig = entries[500].sig.to_bytes();
    sig[10] ^= 1; // exactly one byte of the full encoding
    entries[500].sig = Signature::from_bytes(&sig);

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(err, ChainError::BadSignature { index: 500 });
}

#[test]
fn a_broken_link_is_rejected() {
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    entries[500].prev = Hash::ZERO;

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(err, ChainError::BadPrevLink { index: 500 });
}

#[test]
fn a_seq_gap_is_rejected() {
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    entries[500].seq += 1;

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(
        err,
        ChainError::BadSeq {
            index: 500,
            expected: 500,
            found: 501
        }
    );
}

#[test]
fn a_foreign_mission_is_rejected() {
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    entries[500].mission_id[0] ^= 1;

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(err, ChainError::WrongMission { index: 500 });
}

#[test]
fn a_foreign_epoch_is_rejected() {
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    entries[500].epoch += 1;

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(err, ChainError::WrongEpoch { index: 500 });
}

#[test]
fn an_unknown_author_is_rejected() {
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    entries[500].node = NodeId(9);

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(
        err,
        ChainError::UnknownNode {
            index: 500,
            node: NodeId(9)
        }
    );
}

#[test]
fn a_chain_mixing_two_nodes_is_rejected() {
    // A chain belongs to exactly one node: seq is a per-node counter, so a
    // mixed chain has no coherent meaning even if every signature verifies.
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 1000);

    let mut entries = log.entries().to_vec();
    entries[500].node = NodeId(1); // in the roster below, but not this chain's node

    let mut keys = BTreeMap::new();
    keys.insert(NodeId(0), key.verifying_key());
    keys.insert(NodeId(1), test_key(3).verifying_key());
    let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

    let err = verify_chain(&roster, &entries).unwrap_err();
    assert_eq!(
        err,
        ChainError::ChainNodeMismatch {
            index: 500,
            expected: NodeId(0),
            found: NodeId(1)
        }
    );
}

#[test]
fn a_signature_from_the_wrong_key_is_rejected() {
    // Entry 500 re-signed by a different key: links and seqs untouched, only
    // the signature is wrong.
    let key = test_key(2);
    let log = chain_of(&key, NodeId(0), 501);

    let mut entries = log.entries().to_vec();
    let e = &entries[500];
    let mut bytes = [0u8; 32];
    bytes[0] = 99;
    let attacker = SigningKey::from_bytes(&bytes);
    entries[500] = swarm_core::wire::UnsignedEntry {
        mission_id: e.mission_id,
        epoch: e.epoch,
        node: e.node,
        seq: e.seq,
        prev: e.prev,
        deps: e.deps.clone(),
        body: e.body,
    }
    .sign(&attacker);

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert_eq!(err, ChainError::BadSignature { index: 500 });
}

// ---------------------------------------------------------------------------
// Shape properties of the chain and its bound
// ---------------------------------------------------------------------------

#[test]
fn every_prefix_of_a_valid_chain_is_valid() {
    // Truncation is not tampering: any prefix of a verified chain must
    // verify on its own. M2's anti-entropy relies on exchanging prefixes.
    let key = test_key(4);
    let log = chain_of(&key, NodeId(0), 1000);
    let roster = roster_of(NodeId(0), &key);

    for len in [1usize, 2, 100, 999, 1000] {
        assert_eq!(
            verify_chain(&roster, &log.entries()[..len]).unwrap().len(),
            len
        );
    }
}

#[test]
fn an_empty_chain_verifies_to_nothing() {
    let key = test_key(4);
    assert!(verify_chain(&roster_of(NodeId(0), &key), &[])
        .unwrap()
        .is_empty());
}

#[test]
fn the_log_bound_is_enforced_by_refusal() {
    // docs/spec.md §8.4: eviction is not safe until the MMR exists, so a
    // full log refuses to grow rather than silently dropping history.
    let mut log = Log::new(NodeId(0), test_key(5), 2);
    log.append(claim(0), VersionVector::new()).unwrap();
    log.append(claim(1), VersionVector::new()).unwrap();

    assert_eq!(
        log.append(claim(2), VersionVector::new()),
        Err(LogError::Full)
    );
    assert_eq!(log.len(), 2, "the bound must hold after refusal");
}
