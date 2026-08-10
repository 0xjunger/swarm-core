//! Decoding: the inverse of `wire`'s canonical encoders (`docs/spec.md` §20).
//!
//! Every decoder here answers exactly one question — "what do these bytes
//! mean" — never "is this correct". A `LogBundle` chain out of `seq` order,
//! or with a gap or a duplicate `seq`, decodes cleanly; whether that is a
//! violation is `swarm-verify`'s question, not this module's
//! (`docs/spec.md` §20.2).
//!
//! Canonicity of the pieces decoded here is not optional, though. Two
//! distinct byte strings that decoded to the same `VersionVector` would let
//! an attacker manufacture a fake proof of equivocation and frame an honest
//! node (`DESIGN.md` §7) — so a vector whose components are not strictly
//! ascending by `NodeId`, including one that names a `NodeId` twice, is
//! rejected rather than merely re-sorted. An unrecognised `Body` tag is
//! likewise rejected outright: forward compatibility (silently skipping an
//! unknown field) is explicitly out of scope for Phase 1
//! (`docs/spec.md` §20.1, §5 "Kapsam dışı").
//!
//! Tag bytes are written here as the literals from `docs/spec.md` §8.2,
//! independently of `wire::Body::encode` — the same convention the golden
//! vectors use (`tests/golden_vector.rs`): an inverse that shares code with
//! the thing it inverts can share a bug with it too.

use ed25519_dalek::Signature;

use crate::causal::VersionVector;
use crate::wire::{Body, Entry, Hash, DOMAIN_TAG};
use crate::NodeId;

const TAG_TASK_CLAIM: u8 = 0x00;
const TAG_WITHDRAW: u8 = 0x01;
const TAG_SPEND: u8 = 0x02;

/// Why a decode failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecodeError {
    /// Fewer bytes were present than the format requires at this point.
    Truncated,
    /// A `Body` tag outside `0x00..=0x02`.
    UnknownBodyTag(u8),
    /// The leading bytes do not match `wire::DOMAIN_TAG`.
    BadDomainTag,
    /// Bytes remained after a value that was expected to consume the whole
    /// buffer.
    TrailingBytes,
    /// The bytes present do not correspond to the canonical encoding of
    /// anything — see the module docs. The string names which rule was
    /// violated, for diagnostics only; it plays no role in equality beyond
    /// itself.
    NonCanonical(&'static str),
    /// 32 bytes that do not decode to a valid Ed25519 point.
    BadVerifyingKey,
}

/// Reads exactly `N` bytes off the front of `bytes`, returning the array and
/// what remains.
fn take<const N: usize>(bytes: &[u8]) -> Result<([u8; N], &[u8]), DecodeError> {
    if bytes.len() < N {
        return Err(DecodeError::Truncated);
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes[..N]);
    Ok((out, &bytes[N..]))
}

fn take_u8(bytes: &[u8]) -> Result<(u8, &[u8]), DecodeError> {
    let (b, rest) = take::<1>(bytes)?;
    Ok((b[0], rest))
}

pub(crate) fn take_u16(bytes: &[u8]) -> Result<(u16, &[u8]), DecodeError> {
    let (b, rest) = take::<2>(bytes)?;
    Ok((u16::from_be_bytes(b), rest))
}

pub(crate) fn take_u32(bytes: &[u8]) -> Result<(u32, &[u8]), DecodeError> {
    let (b, rest) = take::<4>(bytes)?;
    Ok((u32::from_be_bytes(b), rest))
}

pub(crate) fn take_u64(bytes: &[u8]) -> Result<(u64, &[u8]), DecodeError> {
    let (b, rest) = take::<8>(bytes)?;
    Ok((u64::from_be_bytes(b), rest))
}

pub(crate) fn take_node(bytes: &[u8]) -> Result<(NodeId, &[u8]), DecodeError> {
    let (b, rest) = take_u8(bytes)?;
    Ok((NodeId(b), rest))
}

fn take_domain_tag<'a>(bytes: &'a [u8], tag: &[u8]) -> Result<&'a [u8], DecodeError> {
    if bytes.len() < tag.len() || &bytes[..tag.len()] != tag {
        return Err(DecodeError::BadDomainTag);
    }
    Ok(&bytes[tag.len()..])
}

