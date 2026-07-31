# swarm-core — Technical Specification

> `DESIGN.md` (Turkish) is the project's source of truth for *what* and *why*.
> This document is the normative specification for *how*: the exact rules the code
> implements. Per `DESIGN.md` §11.6, a decision is written here **before** it enters
> code. Per §11.5, a change to the wire format updates the golden vector test and
> states the reason in the commit message.
>
> **Status: M0.** Only the sections marked *Normative — M0* are binding today.
> Sections marked *Deferred* record questions deliberately left open.

---

## 1. Scope

Phase 1, milestone M0: a deterministic network simulator, written **before** any
protocol exists.

The ordering is not stylistic. If the simulator is written after the protocol, the
protocol will have been written against sockets and wall-clock sleeps and can no
longer be made deterministic (`DESIGN.md` §M0). That is irreversible, so it is a
day-zero decision.

M0 contains no protocol. Node behaviour is a placeholder: count what arrives, echo
it back. The only thing under test is the channel.

**M0 acceptance:** the same seed produces a byte-identical trace across runs;
different seeds produce different traces.

---

## 2. Repository layout

```
Cargo.toml            workspace
rust-toolchain.toml   pinned toolchain — part of the reproducibility claim
clippy.toml           mechanical enforcement of §11.1
DESIGN.md             source of truth (Turkish)
docs/spec.md          this file
crates/swarm-core/    no_std, no I/O, zero dependencies
crates/swarm-sim/     std; the simulator that drives swarm-core
```

`DESIGN.md` §5 draws the crates as a flat tree. They live under `crates/` here only
because the workspace root directory is itself named `swarm-core`, and a nested
`swarm-core/swarm-core/` path is needlessly confusing. The module names *inside*
`swarm-core` (`wire`, `causal`, `log`, `state`, `policy`, `fault`) follow §5 exactly
and are created as each milestone needs them — none exist at M0.

`swarm-verify` and `swarm-net` do not exist yet (M6 and Phase 2 respectively).

---

## 3. The sans-I/O boundary

*Normative — M0.*

`swarm-core` is a pure state machine. Its single entry point is, verbatim from
`DESIGN.md` §5:

```rust
pub fn step(state: &State, ev: Event, now: LogicalTime) -> (State, Vec<Effect>)
```

Inside `swarm-core` the following are forbidden, without exception (`DESIGN.md`
§11.1):

- network access — `std::net`, sockets of any kind
- clocks — `std::time`, `Instant`, `SystemTime`. Time enters **only** as the `now`
  parameter
- randomness — `rand::thread_rng` or any implicitly-seeded source. Randomness never
  enters `swarm-core` at all in M0; when it eventually must, it enters as a parameter
- async runtimes, threads, allocation of I/O resources

This is enforced three ways rather than by discipline:

1. `#![no_std]` — `std::net`, `std::time`, and `std::collections::HashMap` are not
   reachable from the crate.
2. `clippy.toml` `disallowed-types` / `disallowed-methods` — covers `swarm-sim` and
   every crate Phase 2 adds, where `std` *is* available.
3. A cross-compile to `thumbv7em-none-eabihf` in the verification step, so `no_std`
   is proven rather than asserted.

### 3.1 Why `no_std` at M0 rather than at Phase 2

Beyond `DESIGN.md` §5 fidelity, it buys determinism directly. `std::collections::HashMap`
uses `RandomState`, which is seeded per process; iteration order therefore differs
between two runs of the same binary with the same seed. That is precisely the class
of bug M0's byte-identical criterion exists to catch. Under `no_std`, `BTreeMap` is
the only available map, and its iteration order is a deterministic function of its
contents.

### 3.1.1 `NodeId` deliberately does not derive `Hash`

*Normative — M0.*

`NodeId` derives `Ord` but **not** `Hash`. Do not add it.

This was found by experiment rather than by design: an attempt to reproduce the
nondeterminism bug on purpose — by iterating the roster through a `HashSet` instead
of in `NodeId` order — did not compile. `HashSet<NodeId>` requires `NodeId: Hash`,
so the type system refused the bug before Clippy or any test was involved.

That makes it a real defence layer rather than an accident, and it is the cheapest
of the three: it costs nothing and it fails at compile time. Adding `Hash` would
silently remove it. The temptation will arrive at M2, when someone reaches for a
`HashMap` while building the version vector; the answer there is `BTreeMap`.

(For the record, the experiment was completed by adding `Hash` temporarily. Three
determinism tests then failed, and two runs of the same binary at the same seed
produced 916 differing trace lines. The guard tests in §"Tests" continued to pass,
correctly: only the determinism claim had broken.)

