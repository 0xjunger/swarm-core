//! M6's acceptance criterion: I1–I4 become executable checks run across
//! thousands of seeds with `proptest`. I5 and I6 are structural
//! (`swarm-core/src/policy.rs`), not runtime-checked here.
//!
//! `mutant_i3_detection` proves the checker catches a real bug: it is the
//! same test compiled twice, clean and against the `mutant-i3` feature, and
//! only the mutant build is expected to fail (`PHASE1-REMEDIATION.md` A2).

use proptest::prelude::*;

use swarm_sim::{run_with_states, SimConfig};
use swarm_verify::check_invariants;

fn sim_config_strategy() -> impl Strategy<Value = SimConfig> {
    (2u8..=6u8, any::<u64>(), 50u64..=150u64).prop_map(|(nodes, seed, ticks)| SimConfig {
        nodes,
        seed,
        ticks,
        loss_permille: 200,
        delay_min: 1,
        delay_max: 3,
        queue_cap: 64,
        entry_period: 10,
        anti_entropy_period: 15,
        log_cap: 1000,
        buffer_cap: 32,
        partitions: vec![],
        equivocation: None,
        budget_per_node: 3,
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5000))]

    /// M6 primary test: 5000 random seeds, zero invariant violations.
    #[test]
    fn m6_all_invariants_hold_across_five_thousand_seeds(
        cfg in sim_config_strategy()
    ) {
        let (_, states) = run_with_states(&cfg);
        let violations = check_invariants(&states, &cfg.budgets());
        prop_assert!(
            violations.is_empty(),
            "seed {}: {violations:?}",
            cfg.seed
        );
    }
}

// ---------------------------------------------------------------------------
// The deliberately broken variant (`mutant-i3` feature, off by default)
// ---------------------------------------------------------------------------

/// Has both nodes *author* (via `Event::Tick`, the real authoring path —
/// `next_task` makes task 0 the first thing every node claims) a competing
/// claim for task 0, exchanges the two resulting entries, and checks what
/// `check_invariants` says about the result. This test is compiled and run
/// twice, unmodified (`PHASE1-REMEDIATION.md` A2) — the mutation lives in
/// `swarm_core::state::Claims::winner`'s tie-break behind `#[cfg(feature =
/// "mutant-i3")]`, not in this test:
///
/// ```text
/// cargo test --release -p swarm-sim --test m6_property mutant_i3_detection
///   # clean: green
/// cargo test --release -p swarm-core --features mutant-i3 \
///   -p swarm-sim --test m6_property mutant_i3_detection
///   # mutant: must FAIL
/// ```
///
/// Both nodes end up with an identical entry set — `{(A,0), (B,0)}` — by
/// construction, so `check_i3` always compares them. In the clean build,
/// `Claims`'s winner rule is `Claim`'s `Ord` alone, so both nodes agree. In
/// the `mutant-i3` build, `winner` was changed to prefer the observing
/// node's own claim, so A's state says A won and B's state says B won: a
/// real, node-dependent divergence over the identical entry set, produced by
/// the real `Claims` fold — not a hand-built fake.
#[test]
fn mutant_i3_detection() {
    use ed25519_dalek::SigningKey;
    use std::collections::BTreeMap;
    use swarm_core::wire::{Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
    use swarm_core::{step, Effect, Event, LogicalTime, NodeId, State};

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    let a = NodeId(0);
    let b = NodeId(1);
    let (ka, kb) = (key(1), key(2));

    let mut keys = BTreeMap::new();
    keys.insert(a, ka.verifying_key());
    keys.insert(b, kb.verifying_key());
    let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

    // Both nodes author their own task-0 claim via a real Tick — same
    // priority, a genuine tie decided only by the tie-break rule.
    let sa0 = State::new(a, roster.clone(), ka, 64, 8, 1, 0);
    let (sa1, fx_a) = step(&sa0, Event::Tick, LogicalTime(1));
    let Effect::Send { payload: claim_a, .. } = fx_a[0].clone();

    let sb0 = State::new(b, roster.clone(), kb, 64, 8, 1, 0);
    let (sb1, fx_b) = step(&sb0, Event::Tick, LogicalTime(1));
    let Effect::Send { payload: claim_b, .. } = fx_b[0].clone();

    // Exchange: each node receives the other's claim.
    let (sa2, _) = step(&sa1, Event::Recv { from: b, payload: claim_b }, LogicalTime(2));
    let (sb2, _) = step(&sb1, Event::Recv { from: a, payload: claim_a }, LogicalTime(2));

    let mut states = BTreeMap::new();
    states.insert(a, sa2);
    states.insert(b, sb2);

    let violations = check_invariants(&states, &BTreeMap::new());

    // Unconditional on purpose: this must be green against today's
    // swarm-core and FAIL against a `mutant-i3` build. A `#[cfg]`-gated
    // assertion here would let the mutant build quietly compile a
    // different, still-passing check instead of actually catching the bug.
    assert!(
        violations.is_empty(),
        "I3 violated over an identical entry set — expected only when swarm-core \
         was built with `--features mutant-i3`: {violations:?}"
    );
}
