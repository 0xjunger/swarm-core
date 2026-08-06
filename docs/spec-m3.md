# swarm-core — M3 Normative Specification

> `DESIGN.md` (Turkish) is the project's source of truth. `docs/spec.md` (M0),
> `docs/spec-m1.md` (M1) and `docs/spec-m2.md` (M2) remain binding unchanged.
> M3 does not touch the channel semantics, the determinism contract, the
> causal-delivery rule, the anti-entropy rule, or the byte layout of any
> already-specified field. This file is the normative specification for **M3**,
> written before the code per `DESIGN.md` §11.6.
>
> **Status: M3.** Sections marked *Normative — M3* are binding.

---

## 1. Scope

Phase 1, milestone M3: the task-claim CRDT (`DESIGN.md` §9, M3 verbatim):
"Şimdi kayıtların bir *anlamı* oluyor: '7 numaralı görevi ben üstleniyorum'.
İki partisyon aynı görevi talep ederse, birleşmede deterministik bir kazanan
çıkıyor, kaybeden geri çekiliyor."

**M3 acceptance** (`DESIGN.md` §9, "Bitti sayılır"): two partitions claim the
same task; after healing, **both nodes compute the same winner** (nobody
believes it won), and **the losing node's log contains a withdrawal record**.

The winner rule is fixed by `DESIGN.md` §4.2: `min by (priority,
logical_clock, node_id)`.

M2 left the `Body` a placeholder in practice: `step`'s `Tick` arm authored
`TaskClaim { task: next_seq(), priority: 1 }`, so every node claimed a task
numbered by its own `seq` and two nodes never contested anything. M3 gives the
body its meaning.

Out of scope, deliberately (§11): the LWW telemetry register and the sensor
track OR-set of `DESIGN.md` §4.2 — M3's milestone text names task claims only.

---

## 2. `Body::Withdraw`

*Normative — M3.*

```rust
pub enum Body {
    TaskClaim { task: u64, priority: u8 },   // tag 0x00, unchanged
    Withdraw { task: u64 },                  // tag 0x01, new at M3
}
```

Encoding, extending `docs/spec-m1.md` §3.3's table without altering its
existing row:

| Variant | Tag | Fields |
|---|---|---|
| `TaskClaim` | `0x00` | `task` (8 bytes, u64 BE) `\|\|` `priority` (1 byte, u8) |
| **`Withdraw`** | **`0x01`** | **`task` (8 bytes, u64 BE)** |

`TaskClaim`'s bytes are untouched, so **both existing golden vectors still
pass byte for byte** (`docs/spec-m1.md` §7, `docs/spec-m2.md` §10). A new
variant adds a tag; it does not change an existing encoding. A third golden
vector pins `Withdraw` (§10), per `DESIGN.md` §11.4 — every `Body` variant
arrives with a test.

**Meaning.** `Withdraw { task }` is the author's record that it claimed `task`,
observed that it is not the winner, and is standing down. It is an entry in the
author's own log — the "geri çekilme kaydı" the acceptance criterion asks for —
and it is *not* a CRDT operation: see §4.

---

## 3. `logical_clock`: derived from `deps`

*Normative — M3.*

`DESIGN.md` §4.2's winner rule needs three values. `priority` and `node_id` are
already fields of the entry. The third, `logical_clock`, is **derived** — no
new field, no wire-format change:

```
lc(e) = Σ over (n, s) ∈ e.deps of (s + 1)
```

That is: the number of entries `e`'s author had applied at the moment it
authored `e`. `VersionVector` counts from `seq = 0`, so a component `(n, s)`
represents `s + 1` entries from `n`.

**Decision, stated explicitly — confirmed with the project owner before
implementation.** The alternative was an explicit `lamport: u64` field on
`TaskClaim`. It was rejected because it changes the frozen wire format:
both golden vectors would need regenerating and `docs/spec-m1.md` §3.3's table
rewriting, for a value the entry already determines. `docs/spec-m1.md` §5 opened
`priority` early *precisely* so M3's rule would need no format change; deriving
`logical_clock` completes that intent rather than departing from it.

### 3.1 Why this is a logical clock

The property the tie-break needs is that a causally later claim never beats a
causally earlier one. It holds:

> **Claim.** If `e1 → e2` (e1 happens-before e2), then `lc(e1) < lc(e2)`.
>
> **Proof.** `e2`'s author applied `e1` before authoring `e2`, so
> `deps(e2)[node(e1)] ≥ seq(e1)`. Causal delivery (`docs/spec-m2.md` §4) means
> that author could not have applied `e1` until `deps(e1) ≤ its own vector`, so
> at authorship time its vector dominated `deps(e1)` componentwise. `deps(e2)`
> is a snapshot of that vector, hence `deps(e2) ≥ deps(e1)` componentwise. It
> is additionally strictly greater in the `node(e1)` component, because
> `deps(e1)` names at most `seq(e1) − 1` there (an entry's own `deps` is taken
> *before* its append, `docs/spec-m2.md` §3) while `deps(e2)` names at least
> `seq(e1)`. Summing `(s + 1)` over a componentwise-≥ vector that is strictly
> greater in one component, and may carry extra components (each contributing
> ≥ 1), gives `lc(e2) ≥ lc(e1) + 1`. ∎

