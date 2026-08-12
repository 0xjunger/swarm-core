//! M4's acceptance test (`DESIGN.md` D-007):
//!
//! A deliberately faulty node signs two different entries at the same
//! `(node, seq)` and sends one to each side. When the two honest nodes
//! exchange what they have (anti-entropy), each independently produces a
//! proof of equivocation — and a third party holding only the roster
//! verifies it unilaterally, with no further context and no agreement from
//! anyone.

use ed25519_dalek::Signature;
use std::collections::BTreeSet;

use swarm_core::fault::{verify_poe, Poe};
use swarm_core::wire::Roster;
use swarm_core::NodeId;
use swarm_sim::sim::{build_roster, Equivocation};
use swarm_sim::{run_with_states, SimConfig};

const A: NodeId = NodeId(0);
const B: NodeId = NodeId(1);
const F: NodeId = NodeId(2);

fn cfg(seed: u64, loss_permille: u32, ticks: u64) -> SimConfig {
    SimConfig {
        nodes: 3,
        seed,
        ticks,
        loss_permille,
        delay_min: 1,
        delay_max: 3,
        queue_cap: 256,
        // `anti_entropy_period` set comfortably past `entry_period +
        // delay_max`: the first anti-entropy round must not fire until
        // every direct genesis delivery has had a chance to land, or an
        // anti-entropy relay carrying a *stale* pre-genesis vector can race
        // a node's own direct copy and the outcome becomes delivery-order
        // dependent rather than a property of the protocol.
        entry_period: 5,
        anti_entropy_period: 20,
        log_cap: 1000,
        buffer_cap: 32,
        partitions: Vec::new(),
        // F signs one genesis entry for A (the real one, since A is not
        // listed as a victim) and a different one for B. No partition is
        // needed: the two conflicting entries reach A and B directly, and
        // anti-entropy's overlap-by-one reply (`DESIGN.md` D-007) is what
        // lets each side eventually see the other's copy.
        equivocation: Some(Equivocation {
            node: F,
            victims: BTreeSet::from([B]),
        }),
        budget_per_node: 0,
    }
}

/// `verify_poe` needs only a roster of public keys, never a simulator or a
/// trace, to reach a verdict (`DESIGN.md` D-007) — this is the same roster
/// `run_with_states` builds internally, rebuilt here to make the point that
/// no simulator state is required to check a proof.
fn roster3() -> Roster {
    build_roster(&[A, B, F])
}

#[test]
fn both_honest_nodes_independently_prove_the_same_equivocation() {
    let (_, states) = run_with_states(&cfg(7, 0, 60));

    let a_poe = states[&A]
        .poes()
        .find(|p| p.node() == F)
        .expect("A must have seen B's copy of F's genesis entry by now");
    let b_poe = states[&B]
        .poes()
        .find(|p| p.node() == F)
        .expect("B must have seen A's copy of F's genesis entry by now");

    assert_eq!(
        a_poe, b_poe,
        "two independently constructed proofs of the same equivocation must be identical"
    );
    assert_eq!(a_poe.seq(), 0);

    // A third party — no simulator, no trace, just the roster — reaches the
    // same verdict on its own (`DESIGN.md` D-007: "a third party holding
    // only the roster's public keys, who never ran the mission and never
    // exchanged anything with whoever raised the accusation, reaches the
    // identical verdict from the two signatures alone").
    let roster = roster3();
    assert!(verify_poe(&roster, a_poe).is_ok());
    assert!(verify_poe(&roster, b_poe).is_ok());
}

#[test]
fn a_bit_flip_in_either_signature_breaks_verification() {
    let (_, states) = run_with_states(&cfg(7, 0, 60));
    let poe = states[&A].poes().find(|p| p.node() == F).unwrap();

    let mut tampered_a = poe.a().clone();
    let mut sig = tampered_a.sig.to_bytes();
    sig[0] ^= 1;
    tampered_a.sig = Signature::from_bytes(&sig);
    let tampered = Poe::new(tampered_a, poe.b().clone()).expect("still two distinct entries");

    assert!(verify_poe(&roster3(), &tampered).is_err());
}

#[test]
fn honest_nodes_never_produce_a_false_accusation() {
    // Same shape of run, no equivocation configured: nobody is ever proven
    // faulty. Guards against a detector that fires on ordinary anti-entropy
    // duplication.
    let mut cfg = cfg(7, 100, 60);
    cfg.equivocation = None;
    let (_, states) = run_with_states(&cfg);

    for n in [A, B, F] {
        assert_eq!(
            states[&n].poes().count(),
            0,
            "node {n:?} falsely accused someone in a run with no faulty node"
        );
    }
}

#[test]
fn detection_holds_under_loss_across_seeds() {
    // Loss and delay only ever *postpone* detection here, never prevent it:
    // the overlap-by-one reply (`DESIGN.md` D-007) is re-offered on every
    // anti-entropy round, and a dropped message is simply retried at the
    // next one. 300 ticks gives ~14 rounds at
    // this `anti_entropy_period`, comfortably enough at 5% loss for both
    // directions to land at least once.
    for seed in 0..20 {
        let (_, states) = run_with_states(&cfg(seed, 50, 300));
        assert!(
            states[&A].poes().any(|p| p.node() == F),
            "seed {seed}: A never proved F's equivocation"
        );
        assert!(
            states[&B].poes().any(|p| p.node() == F),
            "seed {seed}: B never proved F's equivocation"
        );
    }
}
