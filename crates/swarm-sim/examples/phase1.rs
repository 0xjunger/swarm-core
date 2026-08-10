//! The Phase 1 exit demo (`DESIGN.md` §9 criterion #2, `docs/spec.md` §1):
//! one scripted, non-interactive run that tells the whole story —
//! partition, continued work on both sides, a contested claim, healing,
//! convergence, an equivocator caught without consensus, and the invariant
//! checker's verdict.
//!
//!   cargo run -q -p swarm-sim --example phase1
//!
//! Deterministic and diffable: fixed seeds throughout, no `sleep`, no menu,
//! no animation, no wall-clock or map-iteration-order dependence. Running it
//! twice produces byte-identical output. With no flags, output is
//! byte-identical to the pre-M7 version of this example — the flags below
//! are additive.
//!
//! `docs/spec.md` §20.6's two-command exit scenario:
//!
//!   cargo run -p swarm-sim --example phase1 -- --equivocation \
//!       --export-bundle /tmp/run.bundle --export-spec /tmp/mission.spec
//!   cargo run -p swarm-verify -- --bundle /tmp/run.bundle --spec /tmp/mission.spec
//!
//! `--export-bundle <path> --export-spec <path>` write out, as files, the
//! states this same run already computed — no second run, no different
//! code path. `--equivocation` selects *which* of this example's two
//! scenarios gets exported: the honest 5-node cohort (§1-4) by default, or
//! §5's 3-node equivocation scenario when the flag is given. Without
//! `--equivocation`, `swarm-verify` reports every invariant `Satisfied`;
//! with it, I1 `Violated` — the equivocator is node 2 in that scenario's
//! roster.
//!
//! Two separate simulations make up the story. The first (§1-4 below) is 5
//! honest nodes; its own `check_invariants` result is the "final" one printed
//! at §6 — genuinely empty. The second (§5) is a 3-node scenario with one
//! equivocator. They are kept separate on purpose: folding the equivocator's
//! two conflicting genesis entries into the *same* final state map would
//! make `check_i1` report an "I1 violation" for the union of what honest
//! nodes hold — which is not a bug in the checker, it is the checker
//! correctly observing that a signer produced two different signed entries
//! at the same `(node, seq)`. That fact is not something I1 is meant to rule
//! out (a malicious keyholder can always do that); it is what §5's `Poe`
//! mechanism exists to catch and prove instead. Mixing the two stories into
//! one invariant check would either hide that or misreport it — so §6's
//! "empty" reports on the honest cohort, and §5 stands on its own,
//! independently verified, proof.

use std::collections::{BTreeMap, BTreeSet};

use swarm_core::bundle::{LogBundle, Spec};
use swarm_core::fault::verify_poe;
use swarm_core::wire::{Body, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{Envelope, NodeId, State};
use swarm_sim::demo::{BLD, CYN, DIM, GRN, RED, RST, YEL};
use swarm_sim::sim::{build_roster, Equivocation};
use swarm_sim::{run_with_states, Partition, SimConfig, TraceRecord};
use swarm_verify::check_invariants;

const A: NodeId = NodeId(0);
const B: NodeId = NodeId(1);
const C: NodeId = NodeId(2);
const D: NodeId = NodeId(3);
const E: NodeId = NodeId(4);
const HONEST: [NodeId; 5] = [A, B, C, D, E];

// A separate, 3-node roster for §5. Fresh names so nothing is confused with
// the 5-node cast above — this is a different simulation entirely.
const G: NodeId = NodeId(0);
const H: NodeId = NodeId(1);
const F: NodeId = NodeId(2);

/// The three flags this example accepts, all optional and additive — with
/// none given, this example's behaviour and output are unchanged from
/// before M7 (module docs, above).
struct DemoArgs {
    equivocation: bool,
    export_bundle: Option<String>,
    export_spec: Option<String>,
}

fn parse_demo_args() -> DemoArgs {
    let mut args = DemoArgs {
        equivocation: false,
        export_bundle: None,
        export_spec: None,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--equivocation" => args.equivocation = true,
            "--export-bundle" => args.export_bundle = raw.next(),
            "--export-spec" => args.export_spec = raw.next(),
            other => eprintln!("warning: ignoring unrecognised argument '{other}'"),
        }
    }
    args
}

/// Merges every state's own `export_bundle()` into one file-ready
/// `LogBundle` — the same construction `docs/spec.md` §20.2 describes for
/// assembling a whole run out of individual per-node exports.
fn export_bundle_for(states: &BTreeMap<NodeId, State>) -> LogBundle {
    let mut exports = states.values().map(State::export_bundle);
    let first = exports.next().expect("at least one node in the roster");
    exports.fold(first, LogBundle::merge)
}

