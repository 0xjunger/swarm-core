//! The demo binaries, folded into one dispatcher rather than kept as
//! separate `examples/*.rs` files, to avoid duplicating the setup code each
//! scenario shares. Each scenario is the visible form of an acceptance test
//! already enforced by `cargo test`; running these demonstrates rather than
//! asserts.
//!
//!   cargo run -q -p swarm-sim --example run -- determinism
//!   cargo run -q -p swarm-sim --example run -- converge
//!   cargo run -q -p swarm-sim --example run -- claim
//!   cargo run -q -p swarm-sim --example run -- watch
//!
//! Flags are scenario-specific and documented on each scenario function.

use std::collections::{BTreeMap, BTreeSet};
use swarm_core::state::TaskId;
use swarm_core::wire::{Body, Entry};
use swarm_core::{Envelope, NodeId, State};
use swarm_sim::demo::{envelope_label, flag, pretty_groups, BLD, CYN, DIM, GRN, RED, RST, YEL};
use swarm_sim::{run, run_with_states, Partition, SimConfig, TraceRecord};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scenario = args.get(1).map(String::as_str).unwrap_or("");
    // The scenario name is argv[1]; each scenario re-parses its own flags
    // from the remaining args, same convention each binary used before.
    let rest = &args[1..];

    match scenario {
        "determinism" => determinism(rest),
        "converge" => converge(rest),
        "claim" => claim(rest),
        "watch" => watch(rest),
        other => {
            eprintln!("unknown scenario: {other:?}");
            eprintln!("usage: run -- {{determinism|converge|claim|watch}} [flags]");
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// determinism — the M0 demo: run it twice with the same seed and diff it
// ---------------------------------------------------------------------------

/// ```text
/// cargo run -q -p swarm-sim --example run -- determinism --seed 42 > a.txt
/// cargo run -q -p swarm-sim --example run -- determinism --seed 42 > b.txt
/// diff a.txt b.txt      # empty
/// cargo run -q -p swarm-sim --example run -- determinism --seed 43 | diff a.txt -   # differs
/// ```
///
/// Terminal output only. `DESIGN.md` lists GUIs and visualisation as out of
/// scope for the whole of Phase 1.
fn determinism(args: &[String]) {
    let seed = flag(args, "--seed").unwrap_or(42);
    let ticks = flag(args, "--ticks").unwrap_or(120);
    let summary_only = args.iter().any(|a| a == "--summary");

    let roster: Vec<NodeId> = (0..5).map(NodeId).collect();

    let cfg = SimConfig {
        nodes: 5,
        seed,
        ticks,
        loss_permille: 100,
        delay_min: 1,
        delay_max: 5,
        queue_cap: 64,
        entry_period: 10,
        anti_entropy_period: 15,
        log_cap: 1000,
        buffer_cap: 32,
        // Split the swarm, split it further, then heal it. M2 turns this into
        // a convergence claim, demonstrated end to end by the `converge`
        // scenario.
        partitions: vec![
            (30, Partition::split(&[&roster[0..2], &roster[2..5]])),
            (
                60,
                Partition::split(&[&roster[0..1], &roster[1..3], &roster[3..5]]),
            ),
            (90, Partition::connected(&roster)),
        ],
        equivocation: None,
        budget_per_node: 3,
    };

    let trace = run(&cfg);

    if !summary_only {
        print!("{}", trace.render());
    }

    println!("---");
    println!("seed        {seed}");
    println!("ticks       {ticks}");
    println!("records     {}", trace.records().len());
    println!("digest      {}", trace.digest_hex());
}

// ---------------------------------------------------------------------------
// converge — the M2 demo: partition {A,B} | {C}, run, heal, run more
// ---------------------------------------------------------------------------

const A: NodeId = NodeId(0);
const B: NodeId = NodeId(1);
const C: NodeId = NodeId(2);

/// ```text
/// cargo run -q -p swarm-sim --example run -- converge
/// cargo run -q -p swarm-sim --example run -- converge --seed 7 --before 100 --after 50
/// cargo run -q -p swarm-sim --example run -- converge --quiet   # summary only
/// ```
///
/// This is the visible form of the acceptance test in
/// `tests/m2_convergence.rs`: `DESIGN.md` §9's M2 criterion — "3 node {A,B}
/// ve {C} olarak bölünüyor, 100 tick çalışıyor, birleşiyor, 50 tick daha
/// çalışıyor → üçü de aynı kayıt kümesine sahip" — demonstrated rather than
/// asserted.
fn converge(args: &[String]) {
    let seed = flag(args, "--seed").unwrap_or(42);
    let before = flag(args, "--before").unwrap_or(100);
    let after = flag(args, "--after").unwrap_or(50);
    let quiet = args.iter().any(|a| a == "--quiet");

    // `entry_period` is deliberately coarser than M0/M1's beacon-style
    // default: a periodic creator runs for the whole simulation (a static
    // `SimConfig` has no "stop after tick N"), so the tail between the last
    // entry any node creates and the run's end must be wide enough for it to
    // actually finish propagating — and `anti_entropy_period` must fit
    // several rounds into that tail, since each round is one more chance to
    // close a gap the loss model opened. See `tests/m2_convergence.rs`, which
    // states the measured margins.
    let cfg = SimConfig {
        nodes: 3,
        seed,
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
            (1, Partition::split(&[&[A, B], &[C]])),
            (before + 1, Partition::connected(&[A, B, C])),
        ],
        equivocation: None,
        budget_per_node: 3,
    };

    println!("\n{BLD}swarm-core{RST}  M2 — causal delivery + anti-entropy (docs/spec.md §9)");
    println!(
        "{DIM}seed {CYN}{seed}{RST}{DIM}  ·  {{n0 n1}} | {{n2}} for {before} ticks, heal, {after} more  ·  loss 10%{RST}"
    );
    println!("{DIM}  → sent   ⇒ delivered   ✗ lost   ⊘ blocked by partition   ✓ applied   ⋯ buffered{RST}\n");

    let (trace, states) = run_with_states(&cfg);

    if !quiet {
        for r in trace.records() {
            match r {
                TraceRecord::Partition { at, groups } => {
                    println!(
                        "{DIM}────{RST} t={BLD}{at:>3}{RST}  {YEL}{BLD}PARTITION{RST} {YEL}{}{RST}",
                        pretty_groups(groups)
                    );
                }
                TraceRecord::Deliver {
                    from, to, payload, ..
                } => {
                    println!(
                        "  {GRN}n{} ⇒ n{}{RST}   {DIM}{}{RST}",
                        from.0,
                        to.0,
                        envelope_label(payload)
                    );
                }
                TraceRecord::DropLoss { from, to, .. } => {
                    println!(
                        "  {RED}n{} ✗ n{}{RST}   {DIM}lost in transit{RST}",
                        from.0, to.0
                    );
                }
                TraceRecord::DropPartition { from, to, .. } => {
                    println!(
                        "  {YEL}n{} ⊘ n{}{RST}   {DIM}was in the air when the link died{RST}",
                        from.0, to.0
                    );
                }
                TraceRecord::DropCausalOverflow {
                    node, origin, seq, ..
                } => {
                    println!(
                        "  {YEL}n{} ⊗{RST}        {DIM}causal buffer full, dropped n{}#{}{RST}",
                        node.0, origin.0, seq
                    );
                }
                TraceRecord::Apply {
                    node, origin, seq, ..
                } => {
                    println!(
                        "  {GRN}n{} ✓{RST}        {DIM}applied n{}#{}{RST}",
                        node.0, origin.0, seq
                    );
                }
                TraceRecord::Buffer {
                    node, origin, seq, ..
                } => {
                    println!(
                        "  {YEL}n{} ⋯{RST}        {DIM}buffered n{}#{}, deps not met yet{RST}",
                        node.0, origin.0, seq
                    );
                }
                _ => {}
            }
        }
        println!();
    }

    println!("{BLD}  node   entries   version vector{RST}");
    for n in [A, B, C] {
        let s = &states[&n];
        let vv: Vec<String> = s
            .causal_vv()
            .iter()
            .map(|(o, seq)| format!("n{}={seq}", o.0))
            .collect();
        println!("  n{}   {:>7}   {}", n.0, s.entries().len(), vv.join(" "));
    }

    let maps: BTreeMap<NodeId, BTreeMap<(NodeId, u64), Entry>> = [A, B, C]
        .into_iter()
        .map(|n| {
            let m: BTreeMap<(NodeId, u64), Entry> = states[&n]
                .entries()
                .into_iter()
                .map(|e| ((e.node, e.seq), e.clone()))
                .collect();
            (n, m)
        })
        .collect();

    let converged = maps[&A] == maps[&B] && maps[&B] == maps[&C];
    println!();
    if converged {
        println!("{GRN}{BLD}CONVERGED: yes{RST}  — all three nodes hold the same entry set");
    } else {
        println!("{RED}{BLD}CONVERGED: no{RST}");
        for &n in &[A, B, C] {
            let missing: Vec<(NodeId, u64)> = maps
                .values()
                .flat_map(|m| m.keys())
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|k| !maps[&n].contains_key(k))
                .collect();
            if !missing.is_empty() {
                println!("  n{} is missing: {missing:?}", n.0);
            }
        }
        std::process::exit(1);
    }
    println!();
}

