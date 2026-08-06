//! The golden vector (`DESIGN.md`, "Entry ile nasıl çalışmalı", item 5): the
//! byte-exact encoding and signature of one known `Entry` under one known
//! key.
//!
//! If this test breaks, the wire format has changed. That may be deliberate —
//! in which case update this file and state the reason in the commit message
//! (`DESIGN.md` §11.5) — but it must never happen silently.

use ed25519_dalek::{SigningKey, VerifyingKey};
use swarm_core::causal::VersionVector;
use swarm_core::wire::{Body, Entry, Hash, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
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

// ---------------------------------------------------------------------------
// M2: a second, independent golden vector for a *non-empty* `VersionVector`
// (`docs/spec-m2.md` §10). M1's vector above is unchanged — this is an
// addition, not a replacement, proving `deps`'s already-frozen encoding
// (`docs/spec-m1.md` §3.2: `u16 BE` count, then `(node u8, seq u64 BE)`
// pairs ascending by `NodeId`) holds once the field actually carries data.
// ---------------------------------------------------------------------------

fn golden_entry_with_deps() -> swarm_core::wire::Entry {
    let key = SigningKey::from_bytes(&GOLDEN_KEY);
    let mut deps = VersionVector::new();
    deps.bump(NodeId(0), 5);
    deps.bump(NodeId(1), 9);
    UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: NodeId(2),
        seq: 3,
        prev: Hash::ZERO,
        deps,
        body: Body::TaskClaim {
            task: 42,
            priority: 7,
        },
    }
    .sign(&key)
}

/// Computed independently from the spec layout (same method as
/// `GOLDEN_SIGNING_HEX` above): tag || mission_id || epoch || node || seq ||
/// prev || deps({0: 5, 1: 9}) || TaskClaim{task: 42, priority: 7}. The deps
/// segment is `0002` (count) `00` `0000000000000005` (node 0, seq 5) `01`
/// `0000000000000009` (node 1, seq 9) — ascending by `NodeId`, per R4.
const GOLDEN_SIGNING_HEX_WITH_DEPS: &str = "535741524d5f454e5452595f5631\
    0000000000000000000000000000000000000000000000000000000000000000\
    00000000\
    02\
    0000000000000003\
    0000000000000000000000000000000000000000000000000000000000000000\
    0002\
    00\
    0000000000000005\
    01\
    0000000000000009\
    00\
    000000000000002a\
    07";

const GOLDEN_ENCODED_HEX_WITH_DEPS: &str = "535741524d5f454e5452595f5631\
    0000000000000000000000000000000000000000000000000000000000000000\
    00000000\
    02\
    0000000000000003\
    0000000000000000000000000000000000000000000000000000000000000000\
    0002\
    00\
    0000000000000005\
    01\
    0000000000000009\
    00\
    000000000000002a\
    07\
    ad10d834ef404ca1401af5e5a40ddf9116ae0884ed934a12925f26c5365cfe0\
    10dd69da5e955ace58bafc959863c77a485e9e419960f4e3b5ee19474c20873\
    05";

#[test]
fn golden_vector_with_a_non_empty_version_vector() {
    let e = golden_entry_with_deps();

    assert_eq!(
        hex(&e.signing_bytes()),
        GOLDEN_SIGNING_HEX_WITH_DEPS,
        "the non-empty deps encoding changed — if deliberate, update this \
         file and state the reason in the commit message (DESIGN.md §11.5)"
    );
    assert_eq!(hex(&e.encoded()), GOLDEN_ENCODED_HEX_WITH_DEPS);

    let vk = SigningKey::from_bytes(&GOLDEN_KEY).verifying_key();
    assert!(vk.verify_strict(&e.signing_bytes(), &e.sig).is_ok());
}

// ---------------------------------------------------------------------------
// M3: a third golden vector for the `Withdraw` body (`docs/spec-m3.md` §10).
// Every `Body` variant arrives with a test (`DESIGN.md` §11.4). The two
// vectors above must stay byte-identical — a new enum variant adds a tag, it
// never rewrites an existing encoding. If either of them moves, something has
// silently altered a frozen format.
// ---------------------------------------------------------------------------

fn golden_entry_withdraw() -> Entry {
    let key = SigningKey::from_bytes(&GOLDEN_KEY);
    let mut deps = VersionVector::new();
    deps.bump(NodeId(0), 2);
    UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: NodeId(1),
        seq: 4,
        prev: Hash::ZERO,
        deps,
        body: Body::Withdraw { task: 7 },
    }
    .sign(&key)
}

/// Computed independently from the spec layout: tag || mission_id || epoch ||
/// node || seq || prev || deps({0: 2}) || Withdraw{task: 7}. The body segment
/// is `01` (tag, `docs/spec-m3.md` §2) followed by `0000000000000007` — eight
/// big-endian bytes and **no priority byte**, which is what distinguishes it
/// from `TaskClaim`.
const GOLDEN_SIGNING_HEX_WITHDRAW: &str = "535741524d5f454e5452595f5631\
    0000000000000000000000000000000000000000000000000000000000000000\
    00000000\
    01\
    0000000000000004\
    0000000000000000000000000000000000000000000000000000000000000000\
    0001\
    00\
    0000000000000002\
    01\
    0000000000000007";

const GOLDEN_ENCODED_HEX_WITHDRAW: &str = "535741524d5f454e5452595f5631\
    0000000000000000000000000000000000000000000000000000000000000000\
    00000000\
    01\
    0000000000000004\
    0000000000000000000000000000000000000000000000000000000000000000\
    0001\
    00\
    0000000000000002\
    01\
    0000000000000007\
    6bc5f7d9ea7b1a7970bfd7642c2e5654be761f5bbe7a2a8ffdb49e77d9dc5e1c\
    3ec4dfc13c2bc9a3aa51377a87f9890c7a4e069462a7ce936bf060da14937207";

#[test]
fn golden_vector_pins_the_withdraw_body() {
    let e = golden_entry_withdraw();

    assert_eq!(
        hex(&e.signing_bytes()),
        GOLDEN_SIGNING_HEX_WITHDRAW,
        "the Withdraw encoding changed — if deliberate, update this file and \
         state the reason in the commit message (DESIGN.md §11.5)"
    );
    assert_eq!(hex(&e.encoded()), GOLDEN_ENCODED_HEX_WITHDRAW);

    let vk = SigningKey::from_bytes(&GOLDEN_KEY).verifying_key();
    assert!(vk.verify_strict(&e.signing_bytes(), &e.sig).is_ok());
}

/// A claim and a withdrawal naming the same task must never produce the same
/// signed bytes — otherwise one signature would attest to both
/// (`docs/spec-m3.md` §2).
#[test]
fn a_claim_and_a_withdrawal_for_one_task_sign_different_bytes() {
    let key = SigningKey::from_bytes(&GOLDEN_KEY);
    let unsigned = |body| UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node: NodeId(1),
        seq: 4,
        prev: Hash::ZERO,
        deps: VersionVector::new(),
        body,
    };
    let claim = unsigned(Body::TaskClaim {
        task: 7,
        priority: 1,
    })
    .sign(&key);
    let withdraw = unsigned(Body::Withdraw { task: 7 }).sign(&key);

    assert_ne!(claim.signing_bytes(), withdraw.signing_bytes());
    assert_ne!(claim.sig, withdraw.sig);
}
