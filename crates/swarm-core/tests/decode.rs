//! `decode_entry` is the exact inverse of `Entry::encoded()`
//! (`docs/spec.md` §20.1): `decode(encode(x)) == x` for every `Entry`, or
//! the format is lossy and nothing built on top of it can be trusted.

use ed25519_dalek::SigningKey;
use proptest::prelude::*;

use swarm_core::causal::VersionVector;
use swarm_core::codec::decode_entry_exact;
use swarm_core::wire::{Body, Hash, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;

/// A single fixed signing key. The round-trip property is about the wire
/// format, not about signature validity against any roster — decoding does
/// not check a signature against a key, only that the bytes have the right
/// shape — so every case reuses one key rather than asking proptest to
/// generate 5000 of them.
fn key() -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = 0x42;
    SigningKey::from_bytes(&bytes)
}

fn body_strategy() -> impl Strategy<Value = Body> {
    prop_oneof![
        (any::<u64>(), any::<u8>())
            .prop_map(|(task, priority)| Body::TaskClaim { task, priority }),
        any::<u64>().prop_map(|task| Body::Withdraw { task }),
        any::<u64>().prop_map(|amount| Body::Spend { amount }),
    ]
}

/// A `BTreeMap<u8, u64>` decays into a `VersionVector` with strictly
/// ascending, unique `NodeId`s for free — the same canonicity the decoder
/// requires (`docs/spec.md` §20.1) — because the map already dedups and
/// orders its keys.
fn deps_strategy() -> impl Strategy<Value = VersionVector> {
    prop::collection::btree_map(any::<u8>(), any::<u64>(), 0..6).prop_map(|m| {
        let mut vv = VersionVector::new();
        for (node, seq) in m {
            vv.bump(NodeId(node), seq);
        }
        vv
    })
}

fn entry_strategy() -> impl Strategy<Value = swarm_core::wire::Entry> {
    (
        prop::array::uniform32(any::<u8>()),
        any::<u8>(),
        any::<u64>(),
        prop::array::uniform32(any::<u8>()),
        deps_strategy(),
        body_strategy(),
    )
        .prop_map(|(mission_id, node, seq, prev, deps, body)| {
            UnsignedEntry {
                mission_id,
                epoch: PHASE1_EPOCH,
                node: NodeId(node),
                seq,
                prev: Hash(prev),
                deps,
                body,
            }
            .sign(&key())
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5000))]

    #[test]
    fn decode_is_the_exact_inverse_of_encode(entry in entry_strategy()) {
        let decoded = decode_entry_exact(&entry.encoded()).expect("a freshly encoded entry always decodes");
        prop_assert_eq!(decoded, entry);
    }
}

/// A sanity check that the strategy above can actually produce the M1
/// shape (empty `deps`) `PHASE1_MISSION_ID`/`PHASE1_EPOCH` fixtures use —
/// exercised directly rather than left to chance.
#[test]
fn the_m1_shape_round_trips_too() {
    let entry = UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: NodeId(0),
        seq: 0,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body: Body::TaskClaim {
            task: 7,
            priority: 1,
        },
    }
    .sign(&key());
    assert_eq!(decode_entry_exact(&entry.encoded()).unwrap(), entry);
}
