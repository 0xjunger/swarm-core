//! Verifiable evidence packages (`docs/spec.md` §20.2, §20.3): `LogBundle`
//! (the raw signed log) and `Spec` (the rules to check it against) — the two
//! files a stranger needs, and the only two files a stranger needs.
//!
//! Everything in a `LogBundle` is a raw signed [`Entry`]. No derived field —
//! no `causal_vv`, no claims, no escrow balance — is ever carried, because
//! accepting derived state as input is assuming the answer to the question
//! `verify` exists to ask (§20.5's "why an independent fold").

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use ed25519_dalek::VerifyingKey;

use crate::codec::{decode_entry, take_node, take_u16, take_u32, take_u64, DecodeError};
use crate::wire::{Entry, Roster};
use crate::NodeId;

const BUNDLE_DOMAIN_TAG: &[u8] = b"SWARM_BUNDLE_V1";
const SPEC_DOMAIN_TAG: &[u8] = b"SWARM_SPEC_V1";

/// The raw evidence: every signed entry every observer in the bundle holds,
/// keyed first by who observed it and then by who authored it.
///
/// `views[observer][author]` is `observer`'s own copy of `author`'s chain.
/// Keying by observer as well as author — rather than a single
/// author-keyed union — is what makes I1 (conflicting entries at one
/// `(author, seq)`) and I3 (two observers of the same entry set deriving
/// different state) checkable at all: both are properties of what different
/// observers hold, and an author-only bundle has no observers in it
/// (`docs/spec.md` §20.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LogBundle {
    pub mission_id: [u8; 32],
    pub epoch: u32,
    pub views: BTreeMap<NodeId, BTreeMap<NodeId, Vec<Entry>>>,
}

