//! Invariant tests — written before the code that satisfies them, per
//! `DESIGN.md` §11.7 ("Sıralamayı ters kur: önce invariant, sonra kod").
//!
//! At M1 only I1 was testable: one node, no network, no CRDT, no escrow.
//! M2 activates I2 and I3 (`docs/spec-m2.md` §9); I4–I6 are still recorded
//! at the bottom of this file as documented placeholders. `tests/causal.rs`
//! covers the causal-delivery *mechanism* in depth (multi-origin deps, the
//! buffer bound, anti-entropy) — the tests here restate I2 and I3
//! specifically as invariants, minimally, so each stands on its own.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use swarm_core::causal::VersionVector;
use swarm_core::log::{verify_chain, ChainError, Log};
use swarm_core::wire::{Body, Entry, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Envelope, Event, LogicalTime, NodeId, State};

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
        log.append(claim(i), VersionVector::new()).unwrap();
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
        log.append(claim(i), VersionVector::new()).unwrap();
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
// M2 helpers — a 3-node roster (author `A`, two observers `P`, `Q`) and a
// hand-built chain, so I2/I3 can be exercised without a simulator.
// ---------------------------------------------------------------------------

fn m2_roster() -> (Roster, [SigningKey; 3]) {
    let keys = [test_key(11), test_key(12), test_key(13)];
    let mut m = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        m.insert(NodeId(i as u8), k.verifying_key());
    }
    (Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, m), keys)
}

/// `A`'s chain, built by hand with the self-inclusive `deps` M2 specifies
/// (`docs/spec-m2.md` §3): each entry's `deps` names only its author's own
/// immediate predecessor.
fn a_chain(a: NodeId, key: &SigningKey) -> [Entry; 3] {
    let e0 = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: a,
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: claim(0),
    }
    .sign(key);
    let mut deps1 = VersionVector::new();
    deps1.bump(a, 0);
    let e1 = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: a,
        seq: 1,
        prev: e0.chain_hash(),
        deps: deps1,
        body: claim(1),
    }
    .sign(key);
    let mut deps2 = VersionVector::new();
    deps2.bump(a, 1);
    let e2 = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: a,
        seq: 2,
        prev: e1.chain_hash(),
        deps: deps2,
        body: claim(2),
    }
    .sign(key);
    [e0, e1, e2]
}

fn deliver(state: &State, from: NodeId, entry: Entry, at: u64) -> State {
    let (next, _) = step(
        state,
        Event::Recv {
            from,
            payload: Envelope::Entry(entry),
        },
        LogicalTime(at),
    );
    next
}

// ---------------------------------------------------------------------------
// I2 — an entry is not applied before its deps are delivered
// ---------------------------------------------------------------------------

/// Minimal restatement of I2 as an invariant test: `docs/spec-m2.md` §4's
/// delivery rule is what enforces it. `tests/causal.rs`'s
/// `i2_an_entry_is_not_applied_before_all_its_cross_node_deps_are_delivered`
/// exercises the harder multi-origin case; this is the one-dependency case,
/// kept here so I2 has a test that names it directly.
#[test]
fn i2_an_entry_is_not_applied_before_its_deps_are_delivered() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let observer = NodeId(2);
    let [e0, e1, _] = a_chain(a, &keys[0]);

    let s = State::new(observer, roster, keys[2].clone(), 64, 8, 0, 0);
    let s = deliver(&s, a, e1.clone(), 1);
    assert_eq!(
        s.causal_vv().highest(a),
        None,
        "I2 violated: e1 applied before e0, its dependency, was delivered"
    );
    assert_eq!(s.buffer_keys().collect::<Vec<_>>(), [(a, 1)]);

    let s = deliver(&s, a, e0, 2);
    assert_eq!(s.causal_vv().highest(a), Some(1), "now both are applied");
}

// ---------------------------------------------------------------------------
// I3 — two nodes that have seen the same entry set derive the same state
// ---------------------------------------------------------------------------