// ---------------------------------------------------------------------------
// claim — the M3 demo: two partitions claim the same task, then heal
// ---------------------------------------------------------------------------

const ALL: [NodeId; 3] = [A, B, C];

/// ```text
/// cargo run -q -p swarm-sim --example run -- claim
/// cargo run -q -p swarm-sim --example run -- claim --seed 7 --before 100 --after 50
/// cargo run -q -p swarm-sim --example run -- claim --quiet   # tables only
/// ```
///
/// This is the visible form of `tests/m3_claim.rs`: `DESIGN.md` §9's M3
/// criterion — "İki partisyon aynı görevi talep ediyor, birleşme sonrası her
/// iki node da aynı kazananı hesaplıyor (kimse 'ben kazandım' sanmıyor). Ve
/// kaybeden node'un log'unda geri çekilme kaydı var." — demonstrated rather
/// than asserted.
fn claim(args: &[String]) {
    let seed = flag(args, "--seed").unwrap_or(42);
    let before = flag(args, "--before").unwrap_or(100);
    let after = flag(args, "--after").unwrap_or(50);
    let quiet = args.iter().any(|a| a == "--quiet");

    // Same timing rationale as `tests/m3_claim.rs`: the last authoring tick
    // must sit far enough from the end that the whole losing sequence — heal,
    // rival claim arrives, next period authors the withdrawal, it propagates
    // — actually completes inside the run.
    let cfg = SimConfig {
        nodes: 3,
        seed,
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
            (1, Partition::split(&[&[A, B], &[C]])),
            (before + 1, Partition::connected(&[A, B, C])),
        ],
        equivocation: None,
        budget_per_node: 3,
    };

    println!("\n{BLD}swarm-core{RST}  M3 — task-claim CRDT (docs/spec.md §10)");
    println!(
        "{DIM}seed {CYN}{seed}{RST}{DIM}  ·  {{n0 n1}} | {{n2}} for {before} ticks, heal, {after} more  ·  loss 10%{RST}"
    );
    println!("{DIM}winner rule: min by (priority, logical_clock, node_id) — DESIGN.md §4.2{RST}\n");

    let (trace, states) = run_with_states(&cfg);

    if !quiet {
        // Only a node's *own* authoring is interesting here; the gossip that
        // carries it afterwards is `converge`'s subject, not this scenario's.
        // An authoring broadcast is the **first** time an entry's own author
        // sends it — every later send of the same `(origin, seq)` is an
        // anti-entropy fill reply (`docs/spec.md` §9.5).
        let mut seen: BTreeSet<(NodeId, u64)> = BTreeSet::new();
        for r in trace.records() {
            match r {
                TraceRecord::Partition { at, groups } => println!(
                    "{DIM}────{RST} t={BLD}{at:>3}{RST}  {YEL}{BLD}PARTITION{RST} {YEL}{}{RST}",
                    pretty_groups(groups)
                ),
                TraceRecord::Send {
                    at,
                    from,
                    payload: Envelope::Entry(e),
                    ..
                } if e.node == *from && seen.insert((e.node, e.seq)) => match e.body {
                    Body::TaskClaim { task, priority } => println!(
                        "  t={at:>3}  {GRN}n{} claims task {task}{RST}  {DIM}(seq {}, priority {priority}){RST}",
                        from.0, e.seq
                    ),
                    Body::Withdraw { task } => println!(
                        "  t={at:>3}  {YEL}n{} withdraws from task {task}{RST}  {DIM}(seq {}, lost the contest){RST}",
                        from.0, e.seq
                    ),
                    Body::Spend { amount } => println!(
                        "  t={at:>3}  {CYN}n{} spends {amount}{RST}  {DIM}(seq {}){RST}",
                        from.0, e.seq
                    ),
                },
                _ => {}
            }
        }
        println!();
    }

    // Every task anyone knows about, and what each node thinks of it.
    let tasks: BTreeSet<TaskId> = ALL
        .iter()
        .flat_map(|n| states[n].claims().tasks())
        .collect();

    println!("{BLD}  task   claims (prio, lc, node)                    winner   withdrew{RST}");
    for &task in &tasks {
        let claims: Vec<String> = states[&A]
            .claims()
            .claims(task)
            .map(|c| format!("({},{},n{})", c.priority, c.lc, c.node.0))
            .collect();
        let winner = states[&A].claims().winner(task);
        let withdrew: Vec<String> = ALL
            .iter()
            .filter(|n| states[&A].claims().has_withdrawn(task, **n))
            .map(|n| format!("n{}", n.0))
            .collect();
        println!(
            "  {task:>4}   {:<42} {GRN}{:<8}{RST} {YEL}{}{RST}",
            claims.join(" "),
            winner.map_or("-".into(), |w| format!("n{}", w.node.0)),
            withdrew.join(" ")
        );
    }

    println!("\n{BLD}  node   own log (its own signed chain){RST}");
    for n in ALL {
        println!("  n{}     {}", n.0, own_log(&states[&n]));
    }

    // The criterion itself.
    let mut ok = true;
    println!();
    for &task in &tasks {
        let winners: Vec<_> = ALL
            .iter()
            .map(|n| states[n].claims().winner(task))
            .collect();
        if !winners.iter().all(|w| *w == winners[0]) {
            println!("{RED}  task {task}: nodes disagree about the winner{RST}");
            ok = false;
        }
        let believers: Vec<NodeId> = ALL
            .into_iter()
            .filter(|n| {
                states[n]
                    .claims()
                    .winner(task)
                    .is_some_and(|w| w.node == *n)
            })
            .collect();
        if believers.len() != 1 {
            println!(
                "{RED}  task {task}: {} nodes believe they won ({believers:?}){RST}",
                believers.len()
            );
            ok = false;
        }
    }

    // Task 0 is the contested one: every node's first claim, authored inside
    // the partition, so both sides bid for it without seeing each other.
    let winner = states[&A].claims().winner(0).map(|w| w.node);
    for n in ALL {
        let logged = own_bodies(&states[&n]).contains(&Body::Withdraw { task: 0 });
        if logged != (Some(n) != winner) {
            println!(
                "{RED}  n{}: withdrawal record for task 0 is wrong{RST}",
                n.0
            );
            ok = false;
        }
    }

    if ok {
        println!("{GRN}{BLD}AGREED: yes{RST}  — every node names the same winner for every task,");
        println!("             exactly one node believes it won each, and every loser of the");
        println!("             contested task 0 has a withdrawal record in its own log.");
    } else {
        println!("{RED}{BLD}AGREED: no{RST}");
        std::process::exit(1);
    }
    println!();
}

