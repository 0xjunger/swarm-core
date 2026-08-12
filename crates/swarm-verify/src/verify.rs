//! `verify(bundle, spec) -> Verdict` (`SPEC.md` §7.1): the standalone
//! judge, and `swarm-verify`'s normative surface. No simulator, no live
//! `State`, no access to the process that produced `bundle` — only the
//! bytes in `bundle`, checked against the rules in `spec`. See
//! `crate::oracle`'s module doc for why an entirely separate in-process
//! checker also exists and is not this.

use std::collections::{BTreeMap, BTreeSet};

use swarm_core::bundle::{LogBundle, Spec};
use swarm_core::fault::{verify_poe, Poe};
use swarm_core::log::verify_chain;
use swarm_core::state::{Claim, TaskId};
use swarm_core::wire::{Body, Entry, Roster};
use swarm_core::NodeId;

use crate::fold::{causal_replay, first_missing_dep, Replay};
use crate::verdict::{ChainFinding, ChainProblem, InvariantResult, Verdict, Witness};

const STRUCTURAL_NOTE: &str = "I5/I6 are structural properties of swarm-core's source \
    (policy.rs), not something a log of signed entries can attest to either way — see \
    SPEC.md §6.5-§6.6.";

/// One bundle's chains that survived structural verification, keyed the
/// same way `LogBundle::views` is: observer, then author.
type VerifiedChains = BTreeMap<NodeId, BTreeMap<NodeId, Vec<Entry>>>;

/// One observer's replay and the claims folded from it — everything the
/// invariant checks below need, computed once per observer.
struct ObserverState {
    replay: Replay,
    claims: BTreeMap<TaskId, Vec<Claim>>,
}

/// Checks `bundle` against `spec` and returns a [`Verdict`] (`SPEC.md`
/// §7.1). Never reads anything but its two arguments.
pub fn verify(bundle: &LogBundle, spec: &Spec) -> Verdict {
    let (chains_ok, chain_findings) = verify_chains(bundle, spec);

    let per_observer: BTreeMap<NodeId, ObserverState> = chains_ok
        .iter()
        .map(|(&observer, chains)| (observer, replay_observer(chains)))
        .collect();

    Verdict {
        chains: chain_findings,
        i1: check_i1(&chains_ok, &spec.roster),
        i2: check_i2(&per_observer),
        i3: check_i3(&per_observer),
        i4: check_i4(&per_observer, &spec.budgets),
        structural_note: STRUCTURAL_NOTE,
        input_attestable: false,
    }
}

/// Step 1: for each `(observer, author)` chain — the misfiling check, then
/// `verify_chain` (§4.2), then `spec.log_cap`. Chains that pass are carried
/// forward; chains that fail any of the three are reported directly in
/// `Verdict::chains` and excluded from every further check — malformed
/// evidence is not evidence for or against an invariant.
///
/// The misfiling check runs first: `chains`' key is `author`, but nothing
/// about `LogBundle`'s shape guarantees the entries filed under that key
/// actually claim it — `verify_chain` only ever sees a slice of entries, not
/// the key it was stored under, so it cannot catch a chain filed under the
/// wrong author. A bundle could otherwise carry a genuine equivocation whose
/// second chain is filed under a different author key, evading I1's
/// `(author, seq)` grouping entirely. Checked before `verify_chain` and
/// `log_cap` so a misfiled chain is reported as misfiled even when it is
/// also invalid or too long for another reason.
fn verify_chains(bundle: &LogBundle, spec: &Spec) -> (VerifiedChains, Vec<ChainFinding>) {
    let mut chains_ok = BTreeMap::new();
    let mut findings = Vec::new();

    for (&observer, chains) in &bundle.views {
        let mut ok_chains = BTreeMap::new();
        for (&author, entries) in chains {
            if let Some(first) = entries.first() {
                if first.node != author {
                    findings.push(ChainFinding {
                        observer,
                        author,
                        error: ChainProblem::Misfiled {
                            declared: author,
                            actual: first.node,
                        },
                        entries: entries.clone(),
                    });
                    continue;
                }
            }
            if entries.len() as u64 > spec.log_cap as u64 {
                findings.push(ChainFinding {
                    observer,
                    author,
                    error: ChainProblem::TooLong {
                        cap: spec.log_cap,
                        actual: entries.len(),
                    },
                    entries: entries.clone(),
                });
                continue;
            }
            match verify_chain(&spec.roster, entries) {
                Ok(_) => {
                    ok_chains.insert(author, entries.clone());
                }
                Err(e) => findings.push(ChainFinding {
                    observer,
                    author,
                    error: ChainProblem::Chain(e),
                    entries: entries.clone(),
                }),
            }
        }
        chains_ok.insert(observer, ok_chains);
    }

    (chains_ok, findings)
}

