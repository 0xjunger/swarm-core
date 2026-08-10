//! `LogBundle` round-trips and canonicity (`docs/spec.md` §20.2).

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use swarm_core::bundle::LogBundle;
use swarm_core::causal::VersionVector;
use swarm_core::codec::DecodeError;
use swarm_core::wire::{Body, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Envelope, Event, LogicalTime, NodeId, State};

fn key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    SigningKey::from_bytes(&bytes)
}

fn entry_at(node: NodeId, seq: u64, prev: Hash, k: &SigningKey) -> swarm_core::wire::Entry {
    UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node,
        seq,
        prev,
        deps: VersionVector::new(),
        body: Body::TaskClaim {
            task: seq,
            priority: 1,
        },
    }
    .sign(k)
}

fn sample_bundle() -> LogBundle {
    let a0 = entry_at(NodeId(0), 0, Hash::ZERO, &key(1));
    let a1 = entry_at(NodeId(0), 1, a0.chain_hash(), &key(1));
    let b0 = entry_at(NodeId(1), 0, Hash::ZERO, &key(2));

    let mut chains_observed_by_0 = BTreeMap::new();
    chains_observed_by_0.insert(NodeId(0), vec![a0.clone(), a1.clone()]);
    chains_observed_by_0.insert(NodeId(1), vec![b0.clone()]);

    let mut chains_observed_by_2 = BTreeMap::new();
    chains_observed_by_2.insert(NodeId(0), vec![a0]);

    let mut views = BTreeMap::new();
    views.insert(NodeId(0), chains_observed_by_0);
    views.insert(NodeId(2), chains_observed_by_2);

    LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views,
    }
}

#[test]
fn a_bundle_round_trips() {
    let bundle = sample_bundle();
    let decoded = LogBundle::decode(&bundle.encode()).expect("decodes");
    assert_eq!(decoded, bundle);
}

#[test]
fn an_empty_bundle_round_trips() {
    let bundle = LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views: BTreeMap::new(),
    };
    let decoded = LogBundle::decode(&bundle.encode()).expect("decodes");
    assert_eq!(decoded, bundle);
}

#[test]
fn a_real_run_exports_encodes_and_decodes_byte_identically() {
    // Two nodes, one entry exchanged, so `export_bundle` has something in
    // both `log` and `origins` to draw from.
    let mut keys = BTreeMap::new();
    keys.insert(NodeId(0), key(1).verifying_key());
    keys.insert(NodeId(1), key(2).verifying_key());
    let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

    let a = State::new(NodeId(0), roster.clone(), key(1), 64, 8, 10, 0);
    let (a1, fx) = step(&a, Event::Tick, LogicalTime(10));
    let swarm_core::Effect::Send { payload, .. } = fx[0].clone();
    let Envelope::Entry(a_entry) = payload else {
        panic!("expected an entry")
    };

    let b = State::new(NodeId(1), roster, key(2), 64, 8, 0, 0);
    let (b1, _) = step(
        &b,
        Event::Recv {
            from: NodeId(0),
            payload: Envelope::Entry(a_entry),
        },
        LogicalTime(11),
    );

    let bundle_a = a1.export_bundle();
    let bundle_b = b1.export_bundle();

    for bundle in [&bundle_a, &bundle_b] {
        let encoded = bundle.encode();
        let decoded = LogBundle::decode(&encoded).expect("decodes");
        assert_eq!(&decoded, bundle);
        assert_eq!(decoded.encode(), encoded, "re-encoding is byte-identical");
    }

    // A's export: A observed only its own chain (nothing received back).
    assert_eq!(bundle_a.views.len(), 1);
    assert_eq!(bundle_a.views[&NodeId(0)][&NodeId(0)].len(), 1);

    // B's export: B observed A's one entry, plus nothing of its own (its
    // entry_period is 0).
    assert_eq!(bundle_b.views.len(), 1);
    assert_eq!(bundle_b.views[&NodeId(1)][&NodeId(0)].len(), 1);
    assert!(!bundle_b.views[&NodeId(1)].contains_key(&NodeId(1)));
}

