//! The golden vector (`DESIGN.md`, "Entry ile nasıl çalışmalı", item 5): the
//! byte-exact encoding and signature of one known `Entry` under one known
//! key.
//!
//! If this test breaks, the wire format has changed. That may be deliberate —
//! in which case update this file and state the reason in the commit message
//! (`DESIGN.md` §11.5) — but it must never happen silently.

use ed25519_dalek::{SigningKey, VerifyingKey};
use swarm_core::causal::VersionVector;
use swarm_core::wire::{Body, Hash, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;

/// The golden key: deterministic, public, test-only. Bytes 0..=31.
const GOLDEN_KEY: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31,
];

fn golden_entry() -> swarm_core::wire::Entry {
    let key = SigningKey::from_bytes(&GOLDEN_KEY);
    UnsignedEntry {
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
    .sign(&key)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The pinned signing bytes of the golden entry (`docs/spec-m1.md` §3.1).
/// Computed independently from the spec layout: tag || mission_id || epoch
/// || node || seq || prev || deps (empty) || TaskClaim{task: 7, priority: 1}.
const GOLDEN_SIGNING_HEX: &str = "535741524d5f454e5452595f5631\
    0000000000000000000000000000000000000000000000000000000000000000\
    00000000\
    00\
    0000000000000000\
    0000000000000000000000000000000000000000000000000000000000000000\
    0000\
    00\
    0000000000000007\
    01";

/// The pinned full encoding of the golden entry (`docs/spec-m1.md` §3.4):
/// the signing bytes above followed by the Ed25519 signature.
const GOLDEN_ENCODED_HEX: &str = "535741524d5f454e5452595f5631\
    0000000000000000000000000000000000000000000000000000000000000000\
    00000000\
    00\
    0000000000000000\
    0000000000000000000000000000000000000000000000000000000000000000\
    0000\
    00\
    0000000000000007\
    01\
    eaccc7faafe07d9e81d48c99580d9b8e4db285625a5609b91944bd819445fc6a\
    d15ca63abc640d053e4291e3e1226a8af8f8053ffccd19bdd525516f950ec20a";

#[test]
fn golden_vector_pins_the_wire_format() {
    let e = golden_entry();

    assert_eq!(
        hex(&e.signing_bytes()),
        GOLDEN_SIGNING_HEX,
        "the signing bytes changed — if deliberate, update this file and \
         state the reason in the commit message (DESIGN.md §11.5)"
    );
    assert_eq!(
        hex(&e.encoded()),
        GOLDEN_ENCODED_HEX,
        "the full encoding changed — if deliberate, update this file and \
         state the reason in the commit message (DESIGN.md §11.5)"
    );
}

#[test]
fn golden_signature_verifies_under_the_golden_key() {
    // The pinned bytes are not decoration: the signature they contain must
    // actually verify, and must not verify under any other key.
    let e = golden_entry();
    let vk = SigningKey::from_bytes(&GOLDEN_KEY).verifying_key();
    assert!(vk.verify_strict(&e.signing_bytes(), &e.sig).is_ok());

    let mut other = GOLDEN_KEY;
    other[0] ^= 1;
    let other_vk: VerifyingKey = SigningKey::from_bytes(&other).verifying_key();
    assert!(other_vk.verify_strict(&e.signing_bytes(), &e.sig).is_err());
}
