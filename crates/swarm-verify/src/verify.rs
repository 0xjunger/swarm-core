//! `verify(bundle, spec) -> Verdict` (`docs/spec.md` §20.5): the standalone
//! judge. No simulator, no live `State`, no access to the process that
//! produced `bundle` — only the bytes in `bundle`, checked against the
//! rules in `spec`.

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
    docs/spec.md §15.";

/// One bundle's chains that survived structural verification, keyed the
/// same way `LogBundle::views` is: observer, then author.
type VerifiedChains = BTreeMap<NodeId, BTreeMap<NodeId, Vec<Entry>>>;

/// One observer's replay and the claims folded from it — everything the
/// invariant checks below need, computed once per observer.
struct ObserverState {
    replay: Replay,
    claims: BTreeMap<TaskId, Vec<Claim>>,
}

/// Checks `bundle` against `spec` and returns a [`Verdict`] (`docs/spec.md`
/// §20.5). Never reads anything but its two arguments.
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
/// `verify_chain` (§8.3), then `spec.log_cap`. Chains that pass are carried
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
/// `swarm_core::state::Claims` (`docs/spec.md` §20.5).
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
/// `Claim`'s derived `Ord`, exactly `DESIGN.md` §4.2's rule — restated here,
/// not called from `swarm_core::state::Claims::winner`.
fn winner(claims: &BTreeMap<TaskId, Vec<Claim>>, task: TaskId) -> Option<Claim> {
    claims.get(&task).and_then(|v| v.iter().min().copied())
}

/// I1: at most one distinct signed entry per `(author, seq)`, across every
/// chain-verified entry in the whole bundle. `Undetermined` only when the
/// bundle holds no chain-verified entries at all.
fn check_i1(chains_ok: &VerifiedChains, roster: &Roster) -> InvariantResult {
    let mut by_key: BTreeMap<(NodeId, u64), Vec<Entry>> = BTreeMap::new();
    for chains in chains_ok.values() {
        for entries in chains.values() {
            for entry in entries {
                by_key.entry((entry.node, entry.seq)).or_default().push(entry.clone());
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
            let tasks: BTreeSet<TaskId> = sa.claims.keys().chain(sb.claims.keys()).copied().collect();
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
                    spend_entries.entry(entry.node).or_default().push(entry.clone());
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
