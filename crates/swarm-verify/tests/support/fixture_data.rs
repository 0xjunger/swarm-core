//! The six fixture scenarios (`docs/spec.md` §20.7, E7a): built once here,
//! shared by the regenerator (`examples/gen_fixtures.rs`) and the
//! consistency test (`tests/fixtures.rs`) via `#[path]` inclusion, so there
//! is exactly one definition of what each fixture means — never two
//! functions that could quietly drift apart.
//!
//! Deterministic throughout: fixed seeds, no time, no randomness — running
//! the regenerator twice produces byte-identical files, the same discipline
//! the golden vectors (`swarm-core/tests/golden_vector.rs`) follow.

#![allow(dead_code)]

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, SigningKey};
use swarm_core::bundle::{LogBundle, Spec};
use swarm_core::causal::VersionVector;
use swarm_core::wire::{Body, Entry, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;

pub fn key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    SigningKey::from_bytes(&bytes)
}

fn claim(node: NodeId, seq: u64, prev: Hash, task: u64, k: &SigningKey) -> Entry {
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

fn spend(node: NodeId, seq: u64, prev: Hash, amount: u64, k: &SigningKey) -> Entry {
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

fn roster_of(pairs: &[(NodeId, &SigningKey)]) -> Roster {
    let keys: BTreeMap<NodeId, _> = pairs.iter().map(|&(n, k)| (n, k.verifying_key())).collect();
    Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys)
}

fn view(chains: &[(NodeId, Vec<Entry>)]) -> BTreeMap<NodeId, Vec<Entry>> {
    chains.iter().cloned().collect()
}

fn bundle(views: &[(NodeId, BTreeMap<NodeId, Vec<Entry>>)]) -> LogBundle {
    LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views: views.iter().cloned().collect(),
    }
}

/// All four invariants `Satisfied`: two nodes, each converged on an
/// identical view of both chains.
pub fn clean() -> (LogBundle, Spec) {
    let (ka, kb) = (key(1), key(2));
    let (a, b) = (NodeId(0), NodeId(1));
    let a0 = claim(a, 0, Hash::ZERO, 0, &ka);
    let b0 = claim(b, 0, Hash::ZERO, 1, &kb);

    let converged = view(&[(a, vec![a0.clone()]), (b, vec![b0.clone()])]);
    let log_bundle = bundle(&[(a, converged.clone()), (b, converged)]);

    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: roster_of(&[(a, &ka), (b, &kb)]),
        budgets: BTreeMap::new(),
        log_cap: 1000,
    };
    (log_bundle, spec)
}

/// I1 `Violated(Equivocation)`: node F signs two different genesis entries;
/// two observers each hold one of the two.
pub fn equivocation() -> (LogBundle, Spec) {
    let (kg, kh, kf) = (key(10), key(11), key(12));
    let (g, h, f) = (NodeId(0), NodeId(1), NodeId(2));
    let genuine = claim(f, 0, Hash::ZERO, 1, &kf);
    let forged = claim(f, 0, Hash::ZERO, 2, &kf);
    assert_ne!(genuine.encoded(), forged.encoded());

    let log_bundle = bundle(&[
        (g, view(&[(f, vec![genuine])])),
        (h, view(&[(f, vec![forged])])),
    ]);

    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: roster_of(&[(g, &kg), (h, &kh), (f, &kf)]),
        budgets: BTreeMap::new(),
        log_cap: 1000,
    };
    (log_bundle, spec)
}

/// I4 `Violated(Overspend)`: one node spends 20 against a budget of 3.
pub fn overspend() -> (LogBundle, Spec) {
    let ka = key(20);
    let a = NodeId(0);
    let s0 = spend(a, 0, Hash::ZERO, 10, &ka);
    let s1 = spend(a, 1, s0.chain_hash(), 10, &ka);

    let log_bundle = bundle(&[(a, view(&[(a, vec![s0, s1])]))]);

    let mut budgets = BTreeMap::new();
    budgets.insert(a, 3);
    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: roster_of(&[(a, &ka)]),
        budgets,
        log_cap: 1000,
    };
    (log_bundle, spec)
}

/// A chain-verification failure: a tampered signature. Reported in
/// `Verdict::chains`, not as any of I1-I4.
pub fn broken_chain() -> (LogBundle, Spec) {
    let ka = key(30);
    let a = NodeId(0);
    let mut tampered = claim(a, 0, Hash::ZERO, 0, &ka);
    let mut sig_bytes = tampered.sig.to_bytes();
    sig_bytes[0] ^= 1;
    tampered.sig = Signature::from_bytes(&sig_bytes);

    let log_bundle = bundle(&[(a, view(&[(a, vec![tampered])]))]);

    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: roster_of(&[(a, &ka)]),
        budgets: BTreeMap::new(),
        log_cap: 1000,
    };
    (log_bundle, spec)
}

/// A chain filed under an author key its entries do not claim: node F signs
/// two distinct genesis entries; observer G holds one filed correctly under
/// F, observer H holds the other filed under a different node, D, entirely.
/// Every signature verifies, `seq` is contiguous from zero, and the roster
/// contains the signer — the only thing wrong is the key the second chain
/// was filed under. `Verdict::chains` must report this as `Misfiled`, not
/// silently drop it (`docs/spec.md` §20.5).
pub fn misfiled_chain() -> (LogBundle, Spec) {
    let (kg, kh, kf, kd) = (key(30), key(31), key(32), key(33));
    let (g, h, f, d) = (NodeId(0), NodeId(1), NodeId(2), NodeId(3));
    let genuine = claim(f, 0, Hash::ZERO, 1, &kf);
    let forged = claim(f, 0, Hash::ZERO, 2, &kf);
    assert_ne!(genuine.encoded(), forged.encoded());

    let log_bundle = bundle(&[
        (g, view(&[(f, vec![genuine])])),
        (h, view(&[(d, vec![forged])])),
    ]);

    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: roster_of(&[(g, &kg), (h, &kh), (f, &kf), (d, &kd)]),
        budgets: BTreeMap::new(),
        log_cap: 1000,
    };
    (log_bundle, spec)
}

/// A node whose log never arrived: exactly one observer's view is present.
/// I3 needs two observers to compare and reports `Undetermined`, not a
/// violation and not a vacuous `Satisfied` — silence is ambiguous
/// (`docs/spec.md` §20.2).
pub fn missing_node() -> (LogBundle, Spec) {
    let (ka, kb) = (key(40), key(41));
    let (a, b) = (NodeId(0), NodeId(1));
    let a0 = claim(a, 0, Hash::ZERO, 0, &ka);

    // Only A's own view is in the bundle — B's log never arrived.
    let log_bundle = bundle(&[(a, view(&[(a, vec![a0])]))]);

    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: roster_of(&[(a, &ka), (b, &kb)]),
        budgets: BTreeMap::new(),
        log_cap: 1000,
    };
    (log_bundle, spec)
}

/// Not a valid `LogBundle` at all: a clean bundle's bytes, cut short.
/// Decoding it must fail with `DecodeError::Truncated` before `verify` is
/// ever reached. Paired with `clean()`'s `Spec`, which is never used, since
/// the bundle never decodes.
pub fn truncated_bytes() -> Vec<u8> {
    let (log_bundle, _) = clean();
    let full = log_bundle.encode();
    full[..full.len() - 5].to_vec()
}

pub fn truncated_spec() -> Spec {
    clean().1
}
