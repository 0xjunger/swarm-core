//! Invariant tests — written before the code that satisfies them, per
//! `DESIGN.md` §11.7 ("Sıralamayı ters kur: önce invariant, sonra kod").
//!
//! At M1 only I1 was testable: one node, no network, no CRDT, no escrow.
//! M2 activates I2 and I3, M4 activates PoE verification for I1's cross-node
//! case, M5 activates I4. M6 activates I5 and I6 through the policy gate.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use swarm_core::causal::VersionVector;
use swarm_core::fault::verify_poe;
use swarm_core::log::{verify_chain, ChainError, Log};
use swarm_core::wire::{Body, Entry, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Effect, Envelope, Event, LogicalTime, NodeId, State};

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

/// Adversarial side: a node that signs two different entries at the same
/// `(node, seq)` never gets the second one applied, and the receiver ends up
/// holding a proof a third party can verify unilaterally
/// (`docs/spec.md` §11, M4). This is I1 held against a peer that lies, not
/// just against an honest builder's bug — the two tests above cover
/// construction and single-chain verification; this one covers what a
/// receiver does when a roster member equivocates.
#[test]
fn i1_a_second_signed_entry_at_a_taken_seq_is_never_applied_and_is_proven() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let observer = NodeId(2);

    let first = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: a,
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: claim(0),
    }
    .sign(&keys[0]);
    let second = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: a,
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: claim(1),
    }
    .sign(&keys[0]);

    let s = State::new(observer, roster.clone(), keys[2].clone(), 64, 8, 0, 0);
    let s = deliver(&s, a, first.clone(), 1);
    assert_eq!(s.causal_vv().highest(a), Some(0));
    assert!(s.poes().next().is_none(), "nothing conflicting seen yet");

    let s = deliver(&s, a, second, 2);
    // I1: the second entry never displaces the first.
    assert_eq!(
        s.entries(),
        [&first],
        "I1 violated: a conflicting entry at an already-applied seq was accepted"
    );

    let poe = s.poes().next().expect("equivocation must be detected");
    assert_eq!(poe.node(), a);
    assert_eq!(poe.seq(), 0);
    assert!(
        verify_poe(&roster, poe).is_ok(),
        "a third party holding only the roster must verify the proof unilaterally"
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
/// (`docs/spec.md` §9.2): each entry's `deps` names only its author's own
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

/// Minimal restatement of I2 as an invariant test: `docs/spec.md` §9.3's
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

/// I3 at M3 (`docs/spec.md` §13): "derived state" now means more than the
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
// I4 — spendable rights across all partitions <= authorised total
// ---------------------------------------------------------------------------

fn tick_at(state: &State, at: u64) -> State {
    let (next, _) = step(state, Event::Tick, LogicalTime(at));
    next
}

/// A node with no budgets configured never issues a Spend entry.
#[test]
fn i4_no_budget_means_no_spending() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let s = State::new(a, roster, keys[0].clone(), 64, 8, 10, 0);
    let s = tick_at(&s, 10);
    let spend_count = s
        .entries()
        .iter()
        .filter(|e| matches!(e.body, Body::Spend { .. }))
        .count();
    assert_eq!(spend_count, 0, "no budget => no Spend entries");
}

/// A node with budget spends exactly 1 per authoring tick until exhausted.
#[test]
fn i4_spends_one_per_period_until_exhausted() {
    // entry_period = 5 lets us tick at 5, 10, 15, 20.
    let key = test_key(1);
    let budgets = {
        let mut m = BTreeMap::new();
        m.insert(NodeId(0), 3);
        m
    };
    let s =
        State::new(NodeId(0), roster_of(NodeId(0), &key), key, 64, 8, 5, 0).with_budgets(budgets);

    let s = tick_at(&s, 5);
    let s = tick_at(&s, 10);
    let s = tick_at(&s, 15);
    // After 3 authoring ticks (at 5, 10, 15), budget of 3 is exhausted.
    assert_eq!(s.escrow().remaining(NodeId(0)), 0);

    let s = tick_at(&s, 20);
    // Tick 20 — should NOT increase spend count because budget is zero.
    let spend_count = s
        .entries()
        .iter()
        .filter(|e| matches!(e.body, Body::Spend { .. }))
        .count();
    assert_eq!(spend_count, 3, "budget exhausted, no more Spend");
    assert_eq!(s.escrow().remaining(NodeId(0)), 0);
}

/// The escrow counter sees Spend entries from other nodes and subtracts them.
#[test]
fn i4_escrow_tracks_other_nodes_spending() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let observer = NodeId(2);
    let budgets = {
        let mut b = BTreeMap::new();
        b.insert(a, 3);
        b
    };
    let s = State::new(observer, roster, keys[2].clone(), 64, 8, 0, 0).with_budgets(budgets);

    let e = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: a,
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: Body::Spend { amount: 2 },
    }
    .sign(&keys[0]);

    let s = deliver(&s, a, e, 1);
    assert_eq!(s.escrow().remaining(a), 1);
}

