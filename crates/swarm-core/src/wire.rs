//! The wire format: `Entry`, canonical encoding, domain-separated signing.
//!
//! `DESIGN.md` §3 makes the critical decision: **the published message, the
//! log record, and the proof object are the same struct.** One signature over
//! one canonical encoding serves all three roles, so the format is written by
//! hand and pinned by the golden vector — never left to a serializer.

use alloc::vec::Vec;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::causal::VersionVector;
use crate::NodeId;

/// Domain separation tag (`DESIGN.md` §7, `docs/spec-m1.md` §3.1).
///
/// A signature is only valid under the context it was created for. Prefixing
/// the signed bytes with this tag means an entry signature can never be
/// replayed as a future certificate signature, or vice versa.
pub const DOMAIN_TAG: &[u8] = b"SWARM_ENTRY_V1";

/// Phase 1 fixed values (`DESIGN.md`, item 1: open the fields now, fill them
/// later). `mission_id` will become the roster Merkle root and `epoch` the
/// roster version; at M1 both are constants, but they are already encoded and
/// already checked, so introducing real values later changes no format.
pub const PHASE1_MISSION_ID: [u8; 32] = [0u8; 32];
pub const PHASE1_EPOCH: u32 = 0;

/// A BLAKE3 digest.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    /// The link of an entry with no predecessor (`docs/spec-m1.md` §4.3).
    pub const ZERO: Hash = Hash([0u8; 32]);

    pub fn new(bytes: &[u8]) -> Self {
        Hash(*blake3::hash(bytes).as_bytes())
    }
}

/// What an entry means.
///
/// One variant at M1, two at M3 (`DESIGN.md`, item 3): new variants arrive
/// only when a test demands them (`DESIGN.md` §11.4), and each arrives with a
/// test — the golden vector covers both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Body {
    /// "I claim task `task` with priority `priority`." M3's deterministic
    /// winner rule is `min by (priority, logical_clock, node_id)`
    /// (`DESIGN.md` §4.2); `priority` is encoded from day one so that rule
    /// needs no format change, and `logical_clock` is derived from `deps`
    /// rather than carried, so it needs none either (`docs/spec-m3.md` §3).
    TaskClaim { task: u64, priority: u8 },
    /// "I claimed `task`, I am not the winner, I am standing down."
    ///
    /// A record, not a CRDT operation: it does **not** remove the author's
    /// claim from the claim set (`docs/spec-m3.md` §4.1). This is the
    /// "geri çekilme kaydı" M3's acceptance criterion asks for.
    Withdraw { task: u64 },
}

impl Body {
    const TAG_TASK_CLAIM: u8 = 0x00;
    const TAG_WITHDRAW: u8 = 0x01;

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
    /// The bytes that get signed (`docs/spec-m1.md` §3.1).
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
/// (`DESIGN.md` §3).
///
/// **Untrusted.** An `Entry` is whatever bytes arrived; nothing about it has
/// been checked. Verification turns it into a [`VerifiedEntry`], and only
/// that type may influence state (`DESIGN.md`, item 4).
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

    /// The full canonical encoding (`docs/spec-m1.md` §3.4): the signing bytes
    /// followed by the signature. This is what the hash chain hashes and what
    /// the golden vector pins.
    pub fn encoded(&self) -> Vec<u8> {
        let mut out = self.signing_bytes();
        out.extend_from_slice(&self.sig.to_bytes());
        out
    }

    /// The link this entry contributes to its successor's `prev` field
    /// (`docs/spec-m1.md` §4.3): BLAKE3 of the full encoding, signature
    /// included, so tampering with a signature breaks every following link.
    pub fn chain_hash(&self) -> Hash {
        Hash::new(&self.encoded())
    }

    /// Checks the signature against a known key. Membership and chain rules
    /// are checked by [`crate::log::verify_chain`], not here.
    pub fn verify_signature(&self, key: &VerifyingKey) -> bool {
        key.verify_strict(&self.signing_bytes(), &self.sig).is_ok()
    }
}

/// An entry whose signature and roster membership have been checked.
///
/// The only way to obtain one is through verification
/// ([`crate::log::verify_chain`]). Functions that must not see unverified
/// bytes take this type, so forgetting to verify is a compile error rather
/// than a runtime bug (`DESIGN.md`, item 4).
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
/// (`DESIGN.md` §7).
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
        assert!(entry.verify_signature(&k.verifying_key()));
    }

    #[test]
    fn signature_fails_under_a_different_key() {
        let entry = unsigned().sign(&key(1));
        assert!(!entry.verify_signature(&key(2).verifying_key()));
    }

    #[test]
    fn encoding_covers_the_signature() {
        // The chain hash must change if the signature alone changes: the
        // full encoding includes it (docs/spec-m1.md §4.3).
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
        // under the same signature (`docs/spec-m3.md` §2).
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
}
