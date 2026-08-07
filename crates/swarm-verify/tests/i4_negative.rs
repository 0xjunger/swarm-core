//! I4's negative control: a real overspend must be reported.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use swarm_core::causal::VersionVector;
use swarm_core::wire::{Body, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Envelope, Event, LogicalTime, NodeId, State};
use swarm_verify::check_invariants;

fn key(seed: u8) -> SigningKey {
    let mut b = [0u8; 32];
    b[0] = seed;
    SigningKey::from_bytes(&b)
}

/// Node A is allocated 3 units but authors signed Spend entries totalling 40.
/// A checker that does not report this is not checking anything.
#[test]
fn a_flagrant_overspend_is_reported() {
    let a = NodeId(0);
    let obs = NodeId(1);
    let (ka, kb) = (key(1), key(2));

    let mut keys = BTreeMap::new();
    keys.insert(a, ka.verifying_key());
    keys.insert(obs, kb.verifying_key());
    let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

    let mut budgets = BTreeMap::new();
    budgets.insert(a, 3);
    budgets.insert(obs, 3);

    let mut s = State::new(obs, roster, kb, 64, 8, 0, 0).with_budgets(budgets.clone());

    let mut prev = Hash::ZERO;
    for seq in 0..4u64 {
        let deps = {
            let mut vv = VersionVector::new();
            if seq > 0 {
                vv.bump(a, seq - 1);
            }
            vv
        };
        let e = UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node: a,
            seq,
            prev,
            deps,
            body: Body::Spend { amount: 10 },
        }
        .sign(&ka);
        prev = e.chain_hash();
        let (next, _) = step(
            &s,
            Event::Recv { from: a, payload: Envelope::Entry(e) },
            LogicalTime(seq + 1),
        );
        s = next;
    }

    let mut states = BTreeMap::new();
    states.insert(obs, s);

    let violations = check_invariants(&states, &budgets);
    assert!(
        violations.iter().any(|v| v.invariant == "I4"),
        "spent 40 against a budget of 3 and the checker said: {violations:?}"
    );
}