#[test]
fn i3_same_entries_different_arrival_order_converge_to_identical_state() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let (p, q) = (NodeId(1), NodeId(2));
    let [e0, e1, e2] = a_chain(a, &keys[0]);

    // P receives them in causal order — never buffers.
    let p_state = State::new(p, roster.clone(), keys[1].clone(), 64, 8, 0, 0);
    let p_state = deliver(&p_state, a, e0.clone(), 1);
    let p_state = deliver(&p_state, a, e1.clone(), 2);
    let p_state = deliver(&p_state, a, e2.clone(), 3);

    // Q receives the same three entries in reverse — each of the first two
    // is buffered, then the arrival of e0 drains all of them in one step.
    let q_state = State::new(q, roster, keys[2].clone(), 64, 8, 0, 0);
    let q_state = deliver(&q_state, a, e2, 1);
    let q_state = deliver(&q_state, a, e1, 2);
    let q_state = deliver(&q_state, a, e0, 3);

    assert_eq!(
        p_state.causal_vv().highest(a),
        q_state.causal_vv().highest(a)
    );
    assert_eq!(p_state.causal_vv().highest(a), Some(2));
    assert_eq!(
        p_state.entries(),
        q_state.entries(),
        "I3: same entry set, different arrival order, must derive the same state"
    );
    assert_eq!(q_state.buffer_keys().count(), 0, "fully drained");
}

/// I3 at M3 (`docs/spec-m3.md` §9): "derived state" now means more than the
/// version vector. Two nodes that saw the same entries must hold an identical
/// claim CRDT **and** name the same winner for every task — the property
/// M3's acceptance criterion calls "kimse 'ben kazandım' sanmıyor".
#[test]
fn i3_same_entries_different_arrival_order_derive_the_same_claims_and_winner() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let (p, q) = (NodeId(1), NodeId(2));
    let [e0, e1, e2] = a_chain(a, &keys[0]);

    let p_state = State::new(p, roster.clone(), keys[1].clone(), 64, 8, 0, 0);
    let p_state = deliver(&p_state, a, e0.clone(), 1);
    let p_state = deliver(&p_state, a, e1.clone(), 2);
    let p_state = deliver(&p_state, a, e2.clone(), 3);

    // Same three entries, reverse order: the first two buffer, then e0's
    // arrival drains everything in one step.
    let q_state = State::new(q, roster, keys[2].clone(), 64, 8, 0, 0);
    let q_state = deliver(&q_state, a, e2, 1);
    let q_state = deliver(&q_state, a, e1, 2);
    let q_state = deliver(&q_state, a, e0, 3);

    assert_eq!(
        p_state.claims(),
        q_state.claims(),
        "I3: the derived claim CRDT must not depend on arrival order"
    );

    // `a_chain` writes claims for tasks 0, 1 and 2, so all three are present
    // and each has exactly one claimant.
    let tasks: Vec<u64> = p_state.claims().tasks().collect();
    assert_eq!(tasks, [0, 1, 2], "the folded chain must be visible");
    for task in tasks {
        let winner = p_state.claims().winner(task);
        assert_eq!(winner, q_state.claims().winner(task), "winners disagree");
        assert_eq!(winner.map(|w| w.node), Some(a));
    }
}

// ---------------------------------------------------------------------------
// Documented placeholders — not testable at M2, deliberately not stubbed.
// Each names the milestone whose acceptance criterion activates it
// (docs/spec-m1.md §8):
//
//   I4 — spendable rights across all partitions <= authorised total.
//        Activates at M5 (escrow counter, 1000 seeds).
//   I5 — no safety-critical effect without a valid certificate in the log.
//        Activates with the policy gate (M5).
//   I6 — every effect is traceable to a signed entry chain.
//        Activates when step derives effects from entries generally (M5+;
//        M2 begins this for plain entries, but the full policy-gated claim
//        is later).
// ---------------------------------------------------------------------------