Concurrent entries may share an `lc`; that is expected of any logical clock and
is why `node_id` follows it in the ordering.

**No wall clock, no randomness.** `lc` is a pure function of bytes already
inside the signed entry, so it cannot be spoofed independently of the signature
— `DESIGN.md` §7's objection to wall-clock tie-breaks ("GPS spoof edilebilir,
yani saldırgan claim yarışını kazanır") does not apply. A node *can* understate
its own `deps` to lower its `lc`, but doing so also weakens its causal
position and, from M4 onward, is visible in its chain; preventing it is not
M3's problem.

### 3.2 Honest limit

`lc` counts *all* entries the author had seen, not only task-related ones.
Under a partition, a node in the larger group accumulates entries faster and so
carries a larger `lc` than an isolated node. The isolated node therefore tends
to win contested tasks. This is inherent to Lamport-style clocks (an explicit
`L = max(L, L_recv) + 1` counter behaves identically) and is deterministic and
causally sound, but it is stated here so it is not mistaken for a bug when the
M3 demo shows it.

---

## 4. The claim CRDT

*Normative — M3.* `DESIGN.md` §4.2: `Map<TaskId, ORSet<Claim>>`.

```rust
pub type TaskId = u64;

pub struct Claim { priority: u8, lc: u64, node: NodeId, seq: u64 }

pub struct Claims {
    by_task:   BTreeMap<TaskId, BTreeSet<Claim>>,
    withdrawn: BTreeSet<(TaskId, NodeId)>,
}
```

**The OR-set's unique tag is `(node, seq)`** — the identity of the entry that
carries the claim. That is what makes this an OR-set rather than a plain set:
two nodes claiming the same task with the same priority and the same `lc`
produce two distinct elements, never one merged element. Because the tag comes
from the entry, it needs no separate generation and no randomness.

**Folding.** Exactly one function folds an entry into `Claims`, and it takes a
`VerifiedEntry`, never an `Entry` (`DESIGN.md`, "Entry ile nasıl çalışmalı",
item 4 — unverified bytes must not reach state):

- `TaskClaim { task, priority }` from entry `e` inserts
  `Claim { priority, lc: lc(e), node: e.node, seq: e.seq }` into
  `by_task[task]`.
- `Withdraw { task }` from entry `e` inserts `(task, e.node)` into `withdrawn`.

Both are set insertions, so folding is idempotent and commutative: the same
entry set yields the same `Claims` regardless of arrival order. That is I3
(§9) discharged structurally rather than by argument.

### 4.1 `remove` is not implemented, and why

*Normative — M3.* `Withdraw` does **not** remove the author's claim from
`by_task`. The claim set is grow-only.

**Decision, stated explicitly — confirmed with the project owner.** Making
`Withdraw` an OR-set removal was considered and rejected for M3. It would
require tombstone bookkeeping and, with it, the causal-stability-based GC that
`DESIGN.md` §4.2 and §7 both flag as mandatory before tombstones may exist
("tombstone GC'si için causal stability kullan; yoksa state monoton büyür"),
and §11.6 forbids that decision entering code before it is specified. None of
that is needed by M3's acceptance criterion, which asks for a record in the
loser's *log*, not for a mutation of the claim set.

The consequence is a stronger property, not a weaker one: with a grow-only
set, the winner is a pure `min` over that set, so §5's monotonicity holds and
convergence needs no argument beyond set-union commutativity. An OR-set whose
`remove` has no call site is not added (`DESIGN.md` §11.4).

The honest limit: a node cannot un-claim a task. Nothing in Phase 1 needs to.

### 4.2 Bound

`Claims` has no cap of its own. It is bounded transitively, by the same
argument `docs/spec-m2.md` §7 makes for `origins`: every element comes from an
applied entry, and every node's own `Log::append` refuses past `log_cap`
(`docs/spec-m1.md` §6), so no node can hold more than `N × log_cap` claims and
withdrawals combined.

---

## 5. The winner rule

*Normative — M3.*

```
winner(task) = min of by_task[task] by (priority, lc, node, seq)
             = None if the task has no claims
```