/// Step 2: replay one observer's chain-verified chains to a fixed point and
/// fold `TaskClaim`s into `swarm-verify`'s own claim map — never
/// `swarm_core::state::Claims` (`SPEC.md` §7.2).
fn replay_observer(chains: &BTreeMap<NodeId, Vec<Entry>>) -> ObserverState {
    let replay = causal_replay(chains);
    let mut claims: BTreeMap<TaskId, Vec<Claim>> = BTreeMap::new();
    for entry in &replay.applied {
        if let Body::TaskClaim { task, priority } = entry.body {
            claims.entry(task).or_default().push(Claim {
                priority,
                lc: entry.deps.entry_count(),
                node: entry.node,
                seq: entry.seq,
            });
        }
    }
    ObserverState { replay, claims }
}

/// The winner of `task` under `swarm-verify`'s own claim map: `min` by
/// `Claim`'s derived `Ord`, exactly `SPEC.md` §6.3's rule — restated here,
/// not called from `swarm_core::state::Claims::winner`.
fn winner(claims: &BTreeMap<TaskId, Vec<Claim>>, task: TaskId) -> Option<Claim> {
    claims.get(&task).and_then(|v| v.iter().min().copied())
}

/// I1: at most one distinct signed entry per `(author, seq)`, across every
/// chain-verified entry in the whole bundle. `Undetermined` only when the
/// bundle holds no chain-verified entries at all.
///
/// Keyed by `entry.node`, the signer — not by the bundle's map key — so the
/// grouping cannot be evaded by filing a chain under the wrong author (X1).
/// `mutant-verify-i1` (off by default) reintroduces exactly that false
/// negative, keying by the map key instead. `verify_chains` (above) now
/// excludes any chain whose map key disagrees with its signer before this
/// function ever sees it, so in the full pipeline the two keyings are
/// provably identical — the mutant is unreachable via `verify` end to end.
/// `tests::check_i1_groups_by_the_signer_not_by_the_map_key`, in this
/// module, calls this function directly to test the property on its own.
fn check_i1(chains_ok: &VerifiedChains, roster: &Roster) -> InvariantResult {
    let mut by_key: BTreeMap<(NodeId, u64), Vec<Entry>> = BTreeMap::new();
    for chains in chains_ok.values() {
        #[cfg(not(feature = "mutant-verify-i1"))]
        for entries in chains.values() {
            for entry in entries {
                by_key
                    .entry((entry.node, entry.seq))
                    .or_default()
                    .push(entry.clone());
            }
        }
        #[cfg(feature = "mutant-verify-i1")]
        for (&author, entries) in chains {
            for entry in entries {
                by_key
                    .entry((author, entry.seq))
                    .or_default()
                    .push(entry.clone());
            }
        }
    }
    if by_key.is_empty() {
        return InvariantResult::Undetermined("no chain-verified entries in the bundle");
    }

    for entries in by_key.values() {
        for i in 0..entries.len() {
            for j in (i + 1)..entries.len() {
                if entries[i].encoded() == entries[j].encoded() {
                    continue;
                }
                if let Some(poe) = Poe::new(entries[i].clone(), entries[j].clone()) {
                    if verify_poe(roster, &poe).is_ok() {
                        return InvariantResult::Violated(Box::new(Witness::Equivocation(poe)));
                    }
                }
            }
        }
    }
    InvariantResult::Satisfied
}

/// I2: an entry an observer holds must be reachable by that same observer's
/// own causal fixed point. `Undetermined` when the bundle has no observers.
fn check_i2(per_observer: &BTreeMap<NodeId, ObserverState>) -> InvariantResult {
    if per_observer.is_empty() {
        return InvariantResult::Undetermined("no observers in the bundle");
    }
    for (&observer, state) in per_observer {
        if let Some((_, entry)) = state.replay.leftover.first() {
            let missing = first_missing_dep(entry, &state.replay.final_vv);
            return InvariantResult::Violated(Box::new(Witness::UnmetDependency {
                observer,
                entry: entry.clone(),
                missing,
            }));
        }
    }
    InvariantResult::Satisfied
}

