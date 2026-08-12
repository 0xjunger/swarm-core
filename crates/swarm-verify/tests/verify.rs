//! `verify(bundle, spec)` (`SPEC.md` §7): hand-built bundles
//! exercising each invariant, both directions, plus `Verdict::chains`.
//!
//! `crates/swarm-sim/tests/m7_equivalence.rs` covers the 5000-seed
//! agreement with the oracle on real simulation output; this file covers
//! the individual mechanisms with fixtures small enough to read in one
//! sitting.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use swarm_core::bundle::{LogBundle, Spec};
use swarm_core::causal::VersionVector;
use swarm_core::log::ChainError;
use swarm_core::wire::{Body, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;
use swarm_verify::verdict::{ChainProblem, InvariantResult, Witness};
use swarm_verify::verify;

fn key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    SigningKey::from_bytes(&bytes)
}

fn roster3() -> (Roster, [SigningKey; 3]) {
    let keys = [key(1), key(2), key(3)];
    let mut m = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        m.insert(NodeId(i as u8), k.verifying_key());
    }
    (Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, m), keys)
}

fn spec_of(roster: &Roster, budgets: BTreeMap<NodeId, u64>, log_cap: u32) -> Spec {
    Spec {
        mission_id: roster.mission_id,
        epoch: roster.epoch,
        roster: roster.clone(),
        budgets,
        log_cap,
    }
}

fn claim_at(
    node: NodeId,
    seq: u64,
    prev: Hash,
    task: u64,
    k: &SigningKey,
) -> swarm_core::wire::Entry {
    UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node,
        seq,
        prev,
        deps: VersionVector::new(),
        body: Body::TaskClaim { task, priority: 1 },
    }
    .sign(k)
}

fn spend_at(
    node: NodeId,
    seq: u64,
    prev: Hash,
    amount: u64,
    k: &SigningKey,
) -> swarm_core::wire::Entry {
    UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node,
        seq,
        prev,
        deps: VersionVector::new(),
        body: Body::Spend { amount },
    }
    .sign(k)
}

fn single_view_bundle(
    observer: NodeId,
    chains: BTreeMap<NodeId, Vec<swarm_core::wire::Entry>>,
) -> LogBundle {
    let mut views = BTreeMap::new();
    views.insert(observer, chains);
    LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views,
    }
}

// ---------------------------------------------------------------------------
// A clean bundle
// ---------------------------------------------------------------------------

#[test]
fn a_clean_single_observer_bundle_is_satisfied_on_i1_and_i4_and_undetermined_on_i3() {
    let (roster, keys) = roster3();
    let a0 = claim_at(NodeId(0), 0, Hash::ZERO, 1, &keys[0]);

    let mut chains = BTreeMap::new();
    chains.insert(NodeId(0), vec![a0]);
    let bundle = single_view_bundle(NodeId(0), chains);
    let spec = spec_of(&roster, BTreeMap::new(), 100);

    let verdict = verify(&bundle, &spec);
    assert!(verdict.chains.is_empty());
    assert_eq!(verdict.i1, InvariantResult::Satisfied);
    assert_eq!(verdict.i2, InvariantResult::Satisfied);
    assert_eq!(
        verdict.i3,
        InvariantResult::Undetermined("fewer than two observers to compare")
    );
    assert_eq!(verdict.i4, InvariantResult::Satisfied);
    assert!(!verdict.input_attestable);
    assert!(!verdict.any_violated());
    assert!(
        !verdict.all_satisfied(),
        "I3 undetermined must not read as all-satisfied"
    );
}

// ---------------------------------------------------------------------------
// I1: equivocation
// ---------------------------------------------------------------------------

#[test]
fn two_observers_holding_conflicting_entries_at_the_same_seq_trigger_i1() {
    let (roster, keys) = roster3();
    let genuine = claim_at(NodeId(0), 0, Hash::ZERO, 1, &keys[0]);
    let forged = claim_at(NodeId(0), 0, Hash::ZERO, 2, &keys[0]);
    assert_ne!(genuine.encoded(), forged.encoded());

    let mut chains_g = BTreeMap::new();
    chains_g.insert(NodeId(0), vec![genuine]);
    let mut chains_f = BTreeMap::new();
    chains_f.insert(NodeId(0), vec![forged]);

    let bundle =
        single_view_bundle(NodeId(1), chains_g).merge(single_view_bundle(NodeId(2), chains_f));
    let spec = spec_of(&roster, BTreeMap::new(), 100);

    let verdict = verify(&bundle, &spec);
    match &verdict.i1 {
        InvariantResult::Violated(w) => match w.as_ref() {
            Witness::Equivocation(poe) => assert_eq!(poe.node(), NodeId(0)),
            other => panic!("expected Equivocation, got {other:?}"),
        },
        other => panic!("expected Violated, got {other:?}"),
    }
    assert!(verdict.any_violated());
}

