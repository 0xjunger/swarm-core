//! A minimal terminal view of one run.
//!
//!   cargo run -q --example watch                    # seed 42, 12 ticks
//!   cargo run -q --example watch -- --ticks 40      # far enough to see a partition
//!   cargo run -q --example watch -- --step          # press Enter per tick
//!
//! This reads nothing but the public API of `swarm-sim`. It adds no code to the
//! library and the library does not know it exists.

use std::io::BufRead;
use swarm_core::wire::Body;
use swarm_core::{Envelope, NodeId};
use swarm_sim::{run, Partition, SimConfig, TraceRecord};

const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GRN: &str = "\x1b[32m";
const YEL: &str = "\x1b[33m";
const CYN: &str = "\x1b[36m";
const BLD: &str = "\x1b[1m";
const RST: &str = "\x1b[0m";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seed = flag(&args, "--seed").unwrap_or(42);
    let ticks = flag(&args, "--ticks").unwrap_or(12);
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
        }
    }

    println!(
        "\n  {DIM}records{RST} {}   {DIM}digest{RST} {CYN}{}{RST}\n",
        trace.records().len(),
        &trace.digest_hex()[..32]
    );
}

/// A short human label for what an envelope carries.
fn envelope_label(e: &Envelope) -> String {
    match e {
        Envelope::Entry(entry) => match entry.body {
            Body::TaskClaim { task, .. } => {
                format!("n{}#{} claim t{task}", entry.node.0, entry.seq)
            }
            Body::Withdraw { task } => format!("n{}#{} withdraw t{task}", entry.node.0, entry.seq),
        },
        Envelope::AntiEntropy(vv) => format!("vv sync ({} known)", vv.iter().count()),
    }
}

/// A bar showing how many messages are currently in the air.
fn bar(n: i32) -> String {
    let n = n.max(0) as usize;
    format!(
        "{DIM}in flight{RST} {n:>3} {CYN}{}{RST}",
        "▍".repeat(n.min(40))
    )
}

/// Turns "000:000,001:000,002:001" into "{n0 n1} {n2}".
fn pretty_groups(s: &str) -> String {
    let mut out: Vec<Vec<String>> = Vec::new();
    for pair in s.split(',') {
        let (node, group) = pair.split_once(':').unwrap_or(("0", "0"));
        let g: usize = group.parse().unwrap_or(0);
        while out.len() <= g {
            out.push(Vec::new());
        }
        out[g].push(format!("n{}", node.parse::<u8>().unwrap_or(0)));
    }
    out.iter()
        .map(|g| format!("{{{}}}", g.join(" ")))
        .collect::<Vec<_>>()
        .join("  ")
}

fn flag(args: &[String], name: &str) -> Option<u64> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1)?.parse().ok()
}
