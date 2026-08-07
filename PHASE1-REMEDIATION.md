# Phase 1 Remediation Plan

> **Status:** done. All of Part A, B, and C executed and verified;
> `./scripts/verify.sh` exits 0 with the negative control genuinely failing.
> Originally written after a full audit of the M0–M6 tree.
> **Goal:** actually meet `DESIGN.md`'s Phase 1 exit criteria, and cut the tree
> back toward its stated size budget.
>
> **Read this whole file before touching anything.** Every claim below was
> verified against the code by running it, not inferred. Line numbers are from
> the tree as audited; re-locate by symbol name if they have drifted.

---

## 0. Context for a fresh session

`swarm-core` is a verifiable drone-swarm coordination layer. Design principle:
*act optimistically, prove accountably* — the swarm keeps moving under network
partition, and afterwards it can prove to an untrusting third party that no
member contradicted itself or exceeded its authority envelope.

- `DESIGN.md` — the single source of truth (Turkish). §9 holds the Phase 1
  roadmap (M0–M6) and the exit criteria. §11 holds the working rules.
- `docs/spec.md` — the technical spec, updated alongside the code.
- Crates: `swarm-core` (no_std, sans-I/O state machine), `swarm-sim`
  (deterministic simulator), `swarm-verify` (offline invariant checker).

The six invariants:

| # | Invariant |
|---|---|
| I1 | At most one signed entry per `(node, seq)` |
| I2 | An entry is not applied before its `deps` are delivered |
| I3 | Two nodes that saw the same entry set derive the same state |
| I4 | Spendable rights across all partitions ≤ authorised total |
| I5 | No safety-critical effect without a valid certificate in the log |
| I6 | Every effect traces back to a signed entry chain |

### The headline finding

**The protocol appears correct. The evidence layer is hollow.**

A sweep of 1500 configurations at `ticks=100`, plus a real 5000-case proptest
run, produced **zero invariant violations**. The recorded proptest regression
(`nodes:2, seed:12648182, ticks:100`) no longer reproduces. So this is not a
protocol rescue — it is a one-week repair of the verification apparatus, which
currently certifies far less than `DESIGN.md` claims it does.

### Ground rules

1. Do **not** start Phase 2. Exit criteria are unmet until this plan is done.
2. `DESIGN.md` §11.1 is non-negotiable: no `std::net`, `std::time`, `tokio`, or
   `rand::thread_rng` inside `swarm-core`. Time, network, randomness are
   parameters.
3. Every fix in Part A must **go red before it goes green.** Write the failing
   assertion, watch it fail, then fix the code. A test that never failed proves
   nothing — that is precisely the bug class this plan exists to remove.
4. Run tests with `--release`. Debug is unusable (see Task B4).
5. If code and doc disagree, update the doc (§11 rule 1) — but **never weaken
   an acceptance criterion to match a result.** See Task A5 for why.

---

## Part A — Fix the evidence layer (blocking)

These five tasks are what stand between the project and Phase 1's exit
criteria. Do them in order; A1 and A2 are the load-bearing ones.

---

### A1. `check_i4` is mathematically incapable of failing

**Severity: critical.** I4 is what `DESIGN.md` §9 calls the concept's
*"en satılabilir"* part — the answer to *"what if comms are lost entirely?"*
Its checker is a tautology.

**File:** `crates/swarm-verify/src/lib.rs`, `check_i4` (~line 145).

**The bug.** The checker reconstructs each node's budget from observable state:

```rust
let r = state.escrow().remaining(entry.node);
let s = spent_by_node.get(&entry.node).copied().unwrap_or(0);
budget_by_node.insert(entry.node, r.saturating_add(s));
```

But `Escrow::remaining()` is `alloc.saturating_sub(spent)`. So
`budget = (alloc − spent_local) + spent_global`, and the violation test
`spent_global > budget` reduces to `spent_local > alloc`. Because `remaining`
saturates at zero, that case pins `remaining = 0`, giving
`budget = spent_global` and the comparison `spent > spent`. **False in every
reachable state.**

**Proof it is vacuous.** A node allocated **3** authored signed `Spend` entries
totalling **40**; the checker reported `[]`.

**Fix.** Stop deriving the budget from the state being checked. Pass
allocations in explicitly:

```rust
pub fn check_invariants(
    states: &BTreeMap<NodeId, State>,
    budgets: &BTreeMap<NodeId, u64>,
) -> Vec<Violation>
```

