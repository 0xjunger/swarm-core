//! The wire format: `Entry`, canonical encoding, domain-separated signing.
//!
//! `DESIGN.md` D-008 makes the critical decision: **the published message, the
//! log record, and the proof object are the same struct.** One signature over
//! one canonical encoding serves all three roles, so the format is written by
//! hand and pinned by the golden vector — never left to a serializer.

use alloc::vec::Vec;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::causal::VersionVector;
use crate::NodeId;

/// Domain separation tag (`DESIGN.md` D-008, `SPEC.md` §5.2).
///
/// A signature is only valid under the context it was created for. Prefixing
/// the signed bytes with this tag means an entry signature can never be
/// replayed as a future certificate signature, or vice versa.
pub const DOMAIN_TAG: &[u8] = b"SWARM_ENTRY_V1";

/// Phase 1 fixed values (`DESIGN.md` D-008: open the fields now, fill them
/// later). `mission_id` will become the roster Merkle root and `epoch` the
/// roster version; at M1 both are constants, but they are already encoded and
/// already checked, so introducing real values later changes no format.
pub const PHASE1_MISSION_ID: [u8; 32] = [0u8; 32];
pub const PHASE1_EPOCH: u32 = 0;

/// A BLAKE3 digest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    /// The link of an entry with no predecessor (`SPEC.md` §4.2).
    pub const ZERO: Hash = Hash([0u8; 32]);

    pub fn new(bytes: &[u8]) -> Self {
        Hash(*blake3::hash(bytes).as_bytes())
    }
}

/// What an entry means.
///
/// One variant at M1, two at M3, three at M5 (`DESIGN.md` D-008): new
/// variants arrive only when a test demands them, and
/// each arrives with a test — the golden vector covers all three.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Body {
    /// "I claim task `task` with priority `priority`." M3's deterministic
    /// winner rule is `min by (priority, logical_clock, node_id)`
    /// (`SPEC.md` §6.3); `priority` is encoded from day one so that rule
    /// needs no format change, and `logical_clock` is derived from `deps`
    /// rather than carried, so it needs none either (`SPEC.md` §6.3).
    TaskClaim { task: u64, priority: u8 },
    /// "I claimed `task`, I am not the winner, I am standing down."
    ///
    /// A record, not a CRDT operation: it does **not** remove the author's
    /// claim from the claim set (`SPEC.md` §6.3). This is the withdrawal
    /// `DESIGN.md` D-005 describes as "recorded as a log fact instead,
    /// without removing anything from the underlying claim set."
    Withdraw { task: u64 },
    /// "I am spending `amount` from my escrow allocation." M5's escrow
    /// counter: a node's total spending must never exceed the budget it was
    /// allocated at mission start. The per-node cap means the global
    /// invariant I4 — "sum of spend across all partitions ≤ authorised
    /// total" — holds structurally, without consensus (`SPEC.md` §6.4).
    Spend { amount: u64 },
}

impl Body {
    const TAG_TASK_CLAIM: u8 = 0x00;
    const TAG_WITHDRAW: u8 = 0x01;
    const TAG_SPEND: u8 = 0x02;

    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Body::TaskClaim { task, priority } => {
                out.push(Self::TAG_TASK_CLAIM);
                out.extend_from_slice(&task.to_be_bytes());
                out.push(*priority);
            }
            Body::Withdraw { task } => {
                out.push(Self::TAG_WITHDRAW);
                out.extend_from_slice(&task.to_be_bytes());
            }
            Body::Spend { amount } => {
                out.push(Self::TAG_SPEND);
                out.extend_from_slice(&amount.to_be_bytes());
            }
        }
    }
}

/// An entry that has not yet been signed.
///
/// This is the shape a node fills in locally before handing it to its own
/// key. Everything except the signature lives here, so the signing bytes have
/// exactly one source.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnsignedEntry {
    pub mission_id: [u8; 32],
    pub epoch: u32,
    pub node: NodeId,
    pub seq: u64,
    pub prev: Hash,
    pub deps: VersionVector,
    pub body: Body,
}

