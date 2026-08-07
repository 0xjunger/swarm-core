//! The M2 demo: partition `{A,B} | {C}`, run, heal, run more, and check
//! that every node ends up holding the same entry set.
//!
//!   cargo run -q -p swarm-sim --example converge
//!   cargo run -q -p swarm-sim --example converge -- --seed 7 --before 100 --after 50
//!   cargo run -q -p swarm-sim --example converge -- --quiet   # summary only
//!
//! This is the visible form of the acceptance test in
//! `tests/m2_convergence.rs`: `DESIGN.md` §9's M2 criterion — "3 node
//! {A,B} ve {C} olarak bölünüyor, 100 tick çalışıyor, birleşiyor, 50 tick
//! daha çalışıyor → üçü de aynı kayıt kümesine sahip" — demonstrated
//! rather than asserted.

use std::collections::BTreeMap;

use swarm_core::wire::Entry;
use swarm_core::NodeId;
use swarm_sim::demo::{envelope_label, flag, pretty_groups, BLD, CYN, DIM, GRN, RED, RST, YEL};
use swarm_sim::{run_with_states, Partition, SimConfig, TraceRecord};

const A: NodeId = NodeId(0);
const B: NodeId = NodeId(1);
const C: NodeId = NodeId(2);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = flag(&args, "--seed").unwrap_or(42);
    let before = flag(&args, "--before").unwrap_or(100);
    let after = flag(&args, "--after").unwrap_or(50);
    let quiet = args.iter().any(|a| a == "--quiet");

    // `entry_period` is deliberately coarser than M0/M1's beacon-style
    // default: a periodic creator runs for the whole simulation (a static
    // `SimConfig` has no "stop after tick N"), so the tail between the
    // last entry any node creates and the run's end must be wide enough
    // for it to actually finish propagating — and `anti_entropy_period` must
    // fit several rounds into that tail, since each round is one more chance
    // to close a gap the loss model opened. See `tests/m2_convergence.rs`,
    // which states the measured margins.
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
                .collect::<std::collections::BTreeSet<_>>()
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