/// Decodes a `Body` (`docs/spec.md` §8.2): a one-byte tag, then the
/// variant's fields.
pub fn decode_body(bytes: &[u8]) -> Result<(Body, usize), DecodeError> {
    let start_len = bytes.len();
    let (tag, rest) = take_u8(bytes)?;
    let (body, rest) = match tag {
        TAG_TASK_CLAIM => {
            let (task, rest) = take_u64(rest)?;
            let (priority, rest) = take_u8(rest)?;
            (Body::TaskClaim { task, priority }, rest)
        }
        TAG_WITHDRAW => {
            let (task, rest) = take_u64(rest)?;
            (Body::Withdraw { task }, rest)
        }
        TAG_SPEND => {
            let (amount, rest) = take_u64(rest)?;
            (Body::Spend { amount }, rest)
        }
        other => return Err(DecodeError::UnknownBodyTag(other)),
    };
    Ok((body, start_len - rest.len()))
}

/// Decodes a `VersionVector` (`docs/spec.md` §8.2): a `u16` count, then that
/// many `(NodeId, seq)` pairs. Rejects anything not strictly ascending by
/// `NodeId` — equal or descending, including an outright repeated `NodeId`
/// — since accepting it would let two different byte strings decode to the
/// same vector.
pub fn decode_version_vector(bytes: &[u8]) -> Result<(VersionVector, usize), DecodeError> {
    let start_len = bytes.len();
    let (count, mut rest) = take_u16(bytes)?;
    let mut vv = VersionVector::new();
    let mut last: Option<NodeId> = None;
    for _ in 0..count {
        let (node, r) = take_node(rest)?;
        let (seq, r) = take_u64(r)?;
        rest = r;
        if let Some(prev) = last {
            if node <= prev {
                return Err(DecodeError::NonCanonical("version_vector_order"));
            }
        }
        last = Some(node);
        vv.bump(node, seq);
    }
    Ok((vv, start_len - rest.len()))
}

/// Decodes an `Entry` (`docs/spec.md` §8.2): domain tag, `mission_id`,
/// `epoch`, `node`, `seq`, `prev`, `deps`, `body`, `sig` — the exact inverse
/// of `UnsignedEntry::signing_bytes` plus the trailing signature. Returns
/// the number of bytes consumed so a caller can decode several entries back
/// to back without knowing their length in advance (`docs/spec.md` §20.2,
/// `LogBundle`).
pub fn decode_entry(bytes: &[u8]) -> Result<(Entry, usize), DecodeError> {
    let start_len = bytes.len();
    let rest = take_domain_tag(bytes, DOMAIN_TAG)?;
    let (mission_id, rest) = take::<32>(rest)?;
    let (epoch, rest) = take_u32(rest)?;
    let (node, rest) = take_node(rest)?;
    let (seq, rest) = take_u64(rest)?;
    let (prev, rest) = take::<32>(rest)?;
    let (deps, consumed) = decode_version_vector(rest)?;
    let rest = &rest[consumed..];
    let (body, consumed) = decode_body(rest)?;
    let rest = &rest[consumed..];
    let (sig, rest) = take::<64>(rest)?;

    let entry = Entry {
        mission_id,
        epoch,
        node,
        seq,
        prev: Hash(prev),
        deps,
        body,
        sig: Signature::from_bytes(&sig),
    };
    Ok((entry, start_len - rest.len()))
}