The first three keys are `DESIGN.md` §4.2's rule verbatim. **`seq` is appended
purely for totality**: without it two claims could compare equal only if the
same node claimed the same task twice with the same `lc`, and §6's authoring
rule never produces that. It exists so the ordering is total by construction
rather than by assumption — the same reasoning `docs/spec.md` §5.1 gives for
`enqueue_seq`.

`Claim`'s field order is chosen so that the derived `Ord` **is** this rule and
`BTreeSet::first()` **is** `winner`. There is no second place where the
ordering could drift out of sync with the spec.

### 5.1 Losing is monotone

*Normative — M3.* Because `by_task[task]` only ever grows (§4.1) and the
winner is its minimum, the minimum can only decrease. Therefore:

> Once a node is not the winner of a task, no future entry can make it the
> winner again.

Two consequences the implementation depends on:

1. A `Withdraw` is never regretted, so a node authors at most one per task
   (§6), and the log-growth bound of §4.2 holds.
2. Two nodes that have seen the same entry set agree on the winner (§9, I3),
   and neither can later be contradicted by an entry it has not yet seen
   *becoming* less authoritative.

---

## 6. Authoring: what a node writes, and when

*Normative — M3.*

**All entry authorship happens in the `Event::Tick` arm, never in
`Event::Recv`.** Delivering an entry may emit anti-entropy fill replies
(`docs/spec-m2.md` §6) but never authors a new entry. `tests/causal.rs`'s
`recv` helper already asserts this (`"delivering an entry never itself
replies"`); M3 makes it a stated rule rather than an accident of M2's shape,
because a node that authored while draining its causal buffer would interleave
authorship with the fixed-point drain of `docs/spec-m2.md` §4.

Fixed order within one tick — extends `docs/spec-m2.md` §6's two steps to
three:

1. **Claim.** If `entry_period` fires: author and broadcast
   `TaskClaim { task: k, priority: 1 }`, where `k` is the number of
   `TaskClaim` entries already in this node's own log. Every node therefore
   claims tasks `0, 1, 2, …` in order, so **every task is contested by every
   node** and M3's scenario is the default behaviour rather than a special
   configuration.
