//! The task-claim CRDT (`SPEC.md` §6.3).
//!
//! Written before the code it guards. Claims are built by
//! hand here — the same `raw_entry`/`vv` idiom `tests/causal.rs` uses — so a
//! scenario can produce exactly the `(priority, lc, node)` combination it
//! needs without first growing real chains and real partitions for it. The
//! end-to-end version lives in `swarm-sim/tests/m3_claim.rs`.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use swarm_core::causal::VersionVector;
use swarm_core::log::verify_next;
use swarm_core::state::{Claim, Claims};
use swarm_core::wire::{
    Body, Entry, Hash, Roster, UnsignedEntry, VerifiedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID,
};
use swarm_core::NodeId;

const W: NodeId = NodeId(0);
const X: NodeId = NodeId(1);
const Y: NodeId = NodeId(2);

fn key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    SigningKey::from_bytes(&bytes)
}

fn roster3() -> (Roster, [SigningKey; 3]) {
    let keys = [key(1), key(2), key(3)];
    let mut m = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        m.insert(NodeId(i as u8), k.verifying_key());
    }
    (Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, m), keys)
}

fn vv(pairs: &[(NodeId, u64)]) -> VersionVector {
    let mut v = VersionVector::new();
    for &(n, s) in pairs {
        v.bump(n, s);
    }
    v
}

/// An entry with an arbitrary `deps`, so `lc` (`SPEC.md` §6.3) can be
/// dialled directly: `lc = Σ (seq + 1)` over `deps`.
fn entry(node: NodeId, key: &SigningKey, seq: u64, deps: VersionVector, body: Body) -> Entry {
    UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node,
        seq,
        prev: Hash::ZERO,
        deps,
        body,
    }
    .sign(key)
}

/// `Claims::observe` only accepts verified entries (`SPEC.md` §6.3), so
/// tests must go through the verifier too. `expected_seq`/`expected_prev` are
/// fed from the entry itself: this file is testing the CRDT, not the chain
/// rules, which `tests/chain.rs` already covers.
fn verified(roster: &Roster, e: &Entry) -> VerifiedEntry {
    verify_next(roster, 0, e.seq, e.prev, e).expect("hand-built entry must verify")
}

fn claim_of(
    roster: &Roster,
    node: NodeId,
    k: &SigningKey,
    seq: u64,
    deps: &[(NodeId, u64)],
    task: u64,
    priority: u8,
) -> VerifiedEntry {
    let e = entry(node, k, seq, vv(deps), Body::TaskClaim { task, priority });
    verified(roster, &e)
}

fn withdraw_of(
    roster: &Roster,
    node: NodeId,
    k: &SigningKey,
    seq: u64,
    task: u64,
) -> VerifiedEntry {
    let e = entry(node, k, seq, VersionVector::new(), Body::Withdraw { task });
    verified(roster, &e)
}

fn fold(entries: &[VerifiedEntry]) -> Claims {
    let mut c = Claims::default();
    for e in entries {
        c.observe(e);
    }
    c
}

// ---------------------------------------------------------------------------
// lc: the derived logical clock (`SPEC.md` §6.3)
// ---------------------------------------------------------------------------

#[test]
fn lc_counts_the_entries_the_author_had_applied() {
    // Empty deps: the author had applied nothing.
    assert_eq!(VersionVector::new().entry_count(), 0);
    // (W, 0) means one entry from W; (X, 2) means three from X.
    assert_eq!(vv(&[(W, 0)]).entry_count(), 1);
    assert_eq!(vv(&[(W, 0), (X, 2)]).entry_count(), 4);
}

// ---------------------------------------------------------------------------
// The winner rule's three terms, one test each (`SPEC.md` §6.3:
// min by (priority, logical_clock, node_id))
// ---------------------------------------------------------------------------