fn own_bodies(state: &State) -> Vec<Body> {
    state.log().entries().iter().map(|e| e.body).collect()
}

/// A node's own chain as a compact string: `c0 c1 w0 c2 …`.
fn own_log(state: &State) -> String {
    own_bodies(state)
        .iter()
        .map(|b| match b {
            Body::TaskClaim { task, .. } => format!("claim{task}"),
            Body::Withdraw { task } => format!("wdraw{task}"),
            Body::Spend { amount } => format!("spend{amount}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// watch — a minimal terminal view of one run
// ---------------------------------------------------------------------------

/// ```text
/// cargo run -q -p swarm-sim --example run -- watch                # seed 42, 12 ticks
/// cargo run -q -p swarm-sim --example run -- watch --ticks 40    # far enough to see a partition
/// cargo run -q -p swarm-sim --example run -- watch --step        # press Enter per tick
/// ```
///
/// This reads nothing but the public API of `swarm-sim` plus its shared
/// `demo` formatting helpers (`crates/swarm-sim/src/demo.rs`).
fn watch(args: &[String]) {
    use std::io::BufRead;

    let seed = flag(args, "--seed").unwrap_or(42);
    let ticks = flag(args, "--ticks").unwrap_or(12);
    let step = args.iter().any(|a| a == "--step");

    let roster: Vec<NodeId> = (0..5).map(NodeId).collect();
    let cfg = SimConfig {
        nodes: 5,
        seed,
        ticks,
        loss_permille: 100,
        delay_min: 1,
        delay_max: 5,
        queue_cap: 64,
        entry_period: 10,
        anti_entropy_period: 15,
        log_cap: 1000,
        buffer_cap: 32,
        partitions: vec![
            (30, Partition::split(&[&roster[0..2], &roster[2..5]])),
            (
                60,
                Partition::split(&[&roster[0..1], &roster[1..3], &roster[3..5]]),
            ),
            (90, Partition::connected(&roster)),
        ],
        equivocation: None,
        budget_per_node: 3,
    };

    println!("\n{BLD}swarm-core{RST}  seed {CYN}{seed}{RST}  ·  5 nodes  ·  {ticks} ticks  ·  entry every 10, anti-entropy every 15  ·  loss 10%");
    println!("{DIM}  → sent   ⇒ delivered   ✗ lost   ⊘ blocked by partition   ⊗ queue full   ✓ applied   ⋯ buffered{RST}");

    let trace = run(&cfg);
    let mut inflight = 0i32;
    let mut last_label = String::new(); // description of the last SEND, for the Enqueue/DropLoss lines that follow it
    let mut open = false;

    for r in trace.records() {
        match r {
            TraceRecord::Tick { at } => {
                if open && step {
                    let _ = std::io::stdin().lock().read_line(&mut String::new());
                }
                open = true;
                println!(
                    "\n{DIM}────{RST} t={BLD}{at:>3}{RST} {DIM}{}{RST}  {}",
                    "─".repeat(46),
                    bar(inflight)
                );
            }
            TraceRecord::Partition { groups, .. } => {
                println!(
                    "  {YEL}{BLD}PARTITION{RST}  {YEL}{}{RST}",
                    pretty_groups(groups)
                );
            }
            TraceRecord::Send { payload: p, .. } => last_label = envelope_label(p),
            TraceRecord::Enqueue {
                at, due, from, to, ..
            } => {
                inflight += 1;
                println!(
                    "  n{} → n{}   {DIM}{last_label}{RST} {DIM}arrives t={due} (+{}){RST}",
                    from.0,
                    to.0,
                    due - at
                );
            }
            TraceRecord::Deliver {
                from,
                to,
                payload: p,
                ..
            } => {
                inflight -= 1;
                println!(
                    "  {GRN}n{} ⇒ n{}{RST}   {DIM}{}{RST}",
                    from.0,
                    to.0,
                    envelope_label(p)
                );
            }
            TraceRecord::DropLoss { from, to, .. } => {
                println!(
                    "  {RED}n{} ✗ n{}{RST}   {DIM}{last_label} lost in transit{RST}",
                    from.0, to.0
                );
            }
            TraceRecord::DropPartition { from, to, .. } => {
                inflight -= 1;
                println!(
                    "  {YEL}n{} ⊘ n{}{RST}   {DIM}was in the air when the link died{RST}",
                    from.0, to.0
                );
            }
            TraceRecord::DropOverflow { to, .. } => {
                inflight -= 1;
                println!(
                    "  {YEL}n{} ⊗{RST}        {DIM}queue full, oldest dropped{RST}",
                    to.0
                );
            }
            TraceRecord::Apply {
                node, origin, seq, ..
            } => {
                println!(
                    "  {GRN}n{} ✓{RST}        {DIM}applied n{}#{}{RST}",
                    node.0, origin.0, seq
                );
            }
            TraceRecord::Buffer {
                node, origin, seq, ..
            } => {
                println!(
                    "  {YEL}n{} ⋯{RST}        {DIM}buffered n{}#{}, deps not met yet{RST}",
                    node.0, origin.0, seq
                );
            }
            TraceRecord::DropCausalOverflow {
                node, origin, seq, ..
            } => {
                println!(
                    "  {YEL}n{} ⊗{RST}        {DIM}causal buffer full, dropped n{}#{}{RST}",
                    node.0, origin.0, seq
                );
            }
            TraceRecord::Final { node, recv, sent } => {
                if node.0 == 0 {
                    println!("\n{BLD}  node   sent   recv{RST}");
                }
                println!("  n{}   {sent:>6} {recv:>6}", node.0);
            }
            TraceRecord::Equivocation {
                witness,
                accused,
                seq,
                ..
            } => {
                println!(
                    "  {RED}{BLD}n{} PROVED n{} equivocated{RST}  {DIM}at seq {seq}{RST}",
                    witness.0, accused.0
                );
            }
        }
    }

    println!(
        "\n  {DIM}records{RST} {}   {DIM}digest{RST} {CYN}{}{RST}\n",
        trace.records().len(),
        &trace.digest_hex()[..32]
    );
}

/// A bar showing how many messages are currently in the air.
fn bar(n: i32) -> String {
    let n = n.max(0) as usize;
    format!(
        "{DIM}in flight{RST} {n:>3} {CYN}{}{RST}",
        "▍".repeat(n.min(40))
    )
}