### 3.2 Why the pure signature

`step` takes `&State` and returns a new `State` rather than taking `&mut self`. This
costs a clone per event. It is kept because it is the shape the Phase 4 endgame
needs: a folding scheme's step function is exactly `z_{i+1} = F(z_i, w_i)`, and
`DESIGN.md` §5 stakes the "Phase 3 comes for free" claim on `swarm-verify`'s replay
being the same function compiled to a different target.

**Known cost, accepted deliberately:** once the per-node log lives inside `State`,
cloning is O(n) per event and O(n²) over a run. At 5 nodes and ~10 messages/second
this is irrelevant. If it ever stops being irrelevant, the fix is an internal
`&mut` implementation with this pure signature retained as a wrapper — the public
contract does not change. Revisit no earlier than M5.

---

## 4. Model

*Normative — M0.*

| Concept | Definition |
|---|---|
| Node | A member of the roster, identified by `NodeId(u8)`. Roster is fixed at mission start (`DESIGN.md` §7, "roster churn") |
| Tick | One discrete simulation step. `LogicalTime(u64)`, starting at 1 |
| Logical time | The **only** notion of time in the system |

There is no wall clock anywhere, at any layer. `DESIGN.md` §7 forbids tie-breaking
on wall-clock time because GPS time can be spoofed, which would let an attacker win
claim races. M0 establishes the habit before there is anything to tie-break: the
simulator itself has no access to real time, so a wall-clock dependency cannot be
introduced accidentally later without deleting code that visibly exists to prevent it.

### 4.1 M0 node behaviour (placeholder, replaced at M1)

- On `Recv`: increment `recv_count`; if `hops < MAX_HOPS`, echo the payload back to
  the sender with `hops + 1`.
- On `Tick`: if `now % beacon_period == 0`, send a beacon to every other node in the
  roster, ascending by `NodeId`.

The hop limit bounds the number of messages in flight. The beacon keeps the channel
busy so that loss, delay and partition are actually exercised — a silent network is
trivially deterministic and would satisfy the acceptance criterion while proving
nothing.

---

## 5. Channel semantics

*Normative — M0.*

### 5.1 Delivery

Each destination has its own queue, ordered by `(due_tick, enqueue_seq)` where
`enqueue_seq` is a global monotonic counter assigned at enqueue time. Because
`enqueue_seq` is globally unique, **two messages can never compare equal**, so the
order is total and no tie-break rule is needed.

### 5.2 Delay

Drawn uniformly from `[delay_min, delay_max]` ticks, inclusive.

**`delay_min >= 1` is required.** An effect produced during tick N is never
deliverable during tick N. Without this, a send could cascade into a receive into a
send within a single tick, and the resulting order would depend on iteration
sequence rather than on stated rules.

### 5.3 Loss

Independent per message, expressed in **permille as an integer**. Floating point
does not appear anywhere in the model — not in probabilities, not in delays, not in
the trace. This removes the question of float reproducibility rather than answering
it.

### 5.4 Partition

A partition is an assignment of each node to a group id. Two nodes can exchange
messages if and only if they are in the same group.

**Reachability is evaluated at delivery time, not at send time.** A message already
in flight when a partition opens is dropped. This is the physically honest model —
the radio link goes down while the packet is in the air — and it is what makes the
M2 partition test meaningful: a message that departed before the split must not
magically arrive after it.

Partitions are driven by a script: `Vec<(tick, Partition)>`, applied at the top of
the named tick. M5's randomised partition churn will be a *seeded generator of this
same script*; the simulator engine does not change.

### 5.5 Queue bound