Then `check_i4` compares `Σ unique Spend per node` against `budgets[node]`
directly. Update the three call sites (`m5_escrow.rs`, `m6_property.rs`, and
`swarm-verify`'s own unit tests) to pass the budget map the simulator already
builds from `SimConfig::budget_per_node`.

Consider exposing `Escrow::allocation(node)` so `swarm-sim` can hand the map
over without reconstructing it.

**Regression test.** Add this as `crates/swarm-verify/tests/i4_negative.rs`.
It must fail against today's code before you fix anything:

```rust
//! I4's negative control: a real overspend must be reported.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use swarm_core::causal::VersionVector;
use swarm_core::wire::{Body, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Envelope, Event, LogicalTime, NodeId, State};
use swarm_verify::check_invariants;

fn key(seed: u8) -> SigningKey {
    let mut b = [0u8; 32];
    b[0] = seed;
    SigningKey::from_bytes(&b)
}

/// Node A is allocated 3 units but authors signed Spend entries totalling 40.
/// A checker that does not report this is not checking anything.
#[test]
fn a_flagrant_overspend_is_reported() {
    let a = NodeId(0);
    let obs = NodeId(1);
    let (ka, kb) = (key(1), key(2));

    let mut keys = BTreeMap::new();
    keys.insert(a, ka.verifying_key());
    keys.insert(obs, kb.verifying_key());
    let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

    let mut budgets = BTreeMap::new();
    budgets.insert(a, 3);
    budgets.insert(obs, 3);

    let mut s = State::new(obs, roster, kb, 64, 8, 0, 0).with_budgets(budgets.clone());

    let mut prev = Hash::ZERO;
    for seq in 0..4u64 {
        let deps = {
            let mut vv = VersionVector::new();
            if seq > 0 {
                vv.bump(a, seq - 1);
            }
            vv
        };
        let e = UnsignedEntry {
            mission_id: PHASE1_MISSION_ID,
            epoch: PHASE1_EPOCH,
            node: a,
            seq,
            prev,
            deps,
            body: Body::Spend { amount: 10 },
        }
        .sign(&ka);
        prev = e.chain_hash();
        let (next, _) = step(
            &s,
            Event::Recv { from: a, payload: Envelope::Entry(e) },
            LogicalTime(seq + 1),
        );
        s = next;
    }

    let mut states = BTreeMap::new();
    states.insert(obs, s);

    let violations = check_invariants(&states, &budgets);
    assert!(
        violations.iter().any(|v| v.invariant == "I4"),
        "spent 40 against a budget of 3 and the checker said: {violations:?}"
    );
}
```

`swarm-verify/Cargo.toml` already has `ed25519-dalek` under `[dev-dependencies]`.

**Done when:** the test above fails on the current checker and passes after the
fix, and the full suite is still green.

---

### A2. Both "deliberately broken" tests never call the checker

**Severity: critical.** This is exactly `DESIGN.md` §9's exit criterion #1:
*"bilerek bozulmuş bir versiyonda kırılıyor... Testin bir şeyi gerçekten
yakaladığı kanıtlanmalı, yoksa yeşil ışık anlamsız."*

**Two offenders:**

1. `crates/swarm-sim/tests/m6_property.rs::i3_checker_catches_divergent_claims`
   (~line 61). It builds two `Claims` from two *different* entries and asserts
   they differ. It calls `check_invariants` only on the **clean** state. The
   divergent state never reaches the checker. It proves `Claims::observe`
   distinguishes different inputs — trivially true.
2. `crates/swarm-sim/tests/m5_escrow.rs::i4_check_catches_overspend_in_fabricated_entries`
   (~line 142). Ends with `assert!(spent_counted > 3)`. It asserts `4 > 3`.
   It never calls `check_invariants` either.

`docs/spec.md:1152` states the opposite in writing:

> "The test includes a deliberate-bug case that fabricates Spending beyond
> budget and verifies the check catches it — so the positive tests are not
> vacuously passing."

That sentence is false. A1's probe is the proof.

**Fix.** The original `DESIGN.md` text named the right experiment: *break the
tie-break rule and watch the checker fire.* Do that properly, with a real
mutation of the production code rather than a hand-built fake:

1. Add a `mutant-i3` feature to `swarm-core/Cargo.toml` (off by default,
   never enabled in a normal build).
2. Behind it, change the winner rule in `crates/swarm-core/src/state.rs` so it
   is no longer deterministic across nodes — e.g. `Claims::winner` returns
   `self.by_task.get(&task)?.last()` (max instead of min) under
   `#[cfg(feature = "mutant-i3")]`. Because `Claim`'s `Ord` *is* the winner
   rule, this is a genuine divergence of derived state.
3. Add a test that runs the same simulation twice — once clean, once with the
   mutant — and asserts `check_invariants` returns empty for the first and a
   non-empty I3 violation for the second.

Because a cargo feature cannot be toggled inside one test binary, drive it from
a small script or a `#[ignore]`d test invoked explicitly:

```bash
cargo test --release -p swarm-sim --test m6_property   # clean: green
cargo test --release -p swarm-core --features mutant-i3 \
  -p swarm-sim --test m6_property                      # mutant: must FAIL
```

Wire both into `scripts/verify.sh` (Task A5) so the pair is checked together
and the second one failing is the *expected* outcome.

Delete the two tautological tests, or rewrite them to call `check_invariants`
on the broken state. Either way they must not remain as-is.

**Done when:** a mutated build makes `check_invariants` report an I3 violation,
and the clean build does not. Both directions are asserted by something that
runs.

---

### A3. "5000 seeds" is 256 seeds

**Severity: high** (false claim; the underlying property does hold).

**File:** `crates/swarm-sim/tests/m6_property.rs`, the `proptest! { … }` block.

There is no `ProptestConfig` anywhere in the repo, no `proptest.toml`, and no
`PROPTEST_CASES` in the environment. proptest's default is **256**. The test is
named `m6_all_invariants_hold_across_five_thousand_seeds`. `DESIGN.md` §9 M6
and `docs/spec.md:1323` both claim 5000.

**Measured:**

| Run | Time |
|---|---|
| default (as committed) | 3.74 s |
| `PROPTEST_CASES=5000` | 74.21 s |

A ~20× ratio — exactly 256 → 5000.

**Fix:**

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(5000))]

    #[test]
    fn m6_all_invariants_hold_across_five_thousand_seeds(
        cfg in sim_config_strategy()
    ) { … }
}
```

**Good news:** it already passes at 5000. This is a one-line fix that makes the
name honest. ~75 s in release is acceptable.

**Also fix the strategy.** It hardcodes `ticks: 50`, but
`m6_property.proptest-regressions` records a past failure at `ticks: 100`.
proptest regenerates persisted cases through the *current* strategy, so that
recorded case is no longer reproduced under its original conditions. I swept
1500 configs at `ticks=100` and found **zero** violations, so no live bug is
being hidden — but make `ticks` part of the strategy
(`50u64..=150`) so the regression file means what it claims.

**Done when:** 5000 cases run, `ticks` is generated rather than fixed, and the
suite is green.

---

### A4. I5 and I6 are not checked at all

**Severity: medium** (documentation is wrong; the structural argument is
partly sound).

`check_invariants` runs `check_i1` … `check_i4` only. `DESIGN.md` §9 M6 claims
*"I1–I6 çalıştırılabilir kontroller haline getirilip."* `docs/spec.md:1352` is
honest ("checking I1–I4 (I5/I6 are structural)"); `DESIGN.md` is not.

The I5 "test" at `crates/swarm-core/tests/invariants.rs:472` is:

```rust
assert_eq!(Class::Degradable as u8, 0);
assert_eq!(Class::ExclusiveCostly as u8, 1);
assert_eq!(Class::SafetyCritical as u8, 2);
```

It asserts enum discriminant values. I5 currently holds because
`SafetyCriticalAction` deliberately does not implement `Action` — i.e. code
that was never written cannot run. That is a real compile-time property, but it
is not an executable check and should not be described as one.

**Fix — pick one and make the docs match:**

- **Option A (recommended, cheap).** Keep I5/I6 structural. Correct `DESIGN.md`
  §9 M6 to say "I1–I4 executable, I5–I6 structural," and explain *why* each is
  structural. Replace the discriminant assertion with a `compile_fail` doctest
  proving `commit` cannot be called on a `SafetyCritical` action without a
  certificate — that at least tests the actual claim.
- **Option B (more work).** Implement a real I6 check in `swarm-verify`: for
  every `Effect::Send` observed in the trace, assert the carried entry exists
  in the sender's log with a valid signature. This needs the trace, not just
  final states, so `check_invariants` would take `&Trace` too.

Do **not** leave `DESIGN.md` claiming six executable checks when four exist.

**Done when:** the docs describe what is actually verified, and the I5 test
tests I5 (or is removed as unable to).

---

### A5. Stop weakening acceptance criteria; add a single verify entrypoint

**Severity: process — this is the root cause of A1–A4.**

`git diff DESIGN.md` shows the M6 criterion was edited:

```diff
-**Bitti sayılır:** ... bilerek bozulmuş bir versiyonda (örn. tie-break node_id
- yerine rastgele yapılınca) test kırılıyor. Testin bir şeyi gerçekten
- yakaladığı kanıtlanmalı, yoksa yeşil ışık anlamsız.
+> ✅ **Done.** ...
+**Bitti sayılır:** ... bilerek bozulmuş bir versiyonda (I3: aynı entry set,
+ farklı claims) test kırılıyor. Testin bir şeyi gerçekten yakaladığı kanıtlandı.
```

The specific, hard test was replaced with a vaguer one, marked ✅ — and the
vaguer one still isn't implemented. `DESIGN.md` §11 permits updating the doc
when code and decision conflict; it does not permit lowering the bar to clear
it. This is the exact failure mode §11 exists to prevent.

**Actions:**

1. Restore the original M6 criterion text in `DESIGN.md`, and remove the ✅
   until Tasks A1–A4 are done.
2. Add `scripts/verify.sh` — the one command that decides whether Phase 1 is
   met:

```bash
#!/usr/bin/env bash
# The Phase 1 exit gate. Green here means the criteria are met — nothing else does.
set -euo pipefail

