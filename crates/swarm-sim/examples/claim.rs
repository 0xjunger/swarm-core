//! The M3 demo: two partitions claim the same task, the split heals, and
//! every node independently arrives at the same winner while the losers log
//! their withdrawal.
//!
//!   cargo run -q -p swarm-sim --example claim
//!   cargo run -q -p swarm-sim --example claim -- --seed 7 --before 100 --after 50
//!   cargo run -q -p swarm-sim --example claim -- --quiet   # tables only
//!
//! This is the visible form of `tests/m3_claim.rs`: `DESIGN.md` §9's M3
//! criterion — "İki partisyon aynı görevi talep ediyor, birleşme sonrası her
//! iki node da aynı kazananı hesaplıyor (kimse 'ben kazandım' sanmıyor). Ve
//! kaybeden node'un log'unda geri çekilme kaydı var." — demonstrated rather
//! than asserted.

use std::collections::BTreeSet;

use swarm_core::state::TaskId;
use swarm_core::wire::Body;
use swarm_core::{Envelope, NodeId, State};
use swarm_sim::demo::{flag, pretty_groups, BLD, CYN, DIM, GRN, RED, RST, YEL};
use swarm_sim::{run_with_states, Partition, SimConfig, TraceRecord};

const A: NodeId = NodeId(0);
const B: NodeId = NodeId(1);
const C: NodeId = NodeId(2);
const ALL: [NodeId; 3] = [A, B, C];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = flag(&args, "--seed").unwrap_or(42);
    let before = flag(&args, "--before").unwrap_or(100);
    let after = flag(&args, "--after").unwrap_or(50);
    let quiet = args.iter().any(|a| a == "--quiet");

    // Same timing rationale as `tests/m3_claim.rs`: the last authoring tick
    // must sit far enough from the end that the whole losing sequence —
    // heal, rival claim arrives, next period authors the withdrawal, it
    // propagates — actually completes inside the run.
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
    };

    println!("\n{BLD}swarm-core{RST}  M3 — task-claim CRDT (docs/spec.md §10)");
    println!(
        "{DIM}seed {CYN}{seed}{RST}{DIM}  ·  {{n0 n1}} | {{n2}} for {before} ticks, heal, {after} more  ·  loss 10%{RST}"
    );
    println!("{DIM}winner rule: min by (priority, logical_clock, node_id) — DESIGN.md §4.2{RST}\n");

    let (trace, states) = run_with_states(&cfg);

    if !quiet {
        // Only a node's *own* authoring is interesting here; the gossip that
        // carries it afterwards is `converge`'s subject, not this demo's. An
        // authoring broadcast is the **first** time an entry's own author
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
        })
        .collect::<Vec<_>>()
        .join(" ")
}
