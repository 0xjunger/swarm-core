//! Invariant tests — written before the code that satisfies them, per
//! `DESIGN.md` §11.7 ("Sıralamayı ters kur: önce invariant, sonra kod").
//!
//! At M1 only **I1** is testable: there is one node, no network, no CRDT and
//! no escrow. I2–I6 are recorded at the bottom of this file with the
//! milestone that activates each, so they are not silently decided by
//! implementation accident.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use swarm_core::log::{verify_chain, ChainError, Log};
use swarm_core::wire::{Body, Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;

/// Deterministic test key. Keys are injected, never generated: randomness
/// does not enter `swarm-core` at all (`DESIGN.md` §11.1).
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

// ---------------------------------------------------------------------------
// I1 — at most one signed entry per (node, seq)
// ---------------------------------------------------------------------------

/// Construction side: an appended chain never reuses a seq, and seqs are
/// contiguous from zero. This is crash monotonicity (`DESIGN.md` §4.3) held
/// structurally — seq is derived from chain length, so there is nothing to
/// reuse after a crash.
#[test]
fn i1_construction_never_reuses_a_seq() {
    let mut log = Log::new(NodeId(0), test_key(0), 64);
    for i in 0..50 {
        log.append(claim(i)).unwrap();
    }

    let pairs: Vec<(NodeId, u64)> = log.entries().iter().map(|e| (e.node, e.seq)).collect();
    let unique: BTreeSet<(NodeId, u64)> = pairs.iter().copied().collect();
    assert_eq!(pairs.len(), unique.len(), "a (node, seq) pair repeated");

    for (i, e) in log.entries().iter().enumerate() {
        assert_eq!(e.seq, i as u64, "seqs must be contiguous from zero");
    }
}

/// Verification side: a chain in which one `(node, seq)` appears twice must
/// not verify. This is the seed of M4's equivocation detection — two signed
/// entries at the same `(node, seq)` are the crime; refusing them here is
/// the first line of defence.
#[test]
fn i1_a_duplicated_seq_never_verifies() {
    let key = test_key(0);
    let mut log = Log::new(NodeId(0), key.clone(), 16);
    for i in 0..8 {
        log.append(claim(i)).unwrap();
    }

    // Duplicate entry 3: (node 0, seq 3) now appears twice.
    let mut entries = log.entries().to_vec();
    entries.insert(4, entries[3].clone());

    let err = verify_chain(&roster_of(NodeId(0), &key), &entries).unwrap_err();
    assert!(
        matches!(
            err,
            ChainError::BadSeq {
                index: 4,
                expected: 4,
                found: 3
            }
        ),
        "unexpected error: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Documented placeholders — not testable at M1, deliberately not stubbed.
// Each names the milestone whose acceptance criterion activates it
// (docs/spec-m1.md §8):
//
//   I2 — an entry is not applied before its deps are delivered.
//        Activates at M2 (causal delivery, 3 nodes).
//   I3 — two nodes that have seen the same entry set derive the same state.
//        Activates at M2 (partition {A,B} | {C}, heal, convergence).
//   I4 — spendable rights across all partitions <= authorised total.
//        Activates at M5 (escrow counter, 1000 seeds).
//   I5 — no safety-critical effect without a valid certificate in the log.
//        Activates with the policy gate (M5).
//   I6 — every effect is traceable to a signed entry chain.
//        Activates when step derives effects from entries (M2+).
// ---------------------------------------------------------------------------
