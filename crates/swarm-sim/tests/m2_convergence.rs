//! M2's acceptance test (`DESIGN.md` §9, "Bitti sayılır", verbatim):
//!
//! 3 nodes `{A,B}` and `{C}` are partitioned, run 100 ticks, merge, run 50
//! more ticks — all three end up with **the same entry set**.

use std::collections::BTreeMap;

use swarm_core::wire::Entry;
use swarm_core::NodeId;
use swarm_sim::{run_with_states, Partition, SimConfig};

const A: NodeId = NodeId(0);
const B: NodeId = NodeId(1);
const C: NodeId = NodeId(2);

fn cfg(loss_permille: u32) -> SimConfig {
    SimConfig {
        nodes: 3,
        seed: 7,
        // Partitioned for 100 ticks, then healed — the shape of DESIGN.md's
        // M2 criterion. `entry_period` is deliberately larger than M0/M1's
        // beacon-style default: with a periodic creator running for the
        // whole simulation (there is no "stop after tick 100" in a static
        // `SimConfig`), the *last* entry any node creates is always at risk
        // of not finishing delivery before the run ends — that is an
        // artifact of an arbitrary stopping point, not a convergence
        // failure. Choosing `ticks` to land 24 ticks past the last
        // multiple of `entry_period` leaves room for one full anti-entropy
        // round-trip (`anti_entropy_period` + `delay_max`) after the very
        // last entry, so the assertion below tests real convergence rather
        // than a mid-flight snapshot.
        ticks: 149,
        loss_permille,
        delay_min: 1,
        delay_max: 3,
        queue_cap: 256,
        entry_period: 25,
        anti_entropy_period: 15,
        log_cap: 1000,
        buffer_cap: 32,
        partitions: vec![
            (1, Partition::split(&[&[A, B], &[C]])),
            (101, Partition::connected(&[A, B, C])),
        ],
    }
}

/// A node's delivered entries, keyed by `(author, seq)` — content, not just
/// count, is what the criterion asks for.
fn entry_map(entries: Vec<&Entry>) -> BTreeMap<(NodeId, u64), Entry> {
    entries
        .into_iter()
        .map(|e| ((e.node, e.seq), e.clone()))
        .collect()
}

#[test]
fn partition_heal_converges_to_the_same_entry_set() {
    let (_, states) = run_with_states(&cfg(0));

    let a = entry_map(states[&A].entries());
    let b = entry_map(states[&B].entries());
    let c = entry_map(states[&C].entries());

    assert!(!a.is_empty(), "the run must actually produce entries");
    assert_eq!(a, b, "A and B disagree after healing");
    assert_eq!(b, c, "B and C disagree after healing");

    // Every node's causal_vv must agree with the entry set it derived from
    // (I3: same entries seen, same derived state).
    for node in [A, B, C] {
        for &(origin, seq) in a.keys() {
            let highest = states[&node].causal_vv().highest(origin);
            assert!(
                highest.is_some_and(|h| h >= seq),
                "node {node:?} is missing ({origin:?}, {seq}) from its own version vector"
            );
        }
    }
}

#[test]
fn partition_heal_converges_even_with_message_loss() {
    // Anti-entropy's whole point (`DESIGN.md` §4.1: "kayıp mesajları er ya
    // da geç yakalar") — convergence must hold under real loss, not only
    // the loss-free happy path.
    let (_, states) = run_with_states(&cfg(200));

    let a = entry_map(states[&A].entries());
    let b = entry_map(states[&B].entries());
    let c = entry_map(states[&C].entries());

    assert!(!a.is_empty());
    assert_eq!(a, b, "A and B disagree after healing under loss");
    assert_eq!(b, c, "B and C disagree after healing under loss");
}