impl UnsignedEntry {
    /// The bytes that get signed (`SPEC.md` §5.3).
    ///
    /// Written field by field, big-endian, in one fixed order. Serde is never
    /// involved: its output can change with version or settings, and a silent
    /// change would invalidate every signature ever produced.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(91);
        out.extend_from_slice(DOMAIN_TAG);
        out.extend_from_slice(&self.mission_id);
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.push(self.node.0);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.prev.0);
        self.deps.encode(&mut out);
        self.body.encode(&mut out);
        out
    }

    /// Signs the entry, producing the publishable [`Entry`].
    pub fn sign(self, key: &SigningKey) -> Entry {
        let sig = key.sign(&self.signing_bytes());
        Entry {
            mission_id: self.mission_id,
            epoch: self.epoch,
            node: self.node,
            seq: self.seq,
            prev: self.prev,
            deps: self.deps,
            body: self.body,
            sig,
        }
    }
}

/// The published message, the log record, and the proof object — one struct
/// (`DESIGN.md` D-008).
///
/// **Untrusted.** An `Entry` is whatever bytes arrived; nothing about it has
/// been checked. Verification turns it into a [`VerifiedEntry`], and only
/// that type may influence state (`SPEC.md` §4.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub mission_id: [u8; 32],
    pub epoch: u32,
    pub node: NodeId,
    pub seq: u64,
    pub prev: Hash,
    pub deps: VersionVector,
    pub body: Body,
    pub sig: Signature,
}

impl Entry {
    /// The bytes this entry's signature must cover. Identical to the unsigned
    /// form's, because the signature is over everything except itself.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let unsigned = UnsignedEntry {
            mission_id: self.mission_id,
            epoch: self.epoch,
            node: self.node,
            seq: self.seq,
            prev: self.prev,
            deps: self.deps.clone(),
            body: self.body,
        };
        unsigned.signing_bytes()
    }

    /// The full canonical encoding (`SPEC.md` §5.3): the signing bytes
    /// followed by the signature. This is what the hash chain hashes and what
    /// the golden vector pins.
    pub fn encoded(&self) -> Vec<u8> {
        let mut out = self.signing_bytes();
        out.extend_from_slice(&self.sig.to_bytes());
        out
    }

    /// The link this entry contributes to its successor's `prev` field
    /// (`SPEC.md` §4.2): BLAKE3 of the full encoding, signature
    /// included, so tampering with a signature breaks every following link.
    pub fn chain_hash(&self) -> Hash {
        Hash::new(&self.encoded())
    }
}

/// An entry whose signature and roster membership have been checked.
///
/// The only way to obtain one is through verification
/// ([`crate::log::verify_chain`]). Functions that must not see unverified
/// bytes take this type, so forgetting to verify is a compile error rather
/// than a runtime bug (`SPEC.md` §4.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VerifiedEntry(Entry);

impl VerifiedEntry {
    /// Crate-private: only the verifier constructs these.
    pub(crate) fn from_verified(entry: Entry) -> Self {
        VerifiedEntry(entry)
    }

    /// Read access to the verified entry. Deliberately returns `&Entry`, not
    /// `Entry`: the wrapper must not be peeled off and passed around.
    pub fn entry(&self) -> &Entry {
        &self.0
    }
}

/// The mission's member list and their keys, fixed at mission start
/// (`DESIGN.md` D-005).
///
/// At M1 the roster exists to give the verifier something to check
/// membership against; M2 will build it for real.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Roster {
    pub mission_id: [u8; 32],
    pub epoch: u32,
    keys: alloc::collections::BTreeMap<NodeId, VerifyingKey>,
}

