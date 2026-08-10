//! `swarm-verify`'s own fold over raw entries (`docs/spec.md` §20.5).
//!
//! Deliberately independent of `swarm_core::state::{Claims, Escrow}`: a
//! verifier that reconstructs derived state by calling the same code the
//! system under test uses to reconstruct derived state is a mirror, not a
//! second opinion. This module restates the causal-delivery fixed point and
//! the winner rule from scratch, so that a bug specific to `swarm-core`'s
//! own fold (the `mutant-i3` feature exists to prove such a bug is
//! catchable) is a bug `verify` has no way to inherit.

use std::collections::BTreeMap;

use swarm_core::causal::VersionVector;
use swarm_core::wire::Entry;
use swarm_core::NodeId;

/// The result of replaying one observer's held chains to a causal fixed
/// point.
pub struct Replay {
    /// Entries applied, in application order: each entry's `deps` were
    /// satisfied by everything applied before it.
    pub applied: Vec<Entry>,
    /// `(author, entry)` pairs this observer holds that the fixed-point
    /// replay could never reach — their `deps` name something absent from
    /// everything else this same observer holds.
    ///
    /// A compliant node only ever stores an entry once its `deps` are
    /// satisfied (`swarm-core`'s own `attempt_apply`, `docs/spec.md` §9.3),
    /// so an entry an observer holds but which this replay cannot reach is
    /// direct, self-contained evidence that the bundle does not reflect an
    /// honest export — I2's witness (§20.5).
    pub leftover: Vec<(NodeId, Entry)>,
    /// The version vector at the fixed point — everything in `applied`,
    /// nothing else. Callers use this with [`first_missing_dep`] to name
    /// exactly which dependency a leftover entry is missing.
    pub final_vv: VersionVector,
}

/// Replays `chains` to a causal fixed point: repeatedly applies whichever
/// next entry — in each chain's fixed position order — has its `deps`
/// satisfied by what has been applied so far, until one full pass makes no
/// further progress. The same fixed-point shape as `swarm-core`'s own
/// `drain_buffer` (`docs/spec.md` §9.3), reimplemented here rather than
/// called.
///
/// Callers pass only chains that already survived `verify_chain` (§8.3):
/// that guarantees each chain's own `seq` is contiguous from zero, so
/// within-chain order is already settled and only cross-author `deps` are
/// left to resolve here.
///
/// The version vector is bumped by `entry.node`, the signer, never by the
/// map key `chains` is filed under — `verify_chains` (`verify.rs`) already
/// guarantees the two agree for every chain reaching here, but keying the
/// fold off the signer rather than the bundle's structure keeps that true
/// even if a caller's guarantee ever weakens.
pub fn causal_replay(chains: &BTreeMap<NodeId, Vec<Entry>>) -> Replay {
    let mut cursor: BTreeMap<NodeId, usize> = chains.keys().map(|&n| (n, 0)).collect();
    let mut vv = VersionVector::new();
    let mut applied = Vec::new();

    loop {
        let mut progressed = false;
        for (&author, entries) in chains {
            let pos = cursor[&author];
            let Some(entry) = entries.get(pos) else {
                continue;
            };
            if entry.deps.le(&vv) {
                vv.bump(entry.node, entry.seq);
                applied.push(entry.clone());
                cursor.insert(author, pos + 1);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    let mut leftover = Vec::new();
    for (&author, entries) in chains {
        let pos = cursor[&author];
        for entry in &entries[pos..] {
            leftover.push((author, entry.clone()));
        }
    }

    Replay {
        applied,
        leftover,
        final_vv: vv,
    }
}

/// The first `(origin, seq)` component of `entry.deps` that `vv` does not
/// cover — ascending by `NodeId` (`VersionVector::iter`'s own order), so the
/// choice is deterministic when more than one component is unmet.
///
/// # Panics
///
/// If every component of `entry.deps` is already covered by `vv`. Only
/// called on [`Replay::leftover`] entries, which by construction have at
/// least one unmet component — that is the only reason they are leftover.
pub fn first_missing_dep(entry: &Entry, vv: &VersionVector) -> (NodeId, u64) {
    entry
        .deps
        .iter()
        .find(|&(origin, seq)| vv.highest(origin).is_none_or(|h| h < seq))
        .expect("a leftover entry has at least one unmet dependency by construction")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use swarm_core::wire::{Body, Hash, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    fn claim(node: NodeId, seq: u64, deps: VersionVector, task: u64) -> Entry {
        UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node,
            seq,
            prev: Hash::ZERO,
            deps,
            body: Body::TaskClaim { task, priority: 1 },
        }
        .sign(&key(node.0 + 1))
    }

    #[test]
    fn independent_chains_all_apply_with_no_leftover() {
        let a0 = claim(NodeId(0), 0, VersionVector::new(), 1);
        let b0 = claim(NodeId(1), 0, VersionVector::new(), 2);
        let mut chains = BTreeMap::new();
        chains.insert(NodeId(0), vec![a0]);
        chains.insert(NodeId(1), vec![b0]);

        let replay = causal_replay(&chains);
        assert_eq!(replay.applied.len(), 2);
        assert!(replay.leftover.is_empty());
    }

    #[test]
    fn a_cross_author_dependency_that_is_present_applies_in_order() {
        let a0 = claim(NodeId(0), 0, VersionVector::new(), 1);
        let mut deps = VersionVector::new();
        deps.bump(NodeId(0), 0);
        let b0 = claim(NodeId(1), 0, deps, 2);

        let mut chains = BTreeMap::new();
        // Deliberately store B before A applies structurally — order in the
        // map must not matter, only the dependency graph should.
        chains.insert(NodeId(0), vec![a0.clone()]);
        chains.insert(NodeId(1), vec![b0.clone()]);

        let replay = causal_replay(&chains);
        assert_eq!(replay.applied, vec![a0, b0]);
        assert!(replay.leftover.is_empty());
    }

    #[test]
    fn a_missing_cross_author_dependency_leaves_the_entry_stuck() {
        let mut deps = VersionVector::new();
        deps.bump(NodeId(0), 0); // node 0's genesis, never present in this bundle
        let b0 = claim(NodeId(1), 0, deps, 2);

        let mut chains = BTreeMap::new();
        chains.insert(NodeId(1), vec![b0.clone()]);

        let replay = causal_replay(&chains);
        assert!(replay.applied.is_empty());
        assert_eq!(replay.leftover, vec![(NodeId(1), b0.clone())]);

        let missing = first_missing_dep(&b0, &VersionVector::new());
        assert_eq!(missing, (NodeId(0), 0));
    }
}