echo "== clean build: every invariant must hold =="
cargo test --workspace --release

echo "== negative control: the I3 mutant MUST fail =="
if cargo test --release -p swarm-core --features mutant-i3 \
     -p swarm-sim --test m6_property 2>/dev/null; then
  echo "FAIL: the checker did not catch the deliberately broken build." >&2
  exit 1
fi
echo "OK: the checker caught the mutant."
```

3. Record in `docs/spec.md` that `scripts/verify.sh` is the exit gate.

**Done when:** `scripts/verify.sh` exits 0 and its negative control genuinely
fails on a mutated build.

---

## Part B — Minimize (~2,000 lines)

The tree is **8,916 lines**. `DESIGN.md` §9 budgets *"~600-800 satır"*. That is
11×. Overshoot is expected; this much is not, and most of it is explicitly
out of scope.

Do Part B *after* Part A — deleting code while the checker is broken means
losing the ability to tell whether a deletion broke something.

### B1. Delete `plans/` — 373 KB

A single dumped chat transcript with a broken filename
(`i have a  @swarm-core  which is verifiable drone swarm coordination…​.md`).
Not referenced anywhere. Pure junk.

```bash
git rm -r --cached plans/ 2>/dev/null || true
rm -rf plans/
```

Add `plans/` to `.gitignore` if you want a scratch area.

### B2. Delete `demo/` — 1,186 lines

The single largest file in the project. `DESIGN.md` §9's out-of-scope table:

| Yok | Neden erteliyoruz |
|---|---|
| GUI, görselleştirme | Terminal çıktısı ve test sonucu yeter |

It is an animated, menu-driven ASCII-art app with `thread::sleep` and a
`#[allow(clippy::disallowed_methods)]` to get past the project's own lint. It
sits outside the workspace (its own empty `[workspace]` table), so
`cargo test --workspace` never compiles it — it will rot silently. It covers
only M0–M4; escrow and the invariant checker are absent.

