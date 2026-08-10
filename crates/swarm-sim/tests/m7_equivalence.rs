//! M7's acceptance criterion for `verify` (`docs/spec.md` §20.5): across
//! 5000 random seeds on a clean build, the in-process oracle
//! (`swarm_verify::check_invariants`) and the external verifier
//! (`swarm_verify::verify`, working only from an exported `LogBundle` and
//! `Spec`) agree on whether the run has any violation.
//!
//! This is scoped to *presence of a violation*, not to every field of the
//! two outputs matching, and deliberately does not claim `Undetermined ==
//! Satisfied`: `verify`'s I3 check needs two observers whose applied entry
//! sets coincide, which does not happen for every random seed within the
//! tick budget below, and reporting `Satisfied` on zero comparable pairs
//! would claim evidence the bundle does not contain (`docs/spec.md` §20.5).
//! `Undetermined` is not a violation either way, so it never breaks this
//! equivalence.
//!
//! This test is scoped to a clean `swarm-core` build. `docs/spec.md` §20.5
//! records why it cannot extend to `mutant-i3`: `verify` restates the
//! winner rule itself rather than calling `swarm_core::state::Claims`
//! (`docs/spec.md` §20.5, "why verify does not call swarm-core's own
//! fold"), so it structurally cannot inherit a bug planted inside
//! `Claims::winner` — the two are expected to *disagree* on that build, not
//! match.

use proptest::prelude::*;

use swarm_core::bundle::Spec;
use swarm_core::wire::{PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_sim::sim::build_roster;
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

    /// M7 primary test: the oracle and the external verifier agree on
    /// whether *any* invariant was violated, across 5000 random seeds.
    #[test]
    fn oracle_and_verify_agree_on_violation_presence(cfg in sim_config_strategy()) {
        let (_, states) = run_with_states(&cfg);

        let oracle_violations = check_invariants(&states, &cfg.budgets());

        let mut bundles = states.values().map(|s| s.export_bundle());
        let first = bundles.next().expect("cfg.nodes >= 2");
        let bundle = bundles.fold(first, |acc, b| acc.merge(b));

        let spec = Spec {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            roster: build_roster(&cfg.roster()),
            budgets: cfg.budgets(),
            log_cap: cfg.log_cap as u32,
        };

        let verdict = swarm_verify::verify(&bundle, &spec);

        prop_assert!(
            verdict.chains.is_empty(),
            "seed {}: an honest export produced a chain-verification finding: {:?}",
            cfg.seed,
            verdict.chains
        );
        prop_assert_eq!(
            oracle_violations.is_empty(),
            !verdict.any_violated(),
            "seed {}: oracle={:?} verdict={:?}",
            cfg.seed,
            oracle_violations,
            verdict
        );
    }
}

/// A direct demonstration of `docs/spec.md` §20.5's independence claim: the
/// same tied-claim scenario `m6_property.rs::mutant_i3_detection` uses to
/// prove the oracle catches the `mutant-i3` bug, checked through `verify`
/// instead. `verify`'s own `winner()` is `Claim`'s derived `Ord` alone — it
/// has no notion of "the observing node" to prefer, so this assertion holds
/// unconditionally, on both a clean build and a `mutant-i3` one: unlike
/// `mutant_i3_detection`, this test is not expected to start failing when
/// `swarm-core` is built with `--features mutant-i3`, because `verify`
/// never calls the code that feature changes.
#[test]
fn verify_does_not_inherit_the_mutant_i3_tie_break() {
    use ed25519_dalek::SigningKey;
    use std::collections::BTreeMap;
    use swarm_core::wire::Roster;
    use swarm_core::{step, Effect, Event, LogicalTime, NodeId, State};
    use swarm_verify::verdict::InvariantResult;

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

    let sa0 = State::new(a, roster.clone(), ka, 64, 8, 1, 0);
    let (sa1, fx_a) = step(&sa0, Event::Tick, LogicalTime(1));
    let Effect::Send { payload: claim_a, .. } = fx_a[0].clone();

    let sb0 = State::new(b, roster.clone(), kb, 64, 8, 1, 0);
    let (sb1, fx_b) = step(&sb0, Event::Tick, LogicalTime(1));
    let Effect::Send { payload: claim_b, .. } = fx_b[0].clone();

    let (sa2, _) = step(
        &sa1,
        Event::Recv {
            from: b,
            payload: claim_b,
        },
        LogicalTime(2),
    );
    let (sb2, _) = step(
        &sb1,
        Event::Recv {
            from: a,
            payload: claim_a,
        },
        LogicalTime(2),
    );

    let bundle = sa2.export_bundle().merge(sb2.export_bundle());
    let spec = Spec {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        roster,
        budgets: BTreeMap::new(),
        log_cap: 64,
    };

    let verdict = swarm_verify::verify(&bundle, &spec);
    assert!(
        matches!(verdict.i3, InvariantResult::Satisfied),
        "verify must agree on winner(0) over an identical entry set regardless \
         of how swarm-core's own Claims::winner was built: {:?}",
        verdict.i3
    );
}