/// I4: total unique Spend entries across all nodes must not exceed total budget.
#[test]
fn i4_total_spend_never_exceeds_total_allocation() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let bb = NodeId(1);
    let observer = NodeId(2);
    let budgets = {
        let mut m = BTreeMap::new();
        m.insert(a, 2);
        m.insert(bb, 1);
        m
    };
    let s = State::new(observer, roster, keys[2].clone(), 64, 8, 0, 0).with_budgets(budgets);

    let ea = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: a,
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: Body::Spend { amount: 2 },
    }
    .sign(&keys[0]);
    let eb = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: bb,
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: Body::Spend { amount: 1 },
    }
    .sign(&keys[1]);

    let s = deliver(&s, a, ea, 1);
    let s = deliver(&s, bb, eb, 2);

    let total_spent: u64 = s
        .entries()
        .iter()
        .filter_map(|e| match e.body {
            Body::Spend { amount } => Some(amount),
            _ => None,
        })
        .sum();
    assert_eq!(total_spent, 3);
    // Round tripped through entries and escrow independently.
    assert_eq!(s.escrow().remaining(a), 0);
    assert_eq!(s.escrow().remaining(bb), 0);
}

// ---------------------------------------------------------------------------
// I5 — no safety-critical effect without a valid certificate in the log
// ---------------------------------------------------------------------------
//
// Structurally discharged, not executable-checked (docs/spec.md §15):
// `Class::SafetyCritical` actions have a non-`()` `Cert` type, and in
// Phase 1 `SafetyCriticalAction` does not implement `Action` at all, so no
// safety-critical effect can be created — not even through a bug, because
// the compiler rejects it. That claim is proven by the `compile_fail`
// doctest on `swarm_core::policy::SafetyCriticalAction`, not by an
// assertion here — a runtime test cannot demonstrate that something fails
// to *compile*.
//
// The test below only verifies the part of I5 that *is* runtime behavior:
// the gate exists and is wired in, and a Degradable action passes it.

use swarm_core::policy;

/// The policy gate exists and is wired. A Degradable action passes `commit`
/// unconditionally. The other half of I5 — that a `SafetyCritical` action
/// cannot even be passed to `commit` — is a compile-time property, proven by
/// the `compile_fail` doctest on `policy::SafetyCriticalAction`.
#[test]
fn i5_policy_gate_passes_degradable_actions() {
    // Degradable actions with Cert = () pass the gate.
    let action = policy::TaskClaim {
        task: 0,
        priority: 1,
    };
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let s = State::new(a, roster, keys[0].clone(), 64, 8, 0, 0);
    let result = policy::commit(&s, &action, &());
    assert!(result, "Degradable actions must pass the policy gate");
}

// ---------------------------------------------------------------------------
// I6 — every effect is traceable to a signed entry chain
// ---------------------------------------------------------------------------
//
// `policy::author_and_commit` is the ONLY function that produces effects.
// It always appends to the log before `commit` gates the effects — so every
// effect has a corresponding signed entry. The test below verifies this
// property: for a node run through several ticks, every effect that was
// produced has a matching entry in the node's log.

/// I6: at each authoring tick, every `Effect::Send { payload:
/// Envelope::Entry(..) }` carries an entry that exists in the node's log.
/// Anti-entropy effects carry `Envelope::AntiEntropy`, not `Entry`, and are
/// traced to the version vector — not relevant here.
#[test]
fn i6_effects_trace_back_to_log_entries() {
    let (roster, keys) = m2_roster();
    let a = NodeId(0);
    let s = State::new(a, roster, keys[0].clone(), 64, 8, 10, 0);

    // Tick 10: a claim + possibly a spend (no budget => no spend).
    let (s1, fx1) = step(&s, Event::Tick, LogicalTime(10));
    assert_eq!(s1.log().len(), 1, "one entry authored at tick 10");
    for e in &fx1 {
        if let Effect::Send {
            payload: Envelope::Entry(entry),
            ..
        } = e
        {
            let found = s1
                .log()
                .entries()
                .iter()
                .any(|le| le.chain_hash() == entry.chain_hash());
            assert!(found, "I6: tick 10 effect carries entry not in the log");
        }
    }
    // At least one Entry effect was produced (the claim broadcast).
    assert!(fx1.iter().any(|e| matches!(
        e,
        Effect::Send {
            payload: Envelope::Entry(_),
            ..
        }
    )));

    // Tick 20: a claim for task 1. No withdrawal because node 0 still wins task 0
    // (sole claimant). The claim entry is broadcast.
    let (s2, fx2) = step(&s1, Event::Tick, LogicalTime(20));
    assert_eq!(s2.log().len(), 2, "two entries by tick 20");
    for e in &fx2 {
        if let Effect::Send {
            payload: Envelope::Entry(entry),
            ..
        } = e
        {
            let found = s2
                .log()
                .entries()
                .iter()
                .any(|le| le.chain_hash() == entry.chain_hash());
            assert!(found, "I6: tick 20 effect carries entry not in the log");
        }
    }
    assert!(fx2.iter().any(|e| matches!(
        e,
        Effect::Send {
            payload: Envelope::Entry(_),
            ..
        }
    )));
}