Replace it with **one** non-interactive binary that satisfies exit criterion #2
(see Task C1). If you want to keep the visual work, move it to a branch first —
do not keep it on `main`.

### B3. Delete the dead policy stubs — ~45 lines

**File:** `crates/swarm-core/src/policy.rs`.

Verified **zero references outside their own file**:

- `QuorumCert`
- `OperatorSig`
- `GlobalThresholdCert`
- `PolicyError`
- `ExclusiveCostlyAction`
- `SafetyCriticalAction`

`DESIGN.md` §11.4: *"Her `Body` varyantı bir testle gelir. Kullanılmayan
varyant eklenmez."* And §9's Entry guidance: *"'İleride lazım olur' diye
eklenen her varyant, henüz hiç kullanılmamışken tasarım borcu üretir."*

**Caveat:** `SafetyCriticalAction`'s non-existence *is* the current I5
argument (A4). If you take A4 Option A, keep `Class` and the `Action` trait —
they are load-bearing — and delete the rest, then restate the I5 argument as
"no `Action` impl exists with `CLASS == SafetyCritical`," which is checkable
by a `compile_fail` doctest and does not need the stub structs.

`PolicyError` currently has one variant that is documented as unreachable; if
you delete it, `commit` returns `Result<(), Infallible>` or just `()`.

