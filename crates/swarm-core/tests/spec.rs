//! `Spec` round-trips and canonicity (`docs/spec.md` §20.3).
//!
//! The independence proof — the same bundle verifies clean against its real
//! `Spec` and reports an I4 violation against one with a lowered budget —
//! needs `swarm-verify::verify`, which this crate does not depend on; that
//! test lives in `swarm-verify`'s own suite (`docs/spec.md` §20.5), not here.

use std::collections::BTreeMap;

use ed25519_dalek::{SigningKey, VerifyingKey};
use swarm_core::bundle::Spec;
use swarm_core::codec::DecodeError;
use swarm_core::wire::{Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;

fn key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    SigningKey::from_bytes(&bytes)
}

fn sample_spec() -> Spec {
    let mut keys = BTreeMap::new();
    keys.insert(NodeId(0), key(1).verifying_key());
    keys.insert(NodeId(1), key(2).verifying_key());
    keys.insert(NodeId(2), key(3).verifying_key());
    let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

    let mut budgets = BTreeMap::new();
    budgets.insert(NodeId(0), 3);
    budgets.insert(NodeId(1), 5);
    budgets.insert(NodeId(2), 0);

    Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster,
        budgets,
        log_cap: 1000,
    }
}

#[test]
fn a_spec_round_trips() {
    let spec = sample_spec();
    let decoded = Spec::decode(&spec.encode()).expect("decodes");
    assert_eq!(decoded, spec);
}

#[test]
fn an_empty_spec_round_trips() {
    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, BTreeMap::new()),
        budgets: BTreeMap::new(),
        log_cap: 0,
    };
    let decoded = Spec::decode(&spec.encode()).expect("decodes");
    assert_eq!(decoded, spec);
}

#[test]
fn re_encoding_a_decoded_spec_is_byte_identical() {
    let spec = sample_spec();
    let encoded = spec.encode();
    let decoded = Spec::decode(&encoded).unwrap();
    assert_eq!(decoded.encode(), encoded);
}

#[test]
fn bad_domain_tag_is_rejected() {
    let mut bytes = sample_spec().encode();
    bytes[0] ^= 0xFF;
    assert_eq!(Spec::decode(&bytes), Err(DecodeError::BadDomainTag));
}

#[test]
fn truncated_spec_is_rejected() {
    let bytes = sample_spec().encode();
    let short = &bytes[..bytes.len() - 3];
    assert_eq!(Spec::decode(short), Err(DecodeError::Truncated));
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut bytes = sample_spec().encode();
    bytes.push(0xAB);
    assert_eq!(Spec::decode(&bytes), Err(DecodeError::TrailingBytes));
}

#[test]
fn roster_out_of_order_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SWARM_SPEC_V1");
    bytes.extend_from_slice(&PHASE1_MISSION_ID);
    bytes.extend_from_slice(&PHASE1_EPOCH.to_be_bytes());
    // roster_count = 2, node 1 then node 0 — descending.
    bytes.extend_from_slice(&2u16.to_be_bytes());
    bytes.push(1);
    bytes.extend_from_slice(key(2).verifying_key().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(key(1).verifying_key().as_bytes());
    // budgets: none. log_cap: 0.
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(
        Spec::decode(&bytes),
        Err(DecodeError::NonCanonical("spec_roster_order"))
    );
}

#[test]
fn budgets_out_of_order_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SWARM_SPEC_V1");
    bytes.extend_from_slice(&PHASE1_MISSION_ID);
    bytes.extend_from_slice(&PHASE1_EPOCH.to_be_bytes());
    // empty roster
    bytes.extend_from_slice(&0u16.to_be_bytes());
    // budgets: node 1 then node 0 — descending.
    bytes.extend_from_slice(&2u16.to_be_bytes());
    bytes.push(1);
    bytes.extend_from_slice(&3u64.to_be_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&5u64.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(
        Spec::decode(&bytes),
        Err(DecodeError::NonCanonical("spec_budget_order"))
    );
}

/// Not every 32-byte string is a valid compressed Edwards point — only
/// about half of the possible `y` values pair with an `x` on the curve.
/// Rather than hand-computing a known-bad point, search a small
/// deterministic space for one `ed25519_dalek::VerifyingKey::from_bytes`
/// itself rejects, so the test's premise ("these bytes are not a point") is
/// verified the same way `Spec::decode` verifies it.
fn non_canonical_point_bytes() -> [u8; 32] {
    for b in 0u16..256 {
        let mut candidate = [0xFFu8; 32];
        candidate[0] = b as u8;
        if VerifyingKey::from_bytes(&candidate).is_err() {
            return candidate;
        }
    }
    panic!("no non-canonical point found in the search space");
}

#[test]
fn a_non_canonical_verifying_key_is_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SWARM_SPEC_V1");
    bytes.extend_from_slice(&PHASE1_MISSION_ID);
    bytes.extend_from_slice(&PHASE1_EPOCH.to_be_bytes());
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&non_canonical_point_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(Spec::decode(&bytes), Err(DecodeError::BadVerifyingKey));
}
