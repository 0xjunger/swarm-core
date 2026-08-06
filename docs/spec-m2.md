# swarm-core — M2 Normative Specification

> `DESIGN.md` (Turkish) is the project's source of truth. `docs/spec.md` (M0) and
> `docs/spec-m1.md` (M1) remain binding unchanged; M2 does not touch the channel
> semantics, the determinism contract, the trace format's canonical rules, or the
> wire format's byte layout. This file is the normative specification for **M2**,
> written before the code per `DESIGN.md` §11.6.
>
> **Status: M2.** Sections marked *Normative — M2* are binding.

---

## 1. Scope

Phase 1, milestone M2: causal delivery and anti-entropy, on **three nodes**
(`DESIGN.md` §9, M2 verbatim): "Version vector devreye giriyor. Mesajlar
bağımlılıkları teslim edilmeden uygulanmıyor, bekleme kuyruğunda tutuluyor.
Periyodik olarak node'lar VV'lerini değiş tokuş edip eksikleri tamamlıyor."

**M2 acceptance** (`DESIGN.md` §9, "Bitti sayılır"): 3 nodes `{A,B}` and `{C}`
are partitioned, run 100 ticks, merge, run 50 more ticks — all three end up
with **the same entry set**.

M1's placeholder is retired here: `docs/spec-m1.md` already names the fate of
`Payload` ("becomes `Entry` at M2, when nodes begin broadcasting entries") and
of `deps` ("empty at M1 — there is nothing to depend on before there is a
network. The field exists now; M2 fills it"). This document is that filling.

---

## 2. `Envelope`: what a message carries

*Normative — M2.*

```rust
enum Envelope {
    Entry(wire::Entry),
    AntiEntropy(causal::VersionVector),
}
```

`Event`/`Effect` keep their M0/M1 shape exactly:

```rust
enum Event { Tick, Recv { from: NodeId, payload: Envelope } }
enum Effect { Send { to: NodeId, payload: Envelope } }
```

**Decision, stated explicitly.** `DESIGN.md` §4.1 is prose, not a wire
protocol; it does not mandate a distinct `Event` variant for anti-entropy, only
that anti-entropy *happen*. M1's `lib.rs` carried a doc comment reading
"`AntiEntropy` arrives at M2" beside the `Event` enum, which read as a
forward-declaration of a new variant; this spec supersedes that comment.
Growing `Event`/`Effect` was considered and rejected: dispatching on
`Envelope` inside the existing `Recv`/`Send` shape is one match arm cheaper,
touches fewer call sites (`step`'s two arms, `sim.rs`'s `emit`), and needs no
new RNG-draw or trace-record bookkeeping beyond what a payload-type change
already requires. The doc comment is corrected as part of this milestone.

`Envelope` cannot be `Copy` (`Entry` owns a `Signature`; `VersionVector` owns a
`BTreeMap`). This is a breaking, deliberate change: `Payload` was `Copy`,
`Envelope` is `Clone` only. `Event`/`Effect` lose `Copy`, keep `Clone`.
Consequence traced through `swarm-sim`: `net::Msg` is no longer `Copy`;
`sim::emit` takes `Vec<Effect>` by value instead of `&[Effect]` and matches by
value instead of dereferencing.

---

## 3. `deps`: population rule

*Normative — M2.*

An entry's `deps` is a **snapshot of the author's local causal version vector
at the moment of authorship, self-inclusive**:

```
deps = state.causal_vv.clone()          // taken BEFORE Log::append
Log::append(body, deps)                  // signs and links
state.causal_vv.bump(me, new_entry.seq)  // taken AFTER append
```

**Decision, stated explicitly — this is an interpretive reading of `DESIGN.md`
§4.1, confirmed with the project owner before implementation.** §4.1's prose
gives delivery as two conditions: "`deps ≤ yerel_VV`" and, separately, "aynı
node'dan `seq-1` teslim edilmiş." This spec folds them into one. Because
`causal_vv` is bumped for the author's own `seq` immediately after every local
append, any entry `(X, s)` with `s > 0` has a `deps` that already contains
`(X, s-1)` — the predecessor. Checking `deps ≤ local_vv` at a receiver
therefore *cannot* pass unless the receiver has already applied `(X, s-1)`,
which is exactly same-origin FIFO. One predicate does both jobs. This is the
standard construction for vector-clock-based causal broadcast (each node's own
component in its own vector clock already encodes its own send order); it is
not a weakening of §4.1, it is one way of implementing both of its clauses
with one comparison instead of two.

The first entry from any author (`seq = 0`) has no self-dependency: at the
moment of its creation `causal_vv` does not yet contain that author, so `deps`
omits it, which is correct — there is no predecessor to depend on, exactly
matching `prev = Hash::ZERO` for the same entry.

`prev` and `deps` remain functionally distinct despite both encoding a notion
of "what came before": `prev` is a tamper-evident hash link (identity,
`docs/spec-m1.md` §4.3), `deps` is a counting-only causal gate (delivery
order). They cooperate rather than duplicate — `prev` cannot be checked until
`deps` is satisfied and the predecessor is provably already stored, and this
spec's self-inclusive `deps` guarantees that ordering.

---

## 4. Causal delivery

*Normative — M2.*

On `Event::Recv { from, payload: Envelope::Entry(entry) }`:

1. **Already known.** If `state.causal_vv.highest(entry.node)` is `Some(k)`
   with `k >= entry.seq`, the entry is a duplicate (already applied, or the
   author re-sent it, e.g. via anti-entropy fill after the receiver already
   caught up through some other path). Dropped silently. Not an error: honest
   duplication is expected traffic, not tampering (`docs/spec.md` §9,
   "Byzantine transport... the simulator stays honest").
2. **Deps satisfied.** If `entry.deps.le(&state.causal_vv)`, the entry is
   verified — `log::verify_next(&state.roster, entry.seq, expected_prev,
   &entry)`, where `expected_prev` is the chain hash of the author's last
   entry already held (in `state.origins[&entry.node]`, or `Hash::ZERO` if
   this is the author's first entry as seen by this node) — and, on success,
   applied: pushed into `state.origins`, `state.causal_vv.bump(entry.node,
   entry.seq)`. A verification failure is dropped silently (defensive only;
   never triggered by the honest M2 simulator, matching `docs/spec.md` §9's
   scope boundary — a lying node is M4's problem).
3. **Deps unsatisfied.** The entry is inserted into the bounded causal buffer
   (§5), keyed `(entry.node, entry.seq)`, unverified.

After every successful apply (step 2, including entries pulled out of the
buffer below), the buffer is **drained to a fixed point**: rescan the buffer
ascending by `(origin, seq)` for any entry whose `deps.le(&causal_vv)` now
holds; if found, remove it, verify and apply it exactly as in step 2, then
restart the scan (because the just-applied entry may have unblocked others).
Repeat until one full pass finds nothing more to apply. This is what turns
"partition heals, one anti-entropy fill arrives" into "every causally-ordered
entry that fill unblocked gets applied in the same `step` call," rather than
requiring one `step` per buffered entry.

**Security-relevant rule, stated even though M2's simulator cannot violate
it:** `causal_vv` only ever advances by *locally verifying and applying* an
entry — never by copying or merging a peer's self-reported version vector.
This is what keeps I2 true even against a peer that lies about what it has
seen; `Envelope::AntiEntropy`'s vector is read-only input to a gap
computation (§6), never assigned into `causal_vv` directly.

`Envelope::AntiEntropy` never applies anything by itself; see §6.

---

## 5. The causal buffer

*Normative — M2.*

```rust
struct BufferedEntry { inserted_at: u64, entry: wire::Entry }
// keyed in State by (NodeId, u64) = (origin, seq)
```

`BTreeMap<(NodeId, u64), BufferedEntry>`, bounded by `buffer_cap` (stated at
`State` construction, per `DESIGN.md` §7's "every structure ... sınırlı ve
sınırı ispatlanabilir olmalı").

**Insertion.** If the key is already present, the insertion is a no-op (the
existing `inserted_at` is kept — under the honest M2 transport a re-arriving
entry for the same `(origin, seq)` is byte-identical, so which copy is kept is
immaterial; this stays true independent of that assumption, since only the
verified copy is ever applied). If the key is new and the buffer is at
`buffer_cap`, the entry with the smallest `(inserted_at, origin, seq)` is
evicted first (`DESIGN.md` §4.1: "dolunca en eskiyi düş ve anti-entropy'ye
güven"). "Oldest" is defined by `inserted_at` (the `LogicalTime` tick at
which this node first saw the entry), ties broken by `(origin, seq)` — both
already available as `step` arguments/entry fields, so eviction needs no new
source of ordering and stays within R4's spirit (deterministic, no RNG, no
wall clock).

**Recovery.** A dropped-for-overflow entry is not lost forever: the next
anti-entropy round (§6) re-offers it, because the evicting node's own
`causal_vv` for that origin has not advanced past the eviction, so the gap is
still visible to whichever peer it next syncs with.

**Bound.** Default `buffer_cap = 32` (`swarm-sim`'s `SimConfig`) — half of
the default `log_cap = 1000`'s conceptual per-run working set, same order of
magnitude as the existing network `queue_cap`. Not derived from a formal
budget; revisit if a scenario needs more (`DESIGN.md` §7 requires *a* stated
bound, not a maximal one).

---

## 6. Anti-entropy

*Normative — M2.*

**Trigger.** Every node has its own `anti_entropy_period`. On `Event::Tick`,
if `now.0 % anti_entropy_period == 0` (period `0` disables it, matching
`entry_period`'s existing convention from M0/M1's `beacon_period`), the node
broadcasts `Envelope::AntiEntropy(state.causal_vv.clone())` to every peer,
ascending by `NodeId` (R4).

**Reply — advertise, then immediate push.** On `Event::Recv { from, payload:
Envelope::AntiEntropy(their_vv) }`, the receiver computes, for each origin
ascending by `NodeId` (R4), the gap between what `their_vv` claims and what
the receiver itself holds (`state.causal_vv`), and returns one
`Effect::Send { to: from, payload: Envelope::Entry(e) }` per missing entry,
ascending by `seq` within each origin. No new envelope kind, no explicit
"request" step, no batching envelope.

**Decision, stated explicitly — confirmed with the project owner.** A
request/response/batch protocol (advertise → explicit "send me these" →
bulk reply) was considered and rejected for M2: it needs a third envelope
shape and an extra round trip to express what a single push-on-receipt
already achieves, since both directions of a gap are closed as soon as each
side's own periodic advertisement reaches the other. `DESIGN.md` §4.1's
"periyodik VV değişimi + fark tamamlama" is satisfied by this shape: the
*exchange* is periodic (both sides advertise on their own schedule), and the
*completion* is immediate once a gap is visible to either side.

**Ordering within `Event::Tick`.** Per-tick, a node's own entry-creation (if
`entry_period` fires) happens **before** its anti-entropy advertisement (if
`anti_entropy_period` fires also falls on this tick) — stated as normative,
alongside R1–R4, so the two firing together on the same tick is a fixed,
reproducible order rather than incidental code-order.

**Fill-reply burst size.** Not capped per round. A long partition can produce
a reply containing many entries in one anti-entropy round; this flows through
the existing bounded, drop-oldest network queue (`docs/spec.md` §5.5,
already tested) rather than a new mechanism. A fill entry evicted from the
network queue is simply re-offered on the sender's *next* anti-entropy
period, by the same logic as causal-buffer recovery above. No cap is added
for M2; the queue bound already provides the memory-boundedness `DESIGN.md`
§7 requires, and the acceptance criterion (§1) does not need one.

---

## 7. `State`'s shape at M2

*Normative — M2.*

```rust
pub struct State {
    me: NodeId,
    roster: wire::Roster,
    members: Vec<NodeId>,               // roster members, me excluded, ascending
    entry_period: u64,                  // replaces M0/M1's beacon_period
    anti_entropy_period: u64,
    recv_count: u64,
    sent_count: u64,
    log: log::Log,                                        // this node's own chain
    origins: alloc::collections::BTreeMap<NodeId, alloc::vec::Vec<wire::VerifiedEntry>>,
    causal_vv: causal::VersionVector,   // self-inclusive, §3
    buffer: alloc::collections::BTreeMap<(NodeId, u64), BufferedEntry>,
    buffer_cap: usize,
}
```

```rust
pub fn new(
    me: NodeId,
    roster: wire::Roster,
    key: ed25519_dalek::SigningKey,
    log_cap: usize,
    buffer_cap: usize,
    entry_period: u64,
    anti_entropy_period: u64,
) -> Self
```

Panics if `roster.key(me).is_none()` — same defense as M0/M1's "node absent
from its own roster," re-expressed against `wire::Roster` instead of a bare
`&[NodeId]` slice, for the same reason: silently running as a non-member of
one's own mission is a configuration error that would otherwise stay
invisible.

`origins[N]` has no cap of its own. It is bounded transitively: `N`'s own
`Log::append` refuses once `N`'s `log_cap` is reached (`docs/spec-m1.md` §6),
so no peer can ever hold more than `log_cap` entries authored by `N`. This
assumes a roster-uniform `log_cap`, true for `swarm-sim`'s homogeneous
`SimConfig`; stated here rather than enforced in code, since enforcing a
second, redundant cap would add a failure mode with no scenario that reaches
it at Phase 1 scale.

---

## 8. Bandwidth budget

*Normative — M2, per `DESIGN.md` §7's "baştan hesapla" requirement.*

From the frozen encoding (`docs/spec-m1.md` §3, unchanged by M2):

```
Entry           = 14 (tag) + 32 (mission) + 4 (epoch) + 1 (node) + 8 (seq)
                + 32 (prev) + (2 + 9·D) (deps, D = populated dep count)
                + 10 (TaskClaim body) + 64 (sig)
                = 165 + 9·D bytes
VersionVector   = 2 + 9·N bytes   (N = roster size)
```

At the roster cap `N ≤ 20` (`DESIGN.md` §4.5): `AntiEntropy` ≤ 182 B,
`Entry` (worst case `D = N`) ≤ 345 B. A fill reply after a long partition
costs `(missing count) × Entry size`, self-limited by the network queue bound
(§6) rather than by an explicit per-round cap.

---

## 9. Invariants

*Normative — M2.* Updates `docs/spec-m1.md` §8's table for I2/I3 only; I1,
I4–I6 are unchanged.

| # | Invariant | Status at M2 |
|---|---|---|
| I1 | At most one signed entry per `(node, seq)` | Unchanged from M1: binding, tested in `swarm-core/tests/invariants.rs`. |
| **I2** | An entry is not applied before its `deps` are delivered | **Binding.** §4's delivery rule is the enforcement; tested directly in `swarm-core/tests/causal.rs` (buffering, cross-node deps) and as an explicit invariant test in `swarm-core/tests/invariants.rs`. |
| **I3** | Two nodes that have seen the same entry set derive the same state | **Binding.** Tested in `swarm-core/tests/invariants.rs` (same entries, different arrival order, identical resulting `causal_vv` and entry content) and exercised end-to-end by the M2 acceptance test (`swarm-sim/tests/m2_convergence.rs`). |
| I4 | Spendable rights across all partitions ≤ authorised total | Documented placeholder — activates at M5 (escrow). |
| I5 | No safety-critical effect without a valid certificate in the log | Documented placeholder — activates with the policy gate (M5). |
| I6 | Every effect is traceable to a signed entry chain | Documented placeholder — activates when `step` derives effects from entries generally (M2 partially begins this — entries now cause `Effect::Send`s directly — but the full traceability claim, e.g. for policy-gated effects, is M5+). |

---

## 10. Golden vector

*Normative — M2.* `docs/spec-m1.md`'s golden vector (an `Entry` with an
**empty** `VersionVector`) is unchanged — M1's encoding rules are frozen, and
M2 introduces no new field or format, only non-empty *content* in an
already-specified field. A second, independent golden vector is added
(`swarm-core/tests/golden_vector.rs`) pinning the encoding of an `Entry`
whose `deps` holds two populated entries, proving the non-empty case encodes
per `docs/spec-m1.md` §3.2's rule (`u16 BE` count, then `(node u8, seq u64
BE)` pairs ascending by `NodeId`) without needing to touch the M1 vector.

---

## 11. Dependencies

*Normative — M2.* `swarm-core` gains no new dependency (`causal.rs`'s new
methods and `lib.rs`'s `Envelope` use only what `blake3`/`ed25519-dalek`/
`alloc` already provide). `swarm-sim` gains `ed25519-dalek` as a real
(non-dev) dependency: the simulator now signs on behalf of each simulated
node to construct its `State`, which M0/M1 never needed.

---

## 12. Deferred

Recorded so they are not silently decided by implementation accident:

- **`VersionVector::decode()`.** Not added at M2. `Envelope` carries native
  in-memory `VersionVector`/`Entry` values through `swarm-sim` — nothing in
  M2 serializes then deserializes a VV over real bytes. `decode` gets a call
  site (and a test) only once `swarm-net` (Phase 2) does byte-level wire I/O.
- **`VersionVector::merge()`.** Not added at M2, and not merely deferred —
  actively wrong to add, per §4's security-relevant rule: `causal_vv` must
  never absorb a peer's self-reported vector. A `merge()` free function
  sitting unused in the crate would be an invitation to violate that rule by
  reaching for it later out of convenience.
- **Anti-entropy fill-reply cap.** No per-round cap on how many entries one
  `AntiEntropy` reply may push (§6). Revisit only if a scenario demonstrates
  the existing network-queue bound is insufficient.
- **`Event::AntiEntropy` / `Effect::SendAntiEntropy` as dedicated variants.**
  Considered and rejected (§2); `Envelope` dispatch is preferred. Revisit
  only if a future milestone needs anti-entropy traffic to be distinguishable
  from entry traffic at the `Event`/`Effect` level itself (e.g., different
  RNG/backpressure treatment) rather than at the `Envelope` level.

---

## 13. Changelog

| Milestone | Change |
|---|---|
| M2 | `Envelope` (Entry \| AntiEntropy) replaces `Payload`; self-inclusive `deps` population; causal delivery rule with fixed-point buffer drain; bounded causal buffer with drop-oldest eviction; advertise-then-push-reply anti-entropy; `State` gains `log`, `origins`, `causal_vv`, `buffer`; I2 and I3 promoted to binding. |