/// I3: every pair of observers whose applied `(author, seq)` key-sets match
/// exactly must derive the same `winner(task)` for every task either has
/// claims for. `Undetermined` when fewer than two observers are present, or
/// when no two observers' key-sets coincide — there is nothing to compare
/// either way, and reporting `Satisfied` on zero comparisons would claim
/// evidence the bundle does not contain.
fn check_i3(per_observer: &BTreeMap<NodeId, ObserverState>) -> InvariantResult {
    if per_observer.len() < 2 {
        return InvariantResult::Undetermined("fewer than two observers to compare");
    }

    let key_set = |o: &NodeId| -> BTreeSet<(NodeId, u64)> {
        per_observer[o]
            .replay
            .applied
            .iter()
            .map(|e| (e.node, e.seq))
            .collect()
    };

    let observers: Vec<NodeId> = per_observer.keys().copied().collect();
    let mut compared_any = false;

    for i in 0..observers.len() {
        for j in (i + 1)..observers.len() {
            let (a, b) = (observers[i], observers[j]);
            if key_set(&a) != key_set(&b) {
                continue;
            }
            compared_any = true;

            let sa = &per_observer[&a];
            let sb = &per_observer[&b];
            let tasks: BTreeSet<TaskId> =
                sa.claims.keys().chain(sb.claims.keys()).copied().collect();
            for task in tasks {
                let winner_a = winner(&sa.claims, task);
                let winner_b = winner(&sb.claims, task);
                if winner_a != winner_b {
                    return InvariantResult::Violated(Box::new(Witness::Divergence {
                        a,
                        b,
                        task,
                        winner_a,
                        winner_b,
                    }));
                }
            }
        }
    }

    if compared_any {
        InvariantResult::Satisfied
    } else {
        InvariantResult::Undetermined("no two observers share an identical applied entry set")
    }
}

/// I4: per node, the sum of every distinct `Spend` entry (deduped by
/// `(author, seq)` across observers, the way the same entry can legitimately
/// appear in more than one observer's view) must not exceed
/// `spec.budgets[node]`. `Undetermined` when the bundle has no observers.
fn check_i4(
    per_observer: &BTreeMap<NodeId, ObserverState>,
    budgets: &BTreeMap<NodeId, u64>,
) -> InvariantResult {
    if per_observer.is_empty() {
        return InvariantResult::Undetermined("no observers in the bundle");
    }

    let mut seen: BTreeSet<(NodeId, u64)> = BTreeSet::new();
    let mut spent: BTreeMap<NodeId, u64> = BTreeMap::new();
    let mut spend_entries: BTreeMap<NodeId, Vec<Entry>> = BTreeMap::new();

    for state in per_observer.values() {
        for entry in &state.replay.applied {
            if let Body::Spend { amount } = entry.body {
                if seen.insert((entry.node, entry.seq)) {
                    *spent.entry(entry.node).or_insert(0) += amount;
                    spend_entries
                        .entry(entry.node)
                        .or_default()
                        .push(entry.clone());
                }
            }
        }
    }

    for (&node, &total) in &spent {
        let budget = budgets.get(&node).copied().unwrap_or(0);
        if total > budget {
            return InvariantResult::Violated(Box::new(Witness::Overspend {
                node,
                budget,
                entries: spend_entries.remove(&node).unwrap_or_default(),
            }));
        }
    }
    InvariantResult::Satisfied
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use swarm_core::causal::VersionVector;
    use swarm_core::wire::{Hash, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    fn entry_at(node: NodeId, seq: u64, task: u64, k: &SigningKey) -> Entry {
        UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node,
            seq,
            prev: Hash::ZERO,
            deps: VersionVector::new(),
            body: Body::TaskClaim { task, priority: 1 },
        }
        .sign(k)
    }

    /// `check_i1` in isolation, independent of `verify_chains`' misfiling
    /// filter (X1's other half). That filter now runs unconditionally, so it
    /// already excludes any chain whose map key disagrees with its entries'
    /// signer before `check_i1` ever sees it — which means, in the full
    /// `verify` pipeline, `check_i1`'s own choice of grouping key is
    /// unreachable: every entry that survives `verify_chains` already has
    /// `entry.node == author`. This test bypasses `verify_chains` entirely
    /// and calls `check_i1` directly with a hand-built `VerifiedChains` map
    /// whose key does not match its entries' signer — a shape a real bundle
    /// can no longer produce, but a property `check_i1` should hold on its
    /// own regardless. `mutant-verify-i1` breaks exactly that property; this
    /// is the only test able to observe the difference, since the full
    /// pipeline masks it (`SPEC.md` §7.2).
    #[test]
    fn check_i1_groups_by_the_signer_not_by_the_map_key() {
        let (kg, kf) = (key(1), key(2));
        let (g, h, f, d) = (NodeId(0), NodeId(1), NodeId(2), NodeId(3));
        let genuine = entry_at(f, 0, 1, &kf);
        let forged = entry_at(f, 0, 2, &kf);
        assert_ne!(genuine.encoded(), forged.encoded());

        let mut chains_ok: VerifiedChains = BTreeMap::new();
        chains_ok.insert(g, BTreeMap::from([(f, vec![genuine])]));
        // A shape `verify_chains` no longer lets through: filed under `d`,
        // signed by `f`.
        chains_ok.insert(h, BTreeMap::from([(d, vec![forged])]));

        let mut keys = BTreeMap::new();
        keys.insert(f, kf.verifying_key());
        keys.insert(g, kg.verifying_key());
        let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

        let result = check_i1(&chains_ok, &roster);
        assert!(
            matches!(result, InvariantResult::Violated(_)),
            "expected I1 to catch the equivocation by grouping on the signer, got {result:?}"
        );
    }
}