### B4. Consolidate the example binaries — ~350 lines

| File | Lines |
|---|---|
| `crates/swarm-sim/examples/claim.rs` | 207 |
| `crates/swarm-sim/examples/watch.rs` | 177 |
| `crates/swarm-sim/examples/converge.rs` | 174 |
| `crates/swarm-sim/examples/demo.rs` | 60 |

618 lines across four overlapping terminal renderers. Fold them into one
`examples/run.rs` with a `--scenario {determinism,converge,claim,watch}` flag,
reusing `swarm-sim/src/demo.rs`'s helpers.

**Keep:**
- `crates/swarm-core/examples/chain.rs` (126) — the M1 tamper-detection demo,
  the concrete evidence for the tamper-evidence claim.
- `crates/swarm-sim/src/demo.rs` (91) — the shared-helper exception is well
  argued in its own module doc.

### B5. Make the test suite usable

`cargo test --workspace` in debug ran **over 20 minutes** before being killed
(`m5_escrow` alone). Release: **2.5 minutes**. Cause is ed25519 signing in an
unoptimised build.

Add to the root `Cargo.toml`:

```toml
[profile.test]
opt-level = 2

[profile.dev.package.ed25519-dalek]
opt-level = 3
[profile.dev.package.blake3]
opt-level = 3
[profile.dev.package.curve25519-dalek]
opt-level = 3
```

Also fix the one live clippy warning: `State::new` takes 8 arguments (limit 7).
A `NodeConfig` struct for `log_cap` / `buffer_cap` / `entry_period` /
`anti_entropy_period` fixes it and reads better at every call site.

---

## Part C — Meet the remaining exit criteria

`DESIGN.md` §9 lists three. Part A covers #1. These cover #2 and #3.

### C1. The 90-second demo (criterion #2)

> *"Terminalde 90 saniyede anlatılabilen bir demo var: 5 node, bölünme,
> çalışmaya devam, birleşme, yakınsama, hilecinin ifşası."*

This does not exist. There are five separate example binaries plus a menu-driven
app, and no single artifact tells the whole story.

Build **one** non-interactive binary — `crates/swarm-sim/examples/phase1.rs` —
that runs a single scripted scenario end to end and narrates it:

1. 5 nodes, connected, claiming tasks.
2. Partition `{A,B} | {C,D,E}`. Both sides **keep working** — show entries
   still being authored on both sides. This is the anti-BFT point.
3. Both sides claim the same task.
4. Heal. Show every node converging on the **same winner**, and the loser's
   `Withdraw` record in its own log.
5. Node F equivocates. Show two honest nodes independently producing
   **byte-identical** proofs, then a third party verifying one with nothing
   but the roster — no simulator, no trace, no agreement.
6. Print the final `check_invariants` result: empty.

Constraints: no `sleep`, no menu, no animation. Deterministic, seeded, and
diffable — running it twice with the same seed must produce identical bytes.
Under 90 seconds of *reading*, not 90 seconds of runtime.

### C2. Make `spec.md` true and readable (criterion #3)

> *"`spec.md` başkasının okuyup anlayabileceği durumda."*

At 1,352 lines it is thorough and mostly excellent. But it contains at least
one statement now known to be false, and the invariant table oversells:

- **`docs/spec.md:1152`** — the "deliberate-bug case … verifies the check
  catches it" sentence. False (A2). Rewrite once A1/A2 land.
- **`docs/spec.md:1207`** (I4 row) — "Tested … by `m5_escrow.rs` (1000 seeds)."
  True that it runs, but the check it ran was vacuous. Restate after A1.
- **`docs/spec.md:1323`** — "5000 random seeds." Becomes true after A3.
- **I5/I6 rows** — mark as *structurally* discharged, not executable checks.

Then re-read §12.4 on escrow with fresh eyes and see C3.

### C3. Be honest about what M5 actually demonstrates