#[test]
fn an_unclaimed_task_has_no_winner() {
    assert_eq!(Claims::default().winner(7), None);
    // And a task with claims does not leak into a neighbouring task id.
    let (roster, keys) = roster3();
    let c = fold(&[claim_of(&roster, W, &keys[0], 0, &[], 7, 1)]);
    assert!(c.winner(7).is_some());
    assert_eq!(c.winner(8), None);
}

#[test]
fn priority_decides_first() {
    let (roster, keys) = roster3();
    // X has the lowest priority but the *highest* lc and node id — priority
    // must still dominate both of the later terms.
    let c = fold(&[
        claim_of(&roster, W, &keys[0], 0, &[], 7, 9),
        claim_of(&roster, X, &keys[1], 0, &[(W, 5), (X, 5)], 7, 1),
        claim_of(&roster, Y, &keys[2], 0, &[], 7, 5),
    ]);
    assert_eq!(c.winner(7).map(|w| w.node), Some(X));
}

#[test]
fn logical_clock_decides_when_priorities_tie() {
    let (roster, keys) = roster3();
    // Equal priority. Y's lc is 0, W's is 3, X's is 6 — Y claimed earliest in
    // causal terms and wins despite having the highest node id.
    let c = fold(&[
        claim_of(&roster, W, &keys[0], 0, &[(W, 2)], 7, 1),
        claim_of(&roster, X, &keys[1], 0, &[(W, 2), (X, 2)], 7, 1),
        claim_of(&roster, Y, &keys[2], 0, &[], 7, 1),
    ]);
    let winner = c.winner(7).expect("three claims exist");
    assert_eq!(winner.node, Y);
    assert_eq!(winner.lc, 0);
}

#[test]
fn node_id_decides_when_priority_and_logical_clock_tie() {
    let (roster, keys) = roster3();
    // Everything equal but the author. The lowest NodeId wins — never a wall
    // clock, never insertion order (`DESIGN.md` D-002).
    let c = fold(&[
        claim_of(&roster, Y, &keys[2], 0, &[(W, 0)], 7, 1),
        claim_of(&roster, X, &keys[1], 0, &[(W, 0)], 7, 1),
        claim_of(&roster, W, &keys[0], 0, &[(W, 0)], 7, 1),
    ]);
    assert_eq!(c.winner(7).map(|w| w.node), Some(W));
}

// ---------------------------------------------------------------------------
// The OR-set: unique tags, idempotence, commutativity (`SPEC.md` §6.3)
// ---------------------------------------------------------------------------

#[test]
fn two_identical_looking_claims_from_different_nodes_stay_distinct() {
    // The OR-set's whole point: same task, same priority, same lc, two
    // authors — two elements, not one merged element.
    let (roster, keys) = roster3();
    let c = fold(&[
        claim_of(&roster, W, &keys[0], 0, &[], 7, 1),
        claim_of(&roster, X, &keys[1], 0, &[], 7, 1),
    ]);
    assert_eq!(c.claims(7).count(), 2);
}

#[test]
fn observing_the_same_entry_twice_changes_nothing() {
    let (roster, keys) = roster3();
    let e = claim_of(&roster, W, &keys[0], 0, &[], 7, 1);
    let once = fold(std::slice::from_ref(&e));
    let twice = fold(&[e.clone(), e]);
    assert_eq!(once, twice, "folding is idempotent");
}

#[test]
fn i3_the_same_entry_set_folds_to_the_same_claims_in_any_order() {
    let (roster, keys) = roster3();
    let a = claim_of(&roster, W, &keys[0], 0, &[(W, 3)], 7, 2);
    let b = claim_of(&roster, X, &keys[1], 0, &[], 7, 2);
    let c = claim_of(&roster, Y, &keys[2], 0, &[(X, 1)], 8, 1);
    let d = withdraw_of(&roster, W, &keys[0], 1, 7);

    let forward = fold(&[a.clone(), b.clone(), c.clone(), d.clone()]);
    let backward = fold(&[d, c, b, a]);

    assert_eq!(
        forward, backward,
        "I3: same entries, different order, same derived state"
    );
    assert_eq!(forward.winner(7), backward.winner(7));
    assert_eq!(forward.winner(8), backward.winner(8));
}

