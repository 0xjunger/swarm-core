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

fn cfg(seed: u64, loss_permille: u32) -> SimConfig {
    SimConfig {
        nodes: 3,
        seed,
        // Partitioned for 100 ticks, then healed, then 50 more — DESIGN.md's
        // M2 criterion literally.
        //
        // The two periods below are chosen against one hazard: a static
        // `SimConfig` has no "stop authoring at tick N", so the run always
        // ends while some entry is still in flight unless the tail after the
        // *last* authoring tick is wide enough. That is an artifact of an
        // arbitrary stopping point, not a convergence failure, and it is
        // worth engineering away rather than tolerating — otherwise a green
        // test means "we stopped at a lucky tick".
        //
        // `entry_period: 40` puts the last authoring tick at 120, leaving 30
        // quiet ticks. `anti_entropy_period: 5` fits six anti-entropy rounds
        // into those 30 ticks, and each round independently re-offers
        // whatever a peer is still missing — which is exactly the mechanism
        // `DESIGN.md` §4.1 promises ("kayıp mesajları er ya da geç yakalar").
        // Measured over 60 seeds: six rounds converge every time at 20% loss
        // and all but a handful at 40%, where one round (the earlier
        // `anti_entropy_period: 15`) failed 14 seeds in 60. M3 raised the
        // entry rate — a period now emits a claim *and* any withdrawals —
        // which is what made the old margin too thin.
        ticks: 150,
        loss_permille,
        delay_min: 1,
        delay_max: 3,
        queue_cap: 256,
        entry_period: 40,
        anti_entropy_period: 5,
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
    let (_, states) = run_with_states(&cfg(7, 0));

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
    //
    // Swept over seeds rather than pinned to one: a single seed proves that
    // *a* lossy run converged, which is a much weaker claim than the one
    // this test is named after, and it is the difference between a test that
    // holds and a test that got lucky.
    for seed in 0..20 {
        let (_, states) = run_with_states(&cfg(seed, 200));

        let a = entry_map(states[&A].entries());
        let b = entry_map(states[&B].entries());
        let c = entry_map(states[&C].entries());

        assert!(!a.is_empty(), "seed {seed}: the run produced no entries");
        assert_eq!(
            a, b,
            "seed {seed}: A and B disagree after healing under loss"
        );
        assert_eq!(
            b, c,
            "seed {seed}: B and C disagree after healing under loss"
        );
    }
}