Every queue is bounded (`DESIGN.md` §7, "memory bound"; §4.1, "the causal buffer must
be bounded"). On overflow the **oldest** message for that destination is dropped and
counted.

M0 applies this to the simulator's network queues. The same rule will apply to the
causal buffer at M2. The intent is that no structure in this system is ever allowed
to grow without a stated bound.

---

## 6. The determinism contract

*Normative — M0. This is the load-bearing section.*

Determinism is not a property of the code being single-threaded; it is a property of
this ordering being fixed and total. Any change to it changes every trace produced
after the change.

```
for tick in 1..=ticks:

  1. now = LogicalTime(tick)

  2. apply the partition-schedule entry whose tick == now, if any

  3. DELIVER phase
       for dest in roster, ascending by NodeId:
         for msg in queue[dest] with due <= now, ascending by (due, enqueue_seq):
           if not reachable(msg.from, dest):
              record DropPartition; continue
           (state[dest], effects) = step(&state[dest], Recv{from, payload}, now)
           for e in effects: enqueue(e)

  4. TICK phase
       for node in roster, ascending by NodeId:
         (state[node], effects) = step(&state[node], Tick, now)
         for e in effects: enqueue(e)
```

```
enqueue(Send { to, payload }):

  r_loss  = rng.next_u32()      # always drawn
  r_delay = rng.next_u32()      # always drawn, always second

  if r_loss % 1000 < loss_permille:
      record DropLoss; return

  delay = delay_min + r_delay % (delay_max - delay_min + 1)
  due   = now + delay
  seq   = next_enqueue_seq()    # global, monotonic

  if queue[to].len() == queue_cap:
      evict the entry with the smallest (due, enqueue_seq)
      record DropOverflow

  insert into queue[to]
```

Four rules, each of which the implementation depends on:

**R1 — `delay_min >= 1`.** No within-tick cascades. (§5.2)

**R2 — RNG draws are unconditional and ordered.** Both values are drawn for every
effect, in the order `r_loss` then `r_delay`, even when the message is about to be
dropped for an unrelated reason. Drawing only for surviving messages would make the
RNG stream a function of partition state and queue occupancy, so an unrelated change
to the partition schedule would scramble every subsequent draw and make two traces
incomparable.

**R3 — Integer arithmetic only.** (§5.3)

**R4 — Iteration is by `NodeId`, ascending, everywhere.** Never by map iteration
order, never by insertion order.

### 6.1 Consequence

Changing R1–R4, the roster order, or the number of RNG draws per effect will change
every trace. This is intended and is why the rules live here rather than only in
code. A trace digest is therefore a fingerprint of *the model*, not only of the
seed — which is what makes the `trace_is_sensitive` test meaningful.

---

## 7. Trace format

*Normative — M0.*

The trace is the M0 deliverable and the ancestor of the replay capability described
in `DESIGN.md` §5.2. It is an ordered sequence of records with a canonical one-line
text encoding.

Canonicality rules:

- fixed field order per record type
- integers zero-padded to fixed width, so lexicographic order matches numeric order
- no floating point
- no pointers, addresses, or hash-map iteration
- no wall-clock timestamps
- no source paths or line numbers

Record types at M0: `TICK`, `SEND`, `ENQUEUE`, `DELIVER`, `DROP_LOSS`,
`DROP_PARTITION`, `DROP_OVERFLOW`, `PARTITION`, `FINAL`.

Two views of the same trace:

- `render() -> String` — the full text, so a failing equality assertion produces a
  readable diff rather than "two 32-byte arrays differ".
- `digest() -> [u8; 32]` — BLAKE3 over the rendered bytes; a one-line run
  fingerprint for comparing many runs cheaply.

---

## 8. Dependencies

`swarm-core`: **none.** A zero-dependency core is worth preserving as long as
possible; `blake3` and `ed25519-dalek` arrive at M1.

`swarm-sim`: `swarm-core`, `rand_chacha`, `blake3`.

**On the RNG.** `DESIGN.md` lists `rand`. The concrete generator is
`rand_chacha::ChaCha8Rng` rather than `rand::rngs::StdRng`, because `StdRng`'s
documentation explicitly disclaims value stability across releases: a routine
`cargo update` could silently change what seed 42 produces and break the M0 claim
without breaking the build. `ChaCha8Rng` carries a reproducibility guarantee. This
is a refinement of `DESIGN.md`'s dependency list, not a departure from it.

`turmoil` and `madsim` are excluded, per `DESIGN.md`: both are built on tokio and
would force async throughout.

---

## 9. Deferred

Recorded so they are not silently decided by implementation accident:

- **Per-link RNG streams.** M0 uses one global stream, so activity on one link
  perturbs draws on every other. Traces are still deterministic, but they are
  fragile: adding a message anywhere shifts everything after it. If this becomes
  painful when debugging M2–M5, switch to a per-link stream derived from
  `seed ⊕ H(src, dst)`. Not needed yet.
- **Message duplication.** Not modelled at M0. Anti-entropy (M2) produces duplicates
  naturally, which is the more realistic source; revisit only if that proves
  insufficient.
- **Byzantine transport.** M4's cheating node lies at the *protocol* layer, not the
  channel layer. The simulator stays honest: it drops and delays, it does not forge.
- **Roster changes mid-run.** Out of scope for all of Phase 1 (`DESIGN.md` §7).

---

## 10. Changelog

| Milestone | Change |
|---|---|
| M0 | Initial specification: sans-I/O boundary, channel semantics, determinism contract, trace format. |
