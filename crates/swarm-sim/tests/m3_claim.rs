//! M3's acceptance test (`DESIGN.md` §9, "Bitti sayılır", verbatim):
//!
//! "İki partisyon aynı görevi talep ediyor, birleşme sonrası **her iki node
//! da aynı kazananı** hesaplıyor (kimse 'ben kazandım' sanmıyor). Ve kaybeden
//! node'un log'unda geri çekilme kaydı var."
//!
//! The partition shape is M2's — `{A,B}` against `{C}` — because that is what
//! makes the claims genuinely concurrent: both sides claim task 0 while they
//! cannot see each other, and the contest is only resolved when the link
//! comes back. The timing rationale for `ticks`/`entry_period`/
//! `anti_entropy_period` is the same one `tests/m2_convergence.rs` states at
//! length, plus one extra requirement noted at `cfg` below.

use std::collections::BTreeSet;

use swarm_core::state::TaskId;
use swarm_core::wire::Body;
use swarm_core::{NodeId, State};
use swarm_sim::{run_with_states, Partition, SimConfig};

const A: NodeId = NodeId(0);
const B: NodeId = NodeId(1);
const C: NodeId = NodeId(2);
const ALL: [NodeId; 3] = [A, B, C];

fn cfg(seed: u64, loss_permille: u32) -> SimConfig {
    SimConfig {
        nodes: 3,
        seed,
        // `entry_period: 40` also matters for a reason M2 did not have: a
        // node can only withdraw on an `entry_period` tick
        // (`docs/spec.md` §10.6), so the tail must hold the *whole* losing
        // sequence — heal, rival claim arrives, next period authors the
        // withdrawal, withdrawal propagates. Healing at 101 with authoring
        // ticks at 120 and a 30-tick quiet tail leaves room for exactly
        // that.
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
        equivocation: None,
        budget_per_node: 3,
    }
}

/// The bodies in a node's *own* chain — the log the acceptance criterion
/// talks about when it says "kaybeden node'un log'unda".
fn own_bodies(state: &State) -> Vec<Body> {
    state.log().entries().iter().map(|e| e.body).collect()
}

fn withdrew(state: &State, task: TaskId) -> bool {
    own_bodies(state).contains(&Body::Withdraw { task })
}

fn claimed(state: &State, task: TaskId) -> bool {
    own_bodies(state)
        .iter()
        .any(|b| matches!(b, Body::TaskClaim { task: t, .. } if *t == task))
}

// ---------------------------------------------------------------------------
// The criterion, clause by clause
// ---------------------------------------------------------------------------

/// Clause 1: "her iki node da aynı kazananı hesaplıyor."
///
/// Not "a winner exists" — every node names the *same* one, for every task
/// any of them knows about. This is invariant I3 at M3 (`docs/spec.md`
/// §13) observed end to end.
#[test]
fn every_node_computes_the_same_winner_for_every_task() {
    let (_, states) = run_with_states(&cfg(7, 0));

    let tasks: BTreeSet<TaskId> = ALL
        .iter()
        .flat_map(|n| states[n].claims().tasks())
        .collect();
    assert!(!tasks.is_empty(), "the run must actually produce claims");

    for task in tasks {
        let winners: Vec<_> = ALL
            .iter()
            .map(|n| states[n].claims().winner(task))
            .collect();
        assert!(
            winners[0].is_some(),
            "task {task} is known but has no winner"
        );
        assert!(
            winners.iter().all(|w| *w == winners[0]),
            "task {task}: nodes disagree about the winner: {winners:?}"
        );
    }
}

/// Clause 1, the sharp edge: "kimse 'ben kazandım' sanmıyor."
///
/// A node believes it won a task exactly when it is that task's winner in its
/// own derived state. Since every node agrees on the winner (above), at most
/// one node can believe it won — and exactly one must, so the task is not
/// silently abandoned by everybody.
#[test]
fn exactly_one_node_believes_it_won_each_contested_task() {
    let (_, states) = run_with_states(&cfg(7, 0));

    for task in states[&A].claims().tasks() {
        let believers: Vec<NodeId> = ALL
            .into_iter()
            .filter(|n| {
                states[n]
                    .claims()
                    .winner(task)
                    .is_some_and(|w| w.node == *n)
            })
            .collect();
        assert_eq!(
            believers.len(),
            1,
            "task {task}: {} nodes believe they won ({believers:?})",
            believers.len()
        );
    }
}

/// Clause 2: "kaybeden node'un log'unda geri çekilme kaydı var" — and the
/// mirror image, which the criterion implies but does not say: the winner
/// must *not* withdraw.
///
/// Task 0 is the one both partitions claim: every node's first claim is task
/// 0 (`docs/spec.md` §10.6), and the first authoring tick is 40, well inside
/// the partition. So this is a genuine concurrent contest across the split,
/// which is what `DESIGN.md` asks for.
#[test]
fn the_losers_of_the_contested_task_withdraw_and_the_winner_does_not() {
    let (_, states) = run_with_states(&cfg(7, 0));

    for n in ALL {
        assert!(
            claimed(&states[&n], 0),
            "node {n:?} never claimed task 0, so the contest was not contested"
        );
    }

    let winner = states[&A]
        .claims()
        .winner(0)
        .expect("task 0 was claimed by all three")
        .node;

    for n in ALL {
        if n == winner {
            assert!(
                !withdrew(&states[&n], 0),
                "the winner {n:?} withdrew from the task it won"
            );
        } else {
            assert!(
                withdrew(&states[&n], 0),
                "loser {n:?} has no withdrawal record for task 0 in its own log"
            );
        }
    }

    // And every node can see that the losers stood down, not just the losers
    // themselves — the withdrawal is replicated state, not a private note.
    for observer in ALL {
        for n in ALL {
            assert_eq!(
                states[&observer].claims().has_withdrawn(0, n),
                n != winner,
                "observer {observer:?} disagrees about whether {n:?} withdrew"
            );
        }
    }
}

/// The whole criterion again, under message loss and across seeds. One seed
/// proves a run happened to work; this is the claim the milestone actually
/// makes.
#[test]
fn the_criterion_holds_under_loss_across_seeds() {
    for seed in 0..20 {
        let (_, states) = run_with_states(&cfg(seed, 200));

        let winner = states[&A].claims().winner(0);
        assert!(winner.is_some(), "seed {seed}: task 0 has no winner");

        for n in ALL {
            assert_eq!(
                states[&n].claims().winner(0),
                winner,
                "seed {seed}: node {n:?} disagrees about task 0's winner"
            );
        }

        let winner = winner.expect("checked above").node;
        for n in ALL {
            assert_eq!(
                withdrew(&states[&n], 0),
                n != winner,
                "seed {seed}: node {n:?} withdrew from task 0 iff it lost — violated"
            );
        }
    }
}

/// A node never withdraws from a task it did not claim, and never withdraws
/// twice from the same one (`docs/spec.md` §10.5: losing is monotone).
#[test]
fn withdrawals_are_at_most_one_per_claimed_task() {
    let (_, states) = run_with_states(&cfg(3, 200));

    for n in ALL {
        let mut seen: BTreeSet<TaskId> = BTreeSet::new();
        for body in own_bodies(&states[&n]) {
            let Body::Withdraw { task } = body else {
                continue;
            };
            assert!(
                seen.insert(task),
                "node {n:?} withdrew from task {task} twice"
            );
            assert!(
                claimed(&states[&n], task),
                "node {n:?} withdrew from task {task} without ever claiming it"
            );
        }
    }
}