// ---------------------------------------------------------------------------
// I2: unmet dependency
// ---------------------------------------------------------------------------

#[test]
fn an_entry_whose_dependency_the_observer_never_holds_triggers_i2() {
    let (roster, keys) = roster3();
    let mut deps = VersionVector::new();
    deps.bump(NodeId(0), 0); // never present anywhere in this bundle
    let b0 = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: NodeId(1),
        seq: 0,
        prev: Hash::ZERO,
        deps,
        body: Body::TaskClaim {
            task: 1,
            priority: 1,
        },
    }
    .sign(&keys[1]);

    let mut chains = BTreeMap::new();
    chains.insert(NodeId(1), vec![b0]);
    let bundle = single_view_bundle(NodeId(1), chains);
    let spec = spec_of(&roster, BTreeMap::new(), 100);

    let verdict = verify(&bundle, &spec);
    match &verdict.i2 {
        InvariantResult::Violated(w) => match w.as_ref() {
            Witness::UnmetDependency {
                observer, missing, ..
            } => {
                assert_eq!(*observer, NodeId(1));
                assert_eq!(*missing, (NodeId(0), 0));
            }
            other => panic!("expected UnmetDependency, got {other:?}"),
        },
        other => panic!("expected Violated, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// I3: divergence
// ---------------------------------------------------------------------------

#[test]
fn two_observers_of_the_same_key_set_with_different_content_diverge_on_i3() {
    // A claims task 0 at priority 1. Two different, differently-signed
    // copies of B's claim exist at the same (node=1, seq=0) — an
    // equivocation — one at priority 1 (loses to A on node-id tie-break),
    // one at priority 0 (beats A outright). Two observers each hold A's
    // claim plus one of the two copies of B's: identical `(author, seq)`
    // key sets — `{(0,0), (1,0)}` at both — but different derived winners.
    let (roster, keys) = roster3();
    let a0 = claim_at(NodeId(0), 0, Hash::ZERO, 0, &keys[0]);
    let b0_loses = claim_at(NodeId(1), 0, Hash::ZERO, 0, &keys[1]);
    let b0_wins = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: NodeId(1),
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: Body::TaskClaim {
            task: 0,
            priority: 0, // beats A's priority 1
        },
    }
    .sign(&keys[1]);
    assert_ne!(b0_loses.encoded(), b0_wins.encoded());

    let mut chains_1 = BTreeMap::new();
    chains_1.insert(NodeId(0), vec![a0.clone()]);
    chains_1.insert(NodeId(1), vec![b0_loses]);

    let mut chains_2 = BTreeMap::new();
    chains_2.insert(NodeId(0), vec![a0]);
    chains_2.insert(NodeId(1), vec![b0_wins]);

    let bundle =
        single_view_bundle(NodeId(0), chains_1).merge(single_view_bundle(NodeId(1), chains_2));
    let spec = spec_of(&roster, BTreeMap::new(), 100);

    let verdict = verify(&bundle, &spec);
    match &verdict.i3 {
        InvariantResult::Violated(w) => match w.as_ref() {
            Witness::Divergence { task, .. } => assert_eq!(*task, 0),
            other => panic!("expected Divergence, got {other:?}"),
        },
        other => panic!("expected Violated (key sets match, winners differ), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// I4: overspend
// ---------------------------------------------------------------------------

#[test]
fn spending_beyond_the_spec_budget_triggers_i4() {
    let (roster, keys) = roster3();
    let s0 = spend_at(NodeId(0), 0, Hash::ZERO, 10, &keys[0]);
    let s1 = spend_at(NodeId(0), 1, s0.chain_hash(), 10, &keys[0]);

    let mut chains = BTreeMap::new();
    chains.insert(NodeId(0), vec![s0, s1]);
    let bundle = single_view_bundle(NodeId(0), chains);

    let mut budgets = BTreeMap::new();
    budgets.insert(NodeId(0), 3);
    let spec = spec_of(&roster, budgets, 100);

    let verdict = verify(&bundle, &spec);
    match &verdict.i4 {
        InvariantResult::Violated(w) => match w.as_ref() {
            Witness::Overspend {
                node,
                budget,
                entries,
            } => {
                assert_eq!(*node, NodeId(0));
                assert_eq!(*budget, 3);
                assert_eq!(entries.len(), 2);
            }
            other => panic!("expected Overspend, got {other:?}"),
        },
        other => panic!("expected Violated, got {other:?}"),
    }
}

#[test]
fn the_same_bundle_is_clean_against_a_spec_with_a_high_enough_budget() {
    let (roster, keys) = roster3();
    let s0 = spend_at(NodeId(0), 0, Hash::ZERO, 10, &keys[0]);
    let s1 = spend_at(NodeId(0), 1, s0.chain_hash(), 10, &keys[0]);

    let mut chains = BTreeMap::new();
    chains.insert(NodeId(0), vec![s0, s1]);
    let bundle = single_view_bundle(NodeId(0), chains);

    // Independence proof (`SPEC.md` §4.5): identical bundle, lowered
    // vs. sufficient budget, opposite verdicts.
    let mut low_budgets = BTreeMap::new();
    low_budgets.insert(NodeId(0), 3);
    let low_spec = spec_of(&roster, low_budgets, 100);
    assert!(matches!(
        verify(&bundle, &low_spec).i4,
        InvariantResult::Violated(_)
    ));

    let mut high_budgets = BTreeMap::new();
    high_budgets.insert(NodeId(0), 20);
    let high_spec = spec_of(&roster, high_budgets, 100);
    assert_eq!(verify(&bundle, &high_spec).i4, InvariantResult::Satisfied);
}

// ---------------------------------------------------------------------------
// Chain findings — outside I1-I4
// ---------------------------------------------------------------------------

#[test]
fn a_chain_with_a_broken_signature_is_reported_as_a_chain_finding_not_an_invariant() {
    let (roster, keys) = roster3();
    let mut bad = claim_at(NodeId(0), 0, Hash::ZERO, 1, &keys[0]);
    let mut sig = bad.sig.to_bytes();
    sig[0] ^= 1;
    bad.sig = ed25519_dalek::Signature::from_bytes(&sig);

    let mut chains = BTreeMap::new();
    chains.insert(NodeId(0), vec![bad]);
    let bundle = single_view_bundle(NodeId(0), chains);
    let spec = spec_of(&roster, BTreeMap::new(), 100);

    let verdict = verify(&bundle, &spec);
    assert_eq!(verdict.chains.len(), 1);
    assert_eq!(verdict.chains[0].observer, NodeId(0));
    assert_eq!(verdict.chains[0].author, NodeId(0));
    assert_eq!(
        verdict.chains[0].error,
        ChainProblem::Chain(ChainError::BadSignature { index: 0 })
    );
    // The broken chain contributed no evidence to any invariant.
    assert_eq!(
        verdict.i1,
        InvariantResult::Undetermined("no chain-verified entries in the bundle")
    );
}

#[test]
fn a_chain_longer_than_log_cap_is_a_chain_finding() {
    let (roster, keys) = roster3();
    let a0 = claim_at(NodeId(0), 0, Hash::ZERO, 1, &keys[0]);
    let a1 = claim_at(NodeId(0), 1, a0.chain_hash(), 2, &keys[0]);

    let mut chains = BTreeMap::new();
    chains.insert(NodeId(0), vec![a0, a1]);
    let bundle = single_view_bundle(NodeId(0), chains);
    let spec = spec_of(&roster, BTreeMap::new(), 1);

    let verdict = verify(&bundle, &spec);
    assert_eq!(verdict.chains.len(), 1);
    assert_eq!(
        verdict.chains[0].error,
        ChainProblem::TooLong { cap: 1, actual: 2 }
    );
}

// ---------------------------------------------------------------------------
// Undetermined on an empty bundle
// ---------------------------------------------------------------------------

#[test]
fn an_empty_bundle_is_undetermined_on_every_invariant() {
    let (roster, _keys) = roster3();
    let bundle = LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views: BTreeMap::new(),
    };
    let spec = spec_of(&roster, BTreeMap::new(), 100);

    let verdict = verify(&bundle, &spec);
    assert!(verdict.chains.is_empty());
    assert!(matches!(verdict.i1, InvariantResult::Undetermined(_)));
    assert!(matches!(verdict.i2, InvariantResult::Undetermined(_)));
    assert!(matches!(verdict.i3, InvariantResult::Undetermined(_)));
    assert!(matches!(verdict.i4, InvariantResult::Undetermined(_)));
    assert!(!verdict.any_violated());
    assert!(!verdict.all_satisfied());
}