impl LogBundle {
    /// Canonical encoding (`docs/spec.md` §20.2):
    ///
    /// ```text
    /// b"SWARM_BUNDLE_V1"                 (15 bytes)
    /// || mission_id                      (32 bytes)
    /// || epoch                           (4 bytes, u32 BE)
    /// || view_count                      (2 bytes, u16 BE)
    /// || per view, observer ascending:
    ///      observer                      (1 byte)
    ///      chain_count                   (2 bytes, u16 BE)
    ///      || per chain, author ascending:
    ///           author                   (1 byte)
    ///           entry_count              (4 bytes, u32 BE)
    ///           entry * count            (full canonical `Entry` encoding)
    /// ```
    ///
    /// Within a chain, entries are written in whatever order `self.views`
    /// holds them — normally an author's `seq` order, but **not enforced
    /// here**. Out-of-order, missing, or duplicate `seq` is a finding
    /// `verify` reports (`docs/spec.md` §20.5), not a format error: this
    /// method answers "what do these bytes mean," not "is this correct."
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(BUNDLE_DOMAIN_TAG);
        out.extend_from_slice(&self.mission_id);
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&(self.views.len() as u16).to_be_bytes());
        for (observer, chains) in &self.views {
            out.push(observer.0);
            out.extend_from_slice(&(chains.len() as u16).to_be_bytes());
            for (author, entries) in chains {
                out.push(author.0);
                out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
                for entry in entries {
                    out.extend_from_slice(&entry.encoded());
                }
            }
        }
        out
    }

    /// Decodes a whole buffer as one `LogBundle` — the format a file on disk
    /// holds end to end, so trailing bytes after the last chain are an
    /// error here, unlike [`crate::codec::decode_entry`] (`docs/spec.md`
    /// §20.1). Rejects an observer or author `NodeId` that is not strictly
    /// ascending within its list — same canonicity reasoning as
    /// `VersionVector` (§20.1): two different byte strings must never
    /// decode to the same `LogBundle`.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < BUNDLE_DOMAIN_TAG.len() || &bytes[..BUNDLE_DOMAIN_TAG.len()] != BUNDLE_DOMAIN_TAG {
            return Err(DecodeError::BadDomainTag);
        }
        let rest = &bytes[BUNDLE_DOMAIN_TAG.len()..];
        if rest.len() < 32 {
            return Err(DecodeError::Truncated);
        }
        let mut mission_id = [0u8; 32];
        mission_id.copy_from_slice(&rest[..32]);
        let rest = &rest[32..];
        let (epoch, rest) = take_u32(rest)?;
        let (view_count, mut rest) = take_u16(rest)?;

        let mut views = BTreeMap::new();
        let mut last_observer: Option<NodeId> = None;
        for _ in 0..view_count {
            let (observer, r) = take_node(rest)?;
            if let Some(prev) = last_observer {
                if observer <= prev {
                    return Err(DecodeError::NonCanonical("bundle_view_order"));
                }
            }
            last_observer = Some(observer);

            let (chain_count, r) = take_u16(r)?;
            let mut chains = BTreeMap::new();
            let mut last_author: Option<NodeId> = None;
            let mut r = r;
            for _ in 0..chain_count {
                let (author, r2) = take_node(r)?;
                if let Some(prev) = last_author {
                    if author <= prev {
                        return Err(DecodeError::NonCanonical("bundle_chain_order"));
                    }
                }
                last_author = Some(author);

                let (entry_count, r3) = take_u32(r2)?;
                let mut entries = Vec::with_capacity(entry_count as usize);
                let mut r4 = r3;
                for _ in 0..entry_count {
                    let (entry, consumed) = decode_entry(r4)?;
                    entries.push(entry);
                    r4 = &r4[consumed..];
                }
                chains.insert(author, entries);
                r = r4;
            }
            views.insert(observer, chains);
            rest = r;
        }

        if !rest.is_empty() {
            return Err(DecodeError::TrailingBytes);
        }

        Ok(LogBundle {
            mission_id,
            epoch,
            views,
        })
    }

    /// Unions two bundles' views — the way `swarm-sim` assembles one file
    /// covering every node's export from a single run.
    ///
    /// Both bundles must share `mission_id`/`epoch`; call sites in this
    /// codebase only ever merge exports from the same run, so a mismatch is
    /// a caller bug and panics rather than returning a `Result` no honest
    /// caller would ever see. Where both sides hold a chain for the same
    /// `(observer, author)`, the longer one wins: two honest exports of the
    /// same chain differ only in how much of it each observer had seen, so
    /// the longer one is a superset, never a conflicting alternative — an
    /// actual conflict is what I1 exists to catch, downstream in `verify`,
    /// not something this method silently resolves.
    ///
    /// **This is a convenience for assembling one file out of several
    /// honest exports of a single run — it is not a verification step.**
    /// "Longer wins" assumes both sides are honest; an adversarial input can
    /// lose evidence through it (a shorter, edited chain presented alongside
    /// a genuine one that happens to be even shorter would silently win over
    /// nothing, since there is no third chain to compare against). Nothing
    /// downstream may treat a merged bundle as authenticated — `verify`
    /// checks the result the same as any other bundle, no differently.
    ///
    /// Also: this `merge` has nothing to do with the prohibited
    /// `VersionVector::merge` (§0.4 / docs/spec.md §16) — the name collision
    /// is coincidental but worth flagging so nobody reads this method's
    /// existence as license to add that one.
    pub fn merge(mut self, other: LogBundle) -> Self {
        assert_eq!(self.mission_id, other.mission_id, "merge: mission_id mismatch");
        assert_eq!(self.epoch, other.epoch, "merge: epoch mismatch");
        for (observer, other_chains) in other.views {
            let chains = self.views.entry(observer).or_default();
            for (author, other_entries) in other_chains {
                match chains.get(&author) {
                    Some(existing) if existing.len() >= other_entries.len() => {}
                    _ => {
                        chains.insert(author, other_entries);
                    }
                }
            }
        }
        self
    }
}

/// The mission's rules: roster, per-node spending budgets, and the log
/// bound — everything `verify` needs beyond the raw log itself
/// (`docs/spec.md` §20.3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spec {
    pub mission_id: [u8; 32],
    pub epoch: u32,
    pub roster: Roster,
    pub budgets: BTreeMap<NodeId, u64>,
    pub log_cap: u32,
}