There is no budget transfer — `docs/spec.md` §12 puts it out of scope. So
escrow reduces to a local `if remaining >= 1` in `step`, and I4 holds because
that if-statement holds. The genuinely interesting part of bounded counters —
**redistribution under partition**, which is where the handshake and the real
safety argument live — is exactly what's missing.

`DESIGN.md` §9 calls M5 *"konseptin en satılabilir parçası."* As built it
demonstrates considerably less than that billing. Either:

- **(a)** implement escrow *transfer* (two-phase, partition-internal), which is
  what makes the claim interesting; or
- **(b)** downgrade the claim in `DESIGN.md` and `spec.md` to what's true:
  "per-node static allocation, no transfer — global bound is the trivial
  consequence of local caps."

(b) is honest and costs an hour. (a) is real work and is arguably Phase 2. Pick
deliberately and write down which.

---

## Part D — Strategic notes (not code; read before Phase 2)

Do not act on these in the execution session. They are recorded so they are not
lost, and because they should shape what happens after this plan lands.

### D1. Prior art — read PeerReview before writing more protocol

**PeerReview** (Haeberlen, Kouznetsov, Druschel, SOSP 2007) is accountability
for distributed systems via tamper-evident per-node hash logs, witness sets that
cross-check each other, and deterministic state-machine replay to detect
deviation. That is `DESIGN.md` §4.3, §4.4, and `swarm-verify` — from 2007, and
*stronger*, because replay catches arbitrary deviation rather than only
equivocation.

The other primitives are equally settled: causal broadcast (Birman, ~1987),
CRDTs (Shapiro et al., 2011), escrow / bounded counters (O'Neil 1986;
Balegas et al. 2015), equivocation-as-slashable-evidence (Ethereum PoS),
gossiped tamper-evident logs (Certificate Transparency).

**The protocol contribution here is approximately zero.** What exists is a
*composition* and a *framing*: accountability-instead-of-consensus, applied to
EW-contested drone swarms, over a sans-I/O core positioned for a folding/ZK
backend. That framing is defensible and I have not seen it packaged this way for
this domain — but it is a positioning and integration play, not a research
result. Anyone who knows the literature will ask the PeerReview question in the
first five minutes. Have an answer.

### D2. The core claim is narrower than the pitch

`DESIGN.md` §1 promises proving *"hiçbir üye yetki zarfının dışına çıkmadı."*
What is actually provable is: nobody contradicted **itself**, and nobody
exceeded a **local counter**.

§4.4 already admits the gap — a node that lies *consistently* produces a
perfect chain. Credit for writing that down; most projects hide it. But it
means "verifiable" here means "verifiably self-consistent," and a consistent
liar is precisely the military threat model. That pushes the strongest near-term
buyer toward the **civil liability / UTM regulator** case (prove compliance
without exposing raw telemetry) rather than the EW case. Decide which story you
are actually telling.

### D3. Crash monotonicity is untouched

`DESIGN.md` §4.3 names it *"En tehlikeli kriter"*: a node that crashes, loses
its log tail, and reuses a `seq` **accidentally equivocates and convicts
itself**.

`crates/swarm-core/src/log.rs:80` claims this *"holds structurally here, because
a pure state machine has no persistent tail to lose."* True, and vacuous — it
holds because there is no persistence at all. The moment Phase 2 adds real I/O
this is unsolved, and §4.3's own remedy (fsync the `seq` *before* sending, or a
secure-element monotonic counter) has not been designed.

**This is the first thing Phase 2 must confront, not the last.** Add the design
to `spec.md` before writing any transport code.

### D4. What to do after this plan

Stop coding and go test the thesis. Whether the EW framing survives contact with
someone who buys defence software is not a Rust question, and no additional Rust
will answer it. The strongest asset for that conversation is the M4 equivocation
proof — it is real, it works, and it demonstrates in one screen why consensus
was the wrong primitive.

---

## Execution checklist

Work top to bottom. Do not skip the "must go red first" steps.

**Part A — blocking**
- [x] A1 — write `i4_negative.rs`; confirm it **fails**; fix `check_i4` to take
      budgets explicitly; confirm it passes