2. **Withdraw.** Then, in the same tick, for every task `t` this node has
   claimed, **ascending by `t`** (rule R4's spirit): if
   `winner(t).node != me` and this node's own log holds no
   `Withdraw { task: t }` yet, author and broadcast `Withdraw { task: t }`.
   By §5.1 this fires at most once per task. Emitting all pending withdrawals
   rather than one per tick keeps the rule stateless; the burst is bounded by
   §4.2 and flows through the already-bounded network queue (`docs/spec.md`
   §5.5).
3. **Advertise.** If `anti_entropy_period` fires: broadcast the version vector,
   unchanged from `docs/spec-m2.md` §6.

`k` and "which tasks am I still owed a withdrawal for" are both **derived from
the node's own log** on each tick, never stored in a separate counter. A
redundant counter could drift out of step with the log after a refused append;
a derived value cannot. Cost is a scan of a `log_cap`-bounded vector per tick,
which `DESIGN.md` §9 explicitly declines to optimise at Phase 1 scale.

A full log (`LogError::Full`) remains a silent no-op at every step above, as
at M2: graceful degradation, not a crash.

`priority` is fixed at `1` for autonomously authored claims. A per-node
priority knob is deferred (§11): nothing in Phase 1 produces priorities, and
an unused configuration field is exactly what `DESIGN.md` §11.4 forbids. The
`priority` term of the winner rule is exercised by hand-built claims in
`swarm-core/tests/claims.rs`.

---

## 7. `State`'s shape at M3

*Normative — M3.* One field is added to `docs/spec-m2.md` §7's struct:

```rust
pub struct State {
    // ... unchanged from docs/spec-m2.md §7 ...
    claims: state::Claims,
}
```

**`State::new`'s signature does not change.** No new construction parameter is
needed: the task number is derived (§6) and `priority` is fixed (§6).

`Claims` is advanced in exactly two places, both of which already exist:

- causal delivery's apply step, beside `causal_vv.bump` — the entry is a
  `VerifiedEntry` there by construction;
- the `Tick` arm, for the node's own freshly appended entry. That entry is
  verified by construction — this node just signed it with its own key over
  its own chain head — so it is wrapped through the crate-private
  `VerifiedEntry` constructor rather than round-tripped through
  `log::verify_next`. The type gate of `docs/spec-m1.md` §4.5 is preserved:
  the public API still cannot fold an unverified `Entry`.

---

## 8. Bandwidth budget

*Normative — M3, per `DESIGN.md` §7's "baştan hesapla" requirement.*

From the frozen encoding, updating `docs/spec-m2.md` §8:

```
Entry (TaskClaim) = 165 + 9·D bytes    (unchanged)
Entry (Withdraw)  = 164 + 9·D bytes    (body is 9 B: 1 tag + 8 task)
VersionVector     = 2 + 9·N bytes      (unchanged)
```

At the roster cap `N ≤ 20` (`DESIGN.md` §4.5), a `Withdraw` entry is ≤ 344 B.
M3 raises the steady-state entry rate: in the worst case a node emits one
claim plus one withdrawal per `entry_period` instead of one claim, so entry
traffic at most doubles. Still self-limited by the bounded network queue
(`docs/spec.md` §5.5); no new cap is introduced.

---

## 9. Invariants

*Normative — M3.* Updates `docs/spec-m2.md` §9's table for I3 only.

| # | Invariant | Status at M3 |
|---|---|---|
| I1 | At most one signed entry per `(node, seq)` | Unchanged: binding, tested in `swarm-core/tests/invariants.rs`. |
| I2 | An entry is not applied before its `deps` are delivered | Unchanged from M2: binding, `docs/spec-m2.md` §4 is the enforcement. |
| **I3** | Two nodes that have seen the same entry set derive the same state | **Strengthened.** "Derived state" at M2 meant `causal_vv` and the entry set. At M3 it additionally means `claims` **and `winner(t)` for every task `t`** — the first genuinely derived, order-sensitive-looking state in the system. Discharged structurally by §4 (set insertion is commutative and idempotent) and §5.1; tested in `swarm-core/tests/invariants.rs` and end to end by `swarm-sim/tests/m3_claim.rs`. |
| I4 | Spendable rights across all partitions ≤ authorised total | Documented placeholder — activates at M5 (escrow). |
| I5 | No safety-critical effect without a valid certificate in the log | Documented placeholder — activates with the policy gate (M5). |
| I6 | Every effect is traceable to a signed entry chain | Documented placeholder — M3 extends M2's partial start (a withdrawal is an effect traceable to the claims that caused it), but the full policy-gated claim is M5+. |

---

## 10. Golden vector

*Normative — M3.* The M1 vector (empty `deps`, `TaskClaim`) and the M2 vector
(populated `deps`, `TaskClaim`) are **unchanged and must stay byte-identical**;
if either moves, something has silently altered an already-frozen encoding. A
third vector is added to `swarm-core/tests/golden_vector.rs` pinning an `Entry`
whose body is `Withdraw`, proving tag `0x01` and its single `u64 BE` field
(§2).

---

## 11. Deferred

Recorded so they are not silently decided by implementation accident:

- **OR-set `remove`, tombstones, causal-stability GC.** Rejected for M3 (§4.1),
  not merely postponed: adding `remove` without the GC that `DESIGN.md` §4.2
  and §7 require would create an unbounded structure. Revisit only when a
  milestone needs a claim genuinely retracted from the set rather than
  recorded as stood-down.
- **Per-node `priority`.** No configuration knob (§6). Add one when a scenario
  produces real priorities.
- **LWW telemetry register and sensor-track OR-set** (`DESIGN.md` §4.2). Not
  in M3's milestone text; no milestone claims them yet.
- **Task assignment from outside.** Tasks are numbered `0, 1, 2, …` by each
  node's own claim count (§6). A real mission would receive assignments; Phase
  1's `TaskId` is deliberately abstract (`DESIGN.md` §9, "Görev = soyut bir
  `TaskId`").

---

## 12. Trace format

*Normative — M3.* `docs/spec.md` §7's canonicality rules are unchanged. The
`ENTRY` envelope rendering gains the body, since M3 is the first milestone in
which the body carries meaning and a demo cannot be read without it:

```
kind=ENTRY origin=NNN seq=SSSSSSSSSSSS body=CLAIM task=TTTTTTTTTTTT prio=PPP
kind=ENTRY origin=NNN seq=SSSSSSSSSSSS body=WITHDRAW task=TTTTTTTTTTTT
```

Fixed field order, zero-padded integers, no floats — the existing rules. **This
changes every trace digest**, which `docs/spec.md` §6.1 already anticipates
("changing R1–R4, the roster order, or the number of RNG draws per effect will
change every trace ... this is intended"). No test pins a literal digest; every
digest assertion in `swarm-sim/tests/determinism.rs` compares runs against each
other, so all of them remain meaningful.

---

## 13. Changelog

| Milestone | Change |
|---|---|
| M3 | `Body::Withdraw` (tag `0x01`); `logical_clock` derived from `deps` as `Σ (seq + 1)`; `state` module with `Map<TaskId, ORSet<Claim>>` and the `min by (priority, lc, node, seq)` winner rule; grow-only claim set with withdrawal as a log record rather than a set removal; tick-phase-only authoring with claim → withdraw → advertise ordering; `State` gains `claims`; I3 strengthened to cover derived CRDT state; third golden vector; entry bodies rendered in the trace. |
