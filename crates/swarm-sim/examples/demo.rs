//! The M0 demo: run it twice with the same seed and diff the output.
//!
//!   cargo run -q --example demo -- --seed 42 > a.txt
//!   cargo run -q --example demo -- --seed 42 > b.txt
//!   diff a.txt b.txt      # empty
//!   cargo run -q --example demo -- --seed 43 | diff a.txt -   # differs
//!
//! Terminal output only. `DESIGN.md` lists GUIs and visualisation as out of scope
//! for the whole of Phase 1.

use swarm_core::NodeId;
use swarm_sim::{run, Partition, SimConfig};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = flag(&args, "--seed").unwrap_or(42);
    let ticks = flag(&args, "--ticks").unwrap_or(120);
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
        beacon_period: 10,
        // Split the swarm, split it further, then heal it. M2 turns this into a
        // convergence claim; at M0 it only demonstrates that the channel model
        // reacts to partitions at all.
        partitions: vec![
            (30, Partition::split(&[&roster[0..2], &roster[2..5]])),
            (
                60,
                Partition::split(&[&roster[0..1], &roster[1..3], &roster[3..5]]),
            ),
            (90, Partition::connected(&roster)),
        ],
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

fn flag(args: &[String], name: &str) -> Option<u64> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1)?.parse().ok()
}