impl Spec {
    /// Canonical encoding (`docs/spec.md` §20.3):
    ///
    /// ```text
    /// b"SWARM_SPEC_V1"                   (13 bytes)
    /// || mission_id                      (32 bytes)
    /// || epoch                           (4 bytes, u32 BE)
    /// || roster_count                    (2 bytes, u16 BE)
    /// || (node u8 || verifying_key 32) * count, ascending
    /// || budget_count                    (2 bytes, u16 BE)
    /// || (node u8 || budget u64 BE) * count, ascending
    /// || log_cap                         (4 bytes, u32 BE)
    /// ```
    ///
    /// **Not signed in Phase 1** (`docs/spec.md` §20.3): the verifier
    /// assumes the `Spec` it was handed is the right one. Authenticating the
    /// spec itself is Phase 2.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(SPEC_DOMAIN_TAG);
        out.extend_from_slice(&self.mission_id);
        out.extend_from_slice(&self.epoch.to_be_bytes());

        let members: Vec<NodeId> = self.roster.members().collect();
        out.extend_from_slice(&(members.len() as u16).to_be_bytes());
        for node in &members {
            out.push(node.0);
            let key = self
                .roster
                .key(*node)
                .expect("member came from roster.members()");
            out.extend_from_slice(key.as_bytes());
        }

        out.extend_from_slice(&(self.budgets.len() as u16).to_be_bytes());
        for (node, budget) in &self.budgets {
            out.push(node.0);
            out.extend_from_slice(&budget.to_be_bytes());
        }

        out.extend_from_slice(&self.log_cap.to_be_bytes());
        out
    }

    /// Decodes a whole buffer as one `Spec` (`TrailingBytes` if anything is
    /// left over). Rejects a non-strictly-ascending roster or budget list —
    /// same canonicity reasoning as `LogBundle::decode` — and a 32-byte
    /// roster key that is not a valid Ed25519 point (`DecodeError::BadVerifyingKey`).
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < SPEC_DOMAIN_TAG.len() || &bytes[..SPEC_DOMAIN_TAG.len()] != SPEC_DOMAIN_TAG {
            return Err(DecodeError::BadDomainTag);
        }
        let rest = &bytes[SPEC_DOMAIN_TAG.len()..];
        if rest.len() < 32 {
            return Err(DecodeError::Truncated);
        }
        let mut mission_id = [0u8; 32];
        mission_id.copy_from_slice(&rest[..32]);
        let rest = &rest[32..];
        let (epoch, rest) = take_u32(rest)?;

        let (roster_count, mut rest) = take_u16(rest)?;
        let mut keys = BTreeMap::new();
        let mut last: Option<NodeId> = None;
        for _ in 0..roster_count {
            let (node, r) = take_node(rest)?;
            if let Some(prev) = last {
                if node <= prev {
                    return Err(DecodeError::NonCanonical("spec_roster_order"));
                }
            }
            last = Some(node);
            if r.len() < 32 {
                return Err(DecodeError::Truncated);
            }
            let mut key_bytes = [0u8; 32];
            key_bytes.copy_from_slice(&r[..32]);
            let key =
                VerifyingKey::from_bytes(&key_bytes).map_err(|_| DecodeError::BadVerifyingKey)?;
            keys.insert(node, key);
            rest = &r[32..];
        }
        let roster = Roster::new(mission_id, epoch, keys);

        let (budget_count, mut rest2) = take_u16(rest)?;
        let mut budgets = BTreeMap::new();
        let mut last: Option<NodeId> = None;
        for _ in 0..budget_count {
            let (node, r) = take_node(rest2)?;
            if let Some(prev) = last {
                if node <= prev {
                    return Err(DecodeError::NonCanonical("spec_budget_order"));
                }
            }
            last = Some(node);
            let (budget, r) = take_u64(r)?;
            budgets.insert(node, budget);
            rest2 = r;
        }

        let (log_cap, rest3) = take_u32(rest2)?;
        if !rest3.is_empty() {
            return Err(DecodeError::TrailingBytes);
        }

        Ok(Spec {
            mission_id,
            epoch,
            roster,
            budgets,
            log_cap,
        })
    }
}