/// Decodes exactly one `Entry` and requires the buffer to hold nothing else
/// (`TrailingBytes` otherwise). The form used wherever a buffer is known to
/// carry a single entry end to end — the golden-vector reverse tests.
/// `LogBundle`/`Spec` decode several entries out of one larger buffer and
/// check for trailing bytes only after the last one, so they call
/// [`decode_entry`] directly instead.
pub fn decode_entry_exact(bytes: &[u8]) -> Result<Entry, DecodeError> {
    let (entry, consumed) = decode_entry(bytes)?;
    if consumed != bytes.len() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use crate::wire::{UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    fn sample_entry() -> Entry {
        let mut deps = VersionVector::new();
        deps.bump(NodeId(0), 3);
        deps.bump(NodeId(2), 1);
        UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node: NodeId(1),
            seq: 4,
            prev: Hash::new(b"prev"),
            deps,
            body: Body::TaskClaim {
                task: 9,
                priority: 2,
            },
        }
        .sign(&key(7))
    }

    // -----------------------------------------------------------------
    // Round trip
    // -----------------------------------------------------------------

    #[test]
    fn decode_entry_exact_round_trips_a_freshly_signed_entry() {
        let entry = sample_entry();
        let decoded = decode_entry_exact(&entry.encoded()).expect("decodes");
        assert_eq!(decoded, entry);
    }

    #[test]
    fn decode_entry_reports_bytes_consumed() {
        let entry = sample_entry();
        let encoded = entry.encoded();
        let (_, consumed) = decode_entry(&encoded).expect("decodes");
        assert_eq!(consumed, encoded.len());
    }

    #[test]
    fn two_entries_decode_back_to_back() {
        let a = sample_entry();
        let mut b_unsigned = UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node: NodeId(1),
            seq: 5,
            prev: a.chain_hash(),
            deps: VersionVector::new(),
            body: Body::Withdraw { task: 9 },
        };
        b_unsigned.deps.bump(NodeId(1), 4);
        let b = b_unsigned.sign(&key(7));

        let mut buf = a.encoded();
        buf.extend_from_slice(&b.encoded());

        let (decoded_a, consumed_a) = decode_entry(&buf).expect("first entry decodes");
        assert_eq!(decoded_a, a);
        let (decoded_b, consumed_b) = decode_entry(&buf[consumed_a..]).expect("second decodes");
        assert_eq!(decoded_b, b);
        assert_eq!(consumed_a + consumed_b, buf.len());
    }

    // -----------------------------------------------------------------
    // Negative canonicity tests (5 required by `docs/spec.md` §20.1)
    // -----------------------------------------------------------------

    #[test]
    fn out_of_order_version_vector_is_rejected() {
        // count=2, then (node 2, seq 0) followed by (node 1, seq 0):
        // descending, not strictly ascending.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u16.to_be_bytes());
        bytes.push(2);
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&0u64.to_be_bytes());
        assert_eq!(
            decode_version_vector(&bytes),
            Err(DecodeError::NonCanonical("version_vector_order"))
        );
    }

    #[test]
    fn repeated_node_id_in_version_vector_is_rejected() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u16.to_be_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&5u64.to_be_bytes());
        assert_eq!(
            decode_version_vector(&bytes),
            Err(DecodeError::NonCanonical("version_vector_order"))
        );
    }

    #[test]
    fn truncated_entry_is_rejected() {
        let entry = sample_entry();
        let encoded = entry.encoded();
        // Cut off mid-signature: well past the domain tag, still short.
        let short = &encoded[..encoded.len() - 3];
        assert_eq!(decode_entry(short), Err(DecodeError::Truncated));
    }

    #[test]
    fn trailing_bytes_after_an_entry_are_rejected_by_the_exact_form() {
        let entry = sample_entry();
        let mut encoded = entry.encoded();
        encoded.push(0xFF);
        assert_eq!(
            decode_entry_exact(&encoded),
            Err(DecodeError::TrailingBytes)
        );
    }

    #[test]
    fn unknown_body_tag_is_rejected() {
        let entry = sample_entry();
        let mut encoded = entry.encoded();
        // The body tag sits right after mission_id/epoch/node/seq/prev/deps;
        // for `sample_entry` deps has two components, so: 14 (tag) + 32
        // (mission_id) + 4 (epoch) + 1 (node) + 8 (seq) + 32 (prev) + 2 (vv
        // count) + 2*(1+8) (vv components) = 111.
        let body_tag_index = 14 + 32 + 4 + 1 + 8 + 32 + 2 + 2 * (1 + 8);
        assert_eq!(encoded[body_tag_index], TAG_TASK_CLAIM);
        encoded[body_tag_index] = 0x7F;
        assert_eq!(
            decode_entry(&encoded),
            Err(DecodeError::UnknownBodyTag(0x7F))
        );
    }

    #[test]
    fn bad_domain_tag_is_rejected() {
        let entry = sample_entry();
        let mut encoded = entry.encoded();
        encoded[0] ^= 0xFF;
        assert_eq!(decode_entry(&encoded), Err(DecodeError::BadDomainTag));
    }
}