#[test]
fn merge_unions_two_single_observer_bundles() {
    let a0 = entry_at(NodeId(0), 0, Hash::ZERO, &key(1));

    let mut chains_0 = BTreeMap::new();
    chains_0.insert(NodeId(0), vec![a0.clone()]);
    let mut views_0 = BTreeMap::new();
    views_0.insert(NodeId(0), chains_0);
    let bundle_0 = LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views: views_0,
    };

    let mut chains_1 = BTreeMap::new();
    chains_1.insert(NodeId(0), vec![a0]);
    let mut views_1 = BTreeMap::new();
    views_1.insert(NodeId(1), chains_1);
    let bundle_1 = LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views: views_1,
    };

    let merged = bundle_0.merge(bundle_1);
    assert_eq!(merged.views.len(), 2);
    assert!(merged.views.contains_key(&NodeId(0)));
    assert!(merged.views.contains_key(&NodeId(1)));
}

#[test]
fn merge_keeps_the_longer_chain_for_the_same_observer_author_pair() {
    let a0 = entry_at(NodeId(0), 0, Hash::ZERO, &key(1));
    let a1 = entry_at(NodeId(0), 1, a0.chain_hash(), &key(1));

    let mut short = BTreeMap::new();
    short.insert(NodeId(0), vec![a0.clone()]);
    let mut views_short = BTreeMap::new();
    views_short.insert(NodeId(2), short);
    let bundle_short = LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views: views_short,
    };

    let mut long = BTreeMap::new();
    long.insert(NodeId(0), vec![a0, a1]);
    let mut views_long = BTreeMap::new();
    views_long.insert(NodeId(2), long);
    let bundle_long = LogBundle {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        views: views_long,
    };

    let merged = bundle_short.merge(bundle_long);
    assert_eq!(merged.views[&NodeId(2)][&NodeId(0)].len(), 2);
}

// ---------------------------------------------------------------------------
// Canonicity and format errors
// ---------------------------------------------------------------------------

#[test]
fn bad_domain_tag_is_rejected() {
    let mut bytes = sample_bundle().encode();
    bytes[0] ^= 0xFF;
    assert_eq!(LogBundle::decode(&bytes), Err(DecodeError::BadDomainTag));
}

#[test]
fn truncated_bundle_is_rejected() {
    let bytes = sample_bundle().encode();
    let short = &bytes[..bytes.len() - 3];
    assert_eq!(LogBundle::decode(short), Err(DecodeError::Truncated));
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut bytes = sample_bundle().encode();
    bytes.push(0xAB);
    assert_eq!(LogBundle::decode(&bytes), Err(DecodeError::TrailingBytes));
}

#[test]
fn observers_out_of_order_are_rejected() {
    // Hand-built: domain tag, mission_id, epoch, view_count=2, then
    // observer 2 followed by observer 1 — descending, not ascending.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SWARM_BUNDLE_V1");
    bytes.extend_from_slice(&PHASE1_MISSION_ID);
    bytes.extend_from_slice(&PHASE1_EPOCH.to_be_bytes());
    bytes.extend_from_slice(&2u16.to_be_bytes());
    // observer 2, zero chains
    bytes.push(2);
    bytes.extend_from_slice(&0u16.to_be_bytes());
    // observer 1, zero chains
    bytes.push(1);
    bytes.extend_from_slice(&0u16.to_be_bytes());
    assert_eq!(
        LogBundle::decode(&bytes),
        Err(DecodeError::NonCanonical("bundle_view_order"))
    );
}

#[test]
fn authors_out_of_order_within_a_view_are_rejected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SWARM_BUNDLE_V1");
    bytes.extend_from_slice(&PHASE1_MISSION_ID);
    bytes.extend_from_slice(&PHASE1_EPOCH.to_be_bytes());
    bytes.extend_from_slice(&1u16.to_be_bytes());
    // observer 0, two chains: author 2 then author 1 — descending.
    bytes.push(0);
    bytes.extend_from_slice(&2u16.to_be_bytes());
    bytes.push(2);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.push(1);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(
        LogBundle::decode(&bytes),
        Err(DecodeError::NonCanonical("bundle_chain_order"))
    );
}