impl Roster {
    pub fn new(
        mission_id: [u8; 32],
        epoch: u32,
        keys: alloc::collections::BTreeMap<NodeId, VerifyingKey>,
    ) -> Self {
        Roster {
            mission_id,
            epoch,
            keys,
        }
    }

    pub fn key(&self, node: NodeId) -> Option<&VerifyingKey> {
        self.keys.get(&node)
    }

    pub fn members(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.keys.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    fn unsigned() -> UnsignedEntry {
        UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node: NodeId(1),
            seq: 0,
            prev: Hash::ZERO,
            deps: VersionVector::new(),
            body: Body::TaskClaim {
                task: 7,
                priority: 1,
            },
        }
    }

    #[test]
    fn signing_bytes_begin_with_the_domain_tag() {
        let bytes = unsigned().signing_bytes();
        assert_eq!(&bytes[..DOMAIN_TAG.len()], DOMAIN_TAG);
    }

    #[test]
    fn signature_round_trips() {
        let k = key(1);
        let entry = unsigned().sign(&k);
        assert!(k
            .verifying_key()
            .verify_strict(&entry.signing_bytes(), &entry.sig)
            .is_ok());
    }

    #[test]
    fn signature_fails_under_a_different_key() {
        let entry = unsigned().sign(&key(1));
        assert!(key(2)
            .verifying_key()
            .verify_strict(&entry.signing_bytes(), &entry.sig)
            .is_err());
    }

    #[test]
    fn encoding_covers_the_signature() {
        // The chain hash must change if the signature alone changes: the
        // full encoding includes it (`SPEC.md` §4.2).
        let entry = unsigned().sign(&key(1));
        let mut forged = entry.clone();
        let mut sig = forged.sig.to_bytes();
        sig[0] ^= 1;
        forged.sig = Signature::from_bytes(&sig);
        assert_ne!(entry.chain_hash(), forged.chain_hash());
    }

    #[test]
    fn task_claim_encodes_tag_task_priority() {
        let mut out = Vec::new();
        Body::TaskClaim {
            task: 7,
            priority: 1,
        }
        .encode(&mut out);
        assert_eq!(out, [0x00, 0, 0, 0, 0, 0, 0, 0, 7, 1]);
    }

    #[test]
    fn withdraw_encodes_tag_and_task() {
        let mut out = Vec::new();
        Body::Withdraw { task: 7 }.encode(&mut out);
        assert_eq!(out, [0x01, 0, 0, 0, 0, 0, 0, 0, 7]);
    }

    #[test]
    fn the_two_bodies_never_share_an_encoding() {
        // Distinct tags are what keep a claim from being read as a withdrawal
        // under the same signature (`SPEC.md` §5.3).
        let (mut claim, mut withdraw) = (Vec::new(), Vec::new());
        Body::TaskClaim {
            task: 7,
            priority: 0,
        }
        .encode(&mut claim);
        Body::Withdraw { task: 7 }.encode(&mut withdraw);
        assert_ne!(claim, withdraw);
        assert_ne!(claim[0], withdraw[0]);
    }

    #[test]
    fn spend_encodes_tag_and_amount() {
        let mut out = Vec::new();
        Body::Spend { amount: 1 }.encode(&mut out);
        assert_eq!(out, [0x02, 0, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn spend_never_shares_an_encoding_with_claim_or_withdraw() {
        // Distinct tags — same reasoning as `the_two_bodies_never_share_an_encoding`.
        let (mut claim, mut withdraw, mut spend) = (Vec::new(), Vec::new(), Vec::new());
        Body::TaskClaim {
            task: 0,
            priority: 0,
        }
        .encode(&mut claim);
        Body::Withdraw { task: 0 }.encode(&mut withdraw);
        Body::Spend { amount: 0 }.encode(&mut spend);
        assert_ne!(spend, claim);
        assert_ne!(spend, withdraw);
        assert_eq!(spend[0], 0x02);
    }
}
