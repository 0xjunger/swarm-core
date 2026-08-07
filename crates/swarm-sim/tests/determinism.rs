//! M0 acceptance tests.
//!
//! The milestone's stated criterion (`DESIGN.md` §M0) is only the first two tests
//! here: same seed produces an identical trace, different seeds produce different
//! ones. The rest exist because those two are satisfiable by a completely broken
//! simulator — one that drops every message, or never delivers anything, is
//! perfectly deterministic.
//!
//! `DESIGN.md` §M6 insists that a passing test must be shown to catch something.
//! That principle is worth applying at M0 rather than waiting for M6, so the tests
//! below are split into two groups: the criterion, and the guards that stop the
//! criterion from being met vacuously.

use std::collections::BTreeSet;
use swarm_core::NodeId;
use swarm_sim::{run, Partition, SimConfig, TraceRecord};

/// A busy configuration: loss and delay variance on, so the channel model is
/// actually exercised rather than trivially quiet.
fn busy(seed: u64) -> SimConfig {
    SimConfig {
        nodes: 5,
        seed,
        ticks: 120,
        loss_permille: 100,
        delay_min: 1,
        delay_max: 5,
        queue_cap: 64,
        entry_period: 10,
        anti_entropy_period: 0,
        log_cap: 1000,
        buffer_cap: 32,
        partitions: Vec::new(),
        equivocation: None,
        budget_per_node: 3,
    }
}

// ---------------------------------------------------------------------------
// The M0 criterion
// ---------------------------------------------------------------------------

#[test]
fn same_seed_is_byte_identical() {
    let a = run(&busy(42));
    let b = run(&busy(42));

    // Compare the rendered text, not just the digest: a failure here should show a
    // diff of what actually diverged, not "two 32-byte arrays differ".
    assert_eq!(a.render(), b.render());
    assert_eq!(a.digest(), b.digest());
}

#[test]
fn different_seeds_diverge() {
    let digests: BTreeSet<[u8; 32]> = (0..64u64).map(|s| run(&busy(s)).digest()).collect();
    assert_eq!(
        digests.len(),
        64,
        "two distinct seeds produced the same trace"
    );
}

#[test]
fn determinism_holds_under_partitions() {
    // The path M2-M5 actually depend on: partitions opening and healing mid-run.
    let cfg = |seed| SimConfig {
        partitions: vec![
            (
                20,
                Partition::split(&[&[NodeId(0), NodeId(1)], &[NodeId(2), NodeId(3), NodeId(4)]]),
            ),
            (
                60,
                Partition::split(&[
                    &[NodeId(0)],
                    &[NodeId(1), NodeId(2)],
                    &[NodeId(3), NodeId(4)],
                ]),
            ),
            (90, Partition::connected(&SimConfig::default().roster())),
        ],
        ..busy(seed)
    };

    assert_eq!(run(&cfg(7)).render(), run(&cfg(7)).render());
    assert_ne!(run(&cfg(7)).digest(), run(&cfg(8)).digest());
}

// ---------------------------------------------------------------------------
// Guards: without these, the tests above are satisfiable by a broken simulator
// ---------------------------------------------------------------------------

#[test]
fn all_nodes_actually_communicate() {
    // Guards against determinism-by-silence. A simulator that delivers nothing
    // passes every test above.
    let trace = run(&SimConfig {
        loss_permille: 0,
        ..busy(1)
    });

    let finals: Vec<_> = trace
        .records()
        .iter()
        .filter_map(|r| match r {
            TraceRecord::Final { node, recv, sent } => Some((*node, *recv, *sent)),
            _ => None,
        })
        .collect();

    assert_eq!(finals.len(), 5);
    for (node, recv, sent) in finals {
        assert!(recv > 0, "node {node:?} received nothing");
        assert!(sent > 0, "node {node:?} sent nothing");
    }
}

#[test]
fn partition_blocks_delivery_across_groups() {
    // Guards against a partition model that is deterministic because it does
    // nothing. Split from tick 1 so the whole run is under partition.
    let left = [NodeId(0), NodeId(1)];
    let right = [NodeId(2), NodeId(3), NodeId(4)];

    let trace = run(&SimConfig {
        loss_permille: 0,
        partitions: vec![(1, Partition::split(&[&left, &right]))],
        ..busy(3)
    });

    let mut within = 0usize;
    for r in trace.records() {
        if let TraceRecord::Deliver { from, to, .. } = r {
            let same_group = left.contains(from) == left.contains(to);
            assert!(
                same_group,
                "message crossed the partition: {from:?} -> {to:?}"
            );
            within += 1;
        }
    }

    assert!(
        within > 0,
        "nothing was delivered at all — the test proves nothing"
    );
    assert!(
        trace.count(|r| matches!(r, TraceRecord::DropPartition { .. })) > 0,
        "no message was ever blocked by the partition"
    );
}

#[test]
fn bounded_queue_drops_oldest_under_pressure() {
    // Guards the memory bound DESIGN.md §7 requires. Tiny cap, heavy traffic.
    let trace = run(&SimConfig {
        loss_permille: 0,
        queue_cap: 2,
        entry_period: 1,
        anti_entropy_period: 0,
        log_cap: 1000,
        buffer_cap: 32,
        ticks: 60,
        ..busy(5)
    });

    assert!(
        trace.count(|r| matches!(r, TraceRecord::DropOverflow { .. })) > 0,
        "queue never overflowed, so the bound was never exercised"
    );
    // And the run stayed deterministic while dropping.
    assert_eq!(
        trace.digest(),
        run(&SimConfig {
            loss_permille: 0,
            queue_cap: 2,
            entry_period: 1,
            anti_entropy_period: 0,
            log_cap: 1000,
            buffer_cap: 32,
            ticks: 60,
            ..busy(5)
        })
        .digest()
    );
}

#[test]
fn trace_observes_the_model_not_just_the_seed() {
    // Guards against a trace so coarse it would not notice a behaviour change.
    // Every one of these is a different model at the same seed; all must differ.
    let base = busy(11);
    let variants = [
        base.clone(),
        SimConfig {
            loss_permille: 500,
            ..base.clone()
        },
        SimConfig {
            delay_max: 6,
            ..base.clone()
        },
        SimConfig {
            entry_period: 5,
            anti_entropy_period: 0,
            log_cap: 1000,
            buffer_cap: 32,
            ..base.clone()
        },
        SimConfig {
            nodes: 4,
            ..base.clone()
        },
        SimConfig {
            ticks: 119,
            ..base.clone()
        },
        SimConfig {
            partitions: vec![(2, Partition::split(&[&[NodeId(0)], &base.roster()[1..]]))],
            ..base.clone()
        },
    ];

    let digests: BTreeSet<[u8; 32]> = variants.iter().map(|c| run(c).digest()).collect();
    assert_eq!(
        digests.len(),
        variants.len(),
        "two different models produced the same trace"
    );
}

// ---------------------------------------------------------------------------
// Contract rules that must fail loudly rather than silently
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "delay_min must be >= 1")]
fn zero_delay_is_rejected() {
    // Rule R1 (docs/spec.md §6): a zero delay would allow send -> receive -> send
    // cascades inside one tick, whose order no stated rule determines.
    run(&SimConfig {
        delay_min: 0,
        ..busy(1)
    });
}