fn main() {
    let demo_args = parse_demo_args();

    section(1, "Five nodes, connected, claiming tasks");
    println!(
        "{DIM}nodes n0..n4  ·  every node's first claim is task 0 (`next_task`, `swarm-core/src/lib.rs`)  ·  every node contests it{RST}"
    );

    let before = 100u64;
    let after = 80u64;
    let cfg = SimConfig {
        nodes: 5,
        seed: 1,
        ticks: before + after,
        loss_permille: 100,
        delay_min: 1,
        delay_max: 3,
        queue_cap: 256,
        entry_period: 40,
        anti_entropy_period: 5,
        log_cap: 1000,
        buffer_cap: 32,
        partitions: vec![
            (1, Partition::split(&[&[A, B], &[C, D, E]])),
            (before + 1, Partition::connected(&HONEST)),
        ],
        equivocation: None,
        budget_per_node: 3,
    };

    println!(
        "\n{BLD}partition at t=1:{RST}  {YEL}{{n0 n1}}{RST}  |  {YEL}{{n2 n3 n4}}{RST}   {DIM}(heals at t={}){RST}",
        before + 1
    );

    let (trace, states) = run_with_states(&cfg);

    section(2, "Both sides keep working under partition");
    println!(
        "{DIM}first authoring broadcast per (node, seq), t < {before} only — the anti-BFT point: neither side waits for the other{RST}\n"
    );
    let mut seen: BTreeSet<(NodeId, u64)> = BTreeSet::new();
    let mut printed_by_side: BTreeMap<u8, u32> = BTreeMap::new();
    for r in trace.records() {
        if let TraceRecord::Send {
            at,
            from,
            payload: Envelope::Entry(e),
            ..
        } = r
        {
            if *at >= before || e.node != *from || !seen.insert((e.node, e.seq)) {
                continue;
            }
            let side = if *from == A || *from == B { 0 } else { 1 };
            let count = printed_by_side.entry(side).or_insert(0);
            // A handful per side is enough to make the point; the full
            // picture is in the convergence table below.
            if *count >= 3 {
                continue;
            }
            *count += 1;
            let label = match e.body {
                Body::TaskClaim { task, priority } => {
                    format!("claims task {task} (priority {priority})")
                }
                Body::Withdraw { task } => format!("withdraws from task {task}"),
                Body::Spend { amount } => format!("spends {amount}"),
            };
            let group = if side == 0 { "{n0 n1}" } else { "{n2 n3 n4}" };
            println!("  t={at:>3}  {GRN}n{}{RST} {DIM}{group}{RST}  {label}", from.0);
        }
    }

    section(3, "Both sides claim the same task");
    println!(
        "{DIM}task 0's claimants, in the order the healed swarm sees them, and the winner every node converges on{RST}\n"
    );
    let claims: Vec<String> = HONEST
        .iter()
        .flat_map(|n| states[n].claims().claims(0))
        .map(|c| format!("n{}", c.node.0))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    println!("  claimants of task 0: {}", claims.join(" "));
    let winner = states[&A].claims().winner(0);
    println!(
        "  winner rule: min by (priority, logical_clock, node_id)  →  {GRN}{}{RST}",
        winner.map_or("-".into(), |w| format!("n{}", w.node.0))
    );

    section(4, "Heal, converge, and the losers withdraw");
    println!("{DIM}every node's own view after the merge{RST}\n");
    println!("{BLD}  node   entries   winner(task 0)   withdrew task 0{RST}");
    for &n in &HONEST {
        let s = &states[&n];
        let w = s.claims().winner(0).map_or("-".into(), |w| format!("n{}", w.node.0));
        let withdrew = own_bodies(n, &states).contains(&Body::Withdraw { task: 0 });
        println!(
            "  n{}     {:>7}   {GRN}{w:<14}{RST}  {}",
            n.0,
            s.entries().len(),
            if withdrew { format!("{YEL}yes{RST}") } else { "-".to_string() }
        );
    }
    let entry_sets: Vec<BTreeSet<(NodeId, u64)>> = HONEST
        .iter()
        .map(|n| states[n].entries().iter().map(|e| (e.node, e.seq)).collect())
        .collect();
    let converged = entry_sets.windows(2).all(|w| w[0] == w[1]);
    let agree = HONEST
        .iter()
        .all(|n| states[n].claims().winner(0) == winner);
    println!(
        "\n  {}",
        if converged && agree {
            format!("{GRN}{BLD}CONVERGED: yes{RST}  — same entry set, same winner, everywhere")
        } else {
            format!("{RED}{BLD}CONVERGED: no{RST}")
        }
    );
    if !(converged && agree) {
        std::process::exit(1);
    }

    section(5, "An equivocator is caught without consensus");
    println!(
        "{DIM}a separate, smaller run: n2(\"F\") signs two different genesis entries — the genuine one reaches n0(\"G\"), a forged one reaches n1(\"H\"){RST}\n"
    );
    let eq_cfg = SimConfig {
        nodes: 3,
        seed: 1,
        ticks: 60,
        loss_permille: 0,
        delay_min: 1,
        delay_max: 3,
        queue_cap: 256,
        entry_period: 5,
        anti_entropy_period: 20,
        log_cap: 1000,
        buffer_cap: 32,
        partitions: Vec::new(),
        equivocation: Some(Equivocation {
            node: F,
            victims: BTreeSet::from([H]),
        }),
        budget_per_node: 0,
    };
    let (_, eq_states) = run_with_states(&eq_cfg);
    let g_poe = eq_states[&G].poes().find(|p| p.node() == F);
    let h_poe = eq_states[&H].poes().find(|p| p.node() == F);
    match (g_poe, h_poe) {
        (Some(g), Some(h)) => {
            println!(
                "  n0(\"G\") independently proved n2(\"F\") equivocated at seq {}",
                g.seq()
            );
            println!(
                "  n1(\"H\") independently proved n2(\"F\") equivocated at seq {}",
                h.seq()
            );
            println!(
                "  {}",
                if g == h {
                    format!("{GRN}the two proofs are byte-identical{RST}")
                } else {
                    format!("{RED}the two proofs differ — this should not happen{RST}")
                }
            );

            // A third party: no simulator, no trace, no agreement from
            // anyone — just the roster of public keys.
            let roster = build_roster(&[G, H, F]);
            let verdict = verify_poe(&roster, g);
            println!(
                "  {DIM}third party, holding only the roster, verifies n0's proof:{RST}  {}",
                match verdict {
                    Ok(()) => format!("{GRN}accepted{RST}"),
                    Err(e) => format!("{RED}rejected: {e:?}{RST}"),
                }
            );
            if g != h || verdict.is_err() {
                std::process::exit(1);
            }
        }
        _ => {
            println!("{RED}equivocation was not detected — this should not happen{RST}");
            std::process::exit(1);
        }
    }

    section(6, "The verdict — reproducible by a stranger holding only two files");
    println!(
        "{DIM}the same construction §20.6's two-command scenario uses: bundle assembled from each node's own export_bundle(), checked by swarm-verify::verify — no simulator, no live State{RST}\n"
    );
    let honest_bundle = export_bundle_for(&states);
    let honest_spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster: build_roster(&HONEST),
        budgets: cfg.budgets(),
        log_cap: cfg.log_cap as u32,
    };
    let verdict = swarm_verify::verify(&honest_bundle, &honest_spec);
    if verdict.all_satisfied() {
        println!("  {GRN}{BLD}verify: all satisfied{RST}  — I1-I4 all hold, no chain findings");
    } else {
        println!("  {RED}{BLD}verify:{RST} {verdict:?}");
        std::process::exit(1);
    }

    let violations = check_invariants(&states, &cfg.budgets());
    println!(
        "  {DIM}in-process oracle agrees:{RST} {}",
        if violations.is_empty() {
            format!("{GRN}check_invariants: []{RST}")
        } else {
            format!("{RED}check_invariants: {violations:?}{RST}")
        }
    );
    if !violations.is_empty() {
        std::process::exit(1);
    }
    println!();

    if let (Some(bundle_path), Some(spec_path)) =
        (&demo_args.export_bundle, &demo_args.export_spec)
    {
        let (bundle, roster_ids, budgets, log_cap) = if demo_args.equivocation {
            (
                export_bundle_for(&eq_states),
                vec![G, H, F],
                eq_cfg.budgets(),
                eq_cfg.log_cap as u32,
            )
        } else {
            (honest_bundle, HONEST.to_vec(), cfg.budgets(), cfg.log_cap as u32)
        };

        let spec = Spec {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            roster: build_roster(&roster_ids),
            budgets,
            log_cap,
        };

        std::fs::write(bundle_path, bundle.encode())
            .unwrap_or_else(|e| panic!("failed to write bundle to '{bundle_path}': {e}"));
        std::fs::write(spec_path, spec.encode())
            .unwrap_or_else(|e| panic!("failed to write spec to '{spec_path}': {e}"));

        println!(
            "{DIM}exported {} scenario{RST}  bundle -> {bundle_path}  spec -> {spec_path}",
            if demo_args.equivocation { "the equivocation" } else { "the honest" }
        );
    }
}

fn own_bodies(n: NodeId, states: &BTreeMap<NodeId, swarm_core::State>) -> Vec<Body> {
    states[&n].log().entries().iter().map(|e| e.body).collect()
}

fn section(n: u8, title: &str) {
    println!("\n{BLD}{CYN}§{n}{RST} {BLD}{title}{RST}");
    println!("{DIM}{}{RST}", "─".repeat(60));
}