- [x] A2 — add the `mutant-i3` feature; confirm the mutant build makes
      `check_invariants` report I3; delete/rewrite both tautological tests.
      The mutation actually landed on `Claims::winner`'s tie-break (a
      self-preferring node bias), not `observe`'s fold — a symmetric change
      to the fold is applied identically by every node in-process and
      produces no cross-node divergence to catch; the tie-break is what
      makes the divergence node-dependent. Verified empirically both ways.
- [x] A3 — `ProptestConfig::with_cases(5000)`; make `ticks` generated
- [x] A4 — pick Option A or B; make `DESIGN.md` match reality on I5/I6.
      Took Option A, plus discovered and fixed a second inaccuracy while
      writing the doctest: `Cert` is not actually type-tied to `Class` in
      the `Action` trait, so the pre-existing "non-`()` Cert type" framing
      of I5 was itself imprecise. Corrected in `DESIGN.md`, `docs/spec.md`,
      and `policy.rs`'s own comments.
- [x] A5 — restore the original M6 criterion; add `scripts/verify.sh`

**Part B — minimize**
- [x] B1 — delete `plans/` (373 KB)
- [x] B2 — delete `demo/` (1,186 lines); branch it first if you want to keep it.
      Not branched — untracked, never committed, so there was nothing to lose.
- [x] B3 — delete the dead policy symbols. Kept `SafetyCriticalAction`
      (5 of 6 deleted, not 6): A4's `compile_fail` doctest needs a concrete
      type to demonstrate the "no `Action` impl" claim on, and it is now
      referenced from `docs/spec.md` and `invariants.rs` — no longer dead.
- [x] B4 — fold four examples into one `run.rs` (~350 lines)
- [x] B5 — `[profile.test] opt-level = 2`. `State::new` was already at 7 args
      (a prior fix, before this session); the actual clippy hit was `emit`
      in `swarm-sim/src/sim.rs` (8 args) — fixed with a `Runtime` bundle.

**Part C — exit criteria**
- [x] C1 — one non-interactive `phase1.rs` telling the whole story. Two
      simulations, not one: folding the equivocator's conflicting genesis
      entries into the same `check_invariants` call as the 5-node partition
      story makes `check_i1` report a real (and correct) I1 flag on the
      union of what honest nodes hold — see the file's own module doc for
      why that's not a bug to hide, and why the two stories run separately.
- [x] C2 — correct `spec.md`'s I1-I6 status table and M6 roadmap entry
      (line numbers had drifted from the audit; re-found by content). Also
      fixed the same "I1-I6 executable" overclaim in `swarm-verify`'s own
      module doc, its `Cargo.toml` description, and `m6_property.rs`'s doc
      comment — same false claim, different files.
- [x] C3 — downgraded the M5 claim in `DESIGN.md` (Option b): no transfer,
      per-node static allocation, global bound is the trivial consequence of
      local caps. `docs/spec.md` already scoped transfers out honestly;
      only `DESIGN.md`'s "en satılabilir" framing needed the caveat.

**Gate**
- [x] `./scripts/verify.sh` exits 0, and its negative control genuinely fails
- [x] Line count materially down from 8,916 (8,041 `.rs` lines remaining,
      after both the `demo/` deletion and real net additions: `phase1.rs`,
      `i4_negative.rs`, the `mutant-i3` apparatus, and expanded honesty-fix
      commentary)
- [x] `DESIGN.md` contains no ✅ that this plan has not earned (zero ✅
      markers remain anywhere in the file — M0-M5 never used the convention
      either, so removing M6's premature one rather than re-earning it
      matches the rest of the document)

---

## Reference: verified evidence

Everything asserted above was reproduced by running the code.

| Claim | Evidence |
|---|---|
| `check_i4` cannot fail | Budget 3, spent 40 → `violations = []` |
| "5000 seeds" is 256 | 3.74 s default vs 74.21 s at `PROPTEST_CASES=5000` (~20×) |
| Protocol is likely sound | 1500 configs at `ticks=100` + 5000 real proptest cases → zero violations |
| Recorded regression is stale | `nodes:2, seed:12648182, ticks:100` → `[]` |
| Policy stubs are dead | Zero references outside `policy.rs` for all six symbols |
| Debug suite unusable | `m5_escrow` >20 min debug; whole workspace 2.5 min release |
| Criterion was weakened | `git diff DESIGN.md` — M6 block |
| Full suite currently green | `cargo test --workspace --release` — all pass, 2:29 |

**Note:** the audit created and removed two temporary probe files. The working
tree was left exactly as found; nothing in this plan has been executed.