// ---------------------------------------------------------------------------
// Losing is monotone (`SPEC.md` §6.3)
// ---------------------------------------------------------------------------

#[test]
fn once_a_node_has_lost_no_later_claim_can_make_it_win_again() {
    let (roster, keys) = roster3();
    let mut c = Claims::default();

    // W claims alone: it is the winner.
    c.observe(&claim_of(&roster, W, &keys[0], 0, &[(W, 4)], 7, 5));
    assert_eq!(c.winner(7).map(|w| w.node), Some(W));

    // X's better claim arrives. W has lost.
    c.observe(&claim_of(&roster, X, &keys[1], 0, &[], 7, 1));
    assert_eq!(c.winner(7).map(|w| w.node), Some(X));

    // Every further claim, however weak, leaves W a loser: the set only
    // grows, so its minimum can only improve.
    for (i, node) in [Y, W, X].into_iter().enumerate() {
        let k = &keys[node.0 as usize];
        c.observe(&claim_of(&roster, node, k, 9 + i as u64, &[(W, 9)], 7, 9));
        assert_ne!(c.winner(7).map(|w| w.node), Some(W));
    }
}

// ---------------------------------------------------------------------------
// Withdrawal is a record, not a set removal (`SPEC.md` §6.3)
// ---------------------------------------------------------------------------

#[test]
fn a_withdrawal_is_recorded_without_disturbing_the_winner() {
    let (roster, keys) = roster3();
    let w_claim = claim_of(&roster, W, &keys[0], 0, &[(W, 4)], 7, 5);
    let x_claim = claim_of(&roster, X, &keys[1], 0, &[], 7, 1);

    let before = fold(&[w_claim.clone(), x_claim.clone()]);
    let after = fold(&[w_claim, x_claim, withdraw_of(&roster, W, &keys[0], 1, 7)]);

    assert!(!before.has_withdrawn(7, W));
    assert!(after.has_withdrawn(7, W));
    assert!(!after.has_withdrawn(7, X), "X never withdrew");
    assert_eq!(
        before.winner(7),
        after.winner(7),
        "a withdrawal must not move the winner (SPEC.md §6.3)"
    );
    assert_eq!(
        before.claims(7).count(),
        after.claims(7).count(),
        "the claim set is grow-only: withdrawing removes nothing"
    );
}

// ---------------------------------------------------------------------------
// Claim's Ord *is* the winner rule (`SPEC.md` §6.3)
// ---------------------------------------------------------------------------

#[test]
fn claim_ordering_is_the_winner_rule_in_field_order() {
    let base = Claim {
        priority: 5,
        lc: 5,
        node: X,
        seq: 5,
    };
    // Each field beats every field after it.
    assert!(
        Claim {
            priority: 4,
            lc: 99,
            node: Y,
            seq: 99
        } < base
    );
    assert!(
        Claim {
            lc: 4,
            node: Y,
            seq: 99,
            ..base
        } < base
    );
    assert!(
        Claim {
            node: W,
            seq: 99,
            ..base
        } < base
    );
    // seq is the totality tie-break only (`SPEC.md` §6.3).
    assert!(Claim { seq: 4, ..base } < base);
}

#[test]
fn tasks_are_listed_ascending() {
    let (roster, keys) = roster3();
    let c = fold(&[
        claim_of(&roster, W, &keys[0], 0, &[], 9, 1),
        claim_of(&roster, X, &keys[1], 0, &[], 2, 1),
        claim_of(&roster, Y, &keys[2], 0, &[], 5, 1),
    ]);
    assert_eq!(c.tasks().collect::<Vec<_>>(), [2, 5, 9]);
}
