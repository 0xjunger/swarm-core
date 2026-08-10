# swarm-core — Technical Specification

> `DESIGN.md` (Turkish) is the project's source of truth for *what* and *why*.
> This document is the normative specification for *how*: the exact rules the
> code implements. Per `DESIGN.md` §11.6, a decision is written here **before**
> it enters code; per `DESIGN.md` §11.5, a wire-format change updates the golden vectors
> (§8.5) and states the reason in the commit message.
>
> This is a single living document, organized by topic rather than by
> milestone. Earlier milestones did not each get a frozen file — a decision
> made at M2 and refined at M3 is described once, in its current form, in the
> section that owns the topic. §17 (Roadmap) tracks what is implemented and
> what is not; §18 (Changelog) is the per-milestone history for anyone who
> needs it. If you are looking for "what did M2 add," read the changelog; if
> you are looking for "how does causal delivery work today," read §9 — it will
> not send you chasing three other files to find out.

---

## 1. Status

**Exit gate:** `scripts/verify.sh` is the one command that decides whether
Phase 1's exit criteria are met. It runs the full workspace test suite, then
rebuilds `swarm-core` with the `mutant-i3` negative control (§15) and
requires that build to fail — a checker that cannot fail on a deliberately
broken build is not a checker — then cross-compiles `swarm-core` for
`thumbv7em-none-eabihf` (proving the `no_std` claim, §3.1) and runs
`cargo clippy --workspace -- -D warnings`. Green on `scripts/verify.sh`
means the criteria are met; nothing else in this document does.

**Implemented: M0, M1, M2, M3, M4, M5, M6, M7.** Sections below describe the
system as it exists today, not as it was at any past milestone. Where a rule
changed shape between milestones (e.g. `deps`, invariant I3), only the
current shape is normative; the change itself is recorded in §18/§19.

**Not yet implemented:** everything in Phase 2+ (`swarm-net`, signed `Spec`,
MMR-based log pruning, input attestation). §17 sketches some of this without
freezing decisions that are not yet made — do not treat §17 as binding.

---

## 2. Repository layout

```
Cargo.toml            workspace
rust-toolchain.toml   pinned toolchain — part of the reproducibility claim
clippy.toml           mechanical enforcement of §3's I/O ban
DESIGN.md             source of truth (Turkish)
docs/spec.md           this file
crates/swarm-core/    no_std, no I/O, minimal dependencies (§16)
crates/swarm-sim/     std; the simulator that drives swarm-core
crates/swarm-verify/  std; the offline verifier — no simulator dependency (§20)
```

`DESIGN.md` §5 draws the crates as a flat tree. They live under `crates/` here
only because the workspace root directory is itself named `swarm-core`, and a
nested `swarm-core/swarm-core/` path is needlessly confusing. The module names
*inside* `swarm-core` (`wire`, `causal`, `log`, `state`, `policy`, `fault`)
follow §5 and are created as each milestone needs them; `wire`, `causal`,
`log`, `state`, `fault`, and `policy` exist today (M1–M6).

`swarm-verify` exists as of M6 (`check_invariants`, an in-process oracle) and
gained its external, file-based surface at M7 (§20): `LogBundle`, `Spec`,
`Verdict`, and the `swarm-verify` binary. `swarm-net` (Phase 2) does not exist
yet.

**Two surfaces, one normative.** `swarm-verify` now carries two independent
checkers of I1–I4: `oracle::check_invariants` (`src/oracle.rs`), which reads
live `State` from inside a simulation run, and `verify::verify` (`src/verify.rs`,
§20.5), which judges a `LogBundle`/`Spec` pair with no access to the process
that produced them. **`verify` is normative** — it is the one that answers
this project's central claim, that a stranger holding only files reaches the
same verdict independently. The oracle is retained as `verify`'s differential
partner and as the host for the `mutant-i3` negative control, which `verify`
structurally cannot serve (§20.5); it is not a product surface and its
agreement with `verify` is evidence, not a substitute for `verify` itself.

---

## 3. The sans-I/O boundary

`swarm-core` is a pure state machine. Its single entry point is, verbatim from
`DESIGN.md` §5:

```rust
pub fn step(state: &State, ev: Event, now: LogicalTime) -> (State, Vec<Effect>)
```

Inside `swarm-core` the following are forbidden, without exception
(`DESIGN.md` §11.1):

- network access — `std::net`, sockets of any kind
- clocks — `std::time`, `Instant`, `SystemTime`. Time enters **only** as the
  `now` parameter
- randomness — `rand::thread_rng` or any implicitly-seeded source. Randomness
  never enters `swarm-core`; where a simulated node needs a key or an RNG
  stream, it is generated outside the crate and injected
- async runtimes, threads, allocation of I/O resources

Enforced three ways rather than by discipline:

1. `#![no_std]` — `std::net`, `std::time`, and `std::collections::HashMap` are
   not reachable from the crate.
2. `clippy.toml` `disallowed-types` / `disallowed-methods` — covers
   `swarm-sim` and every crate Phase 2 adds, where `std` *is* available.
3. A cross-compile to `thumbv7em-none-eabihf` in the verification step, so
   `no_std` is proven rather than asserted.

### 3.1 Why `no_std` from day one

Beyond `DESIGN.md` §5 fidelity, it buys determinism directly.
`std::collections::HashMap` uses `RandomState`, seeded per process, so
iteration order differs between two runs of the same binary with the same
seed — precisely the class of bug M0's byte-identical-trace criterion exists
to catch. Under `no_std`, `BTreeMap` is the only available map, and its
iteration order is a deterministic function of its contents.

### 3.2 `NodeId` deliberately does not derive `Hash`

`NodeId` derives `Ord` but **not** `Hash`. Do not add it.

This was found by experiment rather than by design: an attempt to reproduce
the nondeterminism bug on purpose — by iterating the roster through a
`HashSet` instead of in `NodeId` order — did not compile. `HashSet<NodeId>`
requires `NodeId: Hash`, so the type system refused the bug before Clippy or
any test was involved.

That makes it a real defence layer rather than an accident, and it is the
cheapest of the three: it costs nothing and it fails at compile time. Adding
`Hash` would silently remove it. The temptation arrives whenever someone
reaches for a `HashMap` while building keyed state; the answer is always
`BTreeMap`.

(For the record, the experiment was completed by adding `Hash` temporarily.
Three determinism tests then failed, and two runs of the same binary at the
same seed produced 916 differing trace lines. The invariant-guard tests
continued to pass, correctly: only the determinism claim had broken.)

### 3.3 Why the pure `step` signature

`step` takes `&State` and returns a new `State` rather than taking `&mut
self`. This costs a clone per event. It is kept because it is the shape the
Phase 4 endgame needs: a folding scheme's step function is exactly `z_{i+1} =
F(z_i, w_i)`, and `DESIGN.md` §5 stakes the "Phase 3 comes for free" claim on
`swarm-verify`'s replay being the same function compiled to a different
target.

**Known cost, accepted deliberately:** with the per-node log inside `State`,
cloning is O(n) per event and O(n²) over a run. At 5 nodes and ~10
messages/second this is irrelevant. If it ever stops being irrelevant, the fix
is an internal `&mut` implementation with this pure signature retained as a
wrapper — the public contract does not change. Revisit no earlier than M5.

---

## 4. Model

| Concept | Definition |
|---|---|
| Node | A member of the roster, identified by `NodeId(u8)`. Roster is fixed at mission start (`DESIGN.md` §7, "roster churn") |
| Tick | One discrete simulation step. `LogicalTime(u64)`, starting at 1 |
| Logical time | The **only** notion of time in the system |

There is no wall clock anywhere, at any layer. `DESIGN.md` §7 forbids
tie-breaking on wall-clock time because GPS time can be spoofed, which would
let an attacker win claim races (this is exactly why M3's winner rule uses a
derived logical clock, `DESIGN.md` §11.2). The simulator itself has no access to real
time, so a wall-clock dependency cannot be introduced accidentally later
without deleting code that visibly exists to prevent it.

### 4.1 History: M0's placeholder node behaviour

Before any protocol existed (M0), node behaviour was a placeholder that no
longer runs: on `Recv`, increment a counter and echo the payload back with a
hop count; on `Tick`, broadcast a beacon on a period. Its only purpose was to
exercise the channel — loss, delay, partition — before there was anything
real to send over it. Retired at M1 (`Entry` replaces the echoed token) and
M2 (`Envelope` replaces the beacon). No code from this behaviour remains;
recorded here only because the ordering decision it represents (channel
before protocol, §3) is still why the crates are laid out this way.

---

## 5. Channel semantics

*Implemented by `swarm-sim`, not `swarm-core` — the simulator's model of an
unreliable radio link.*

### 5.1 Delivery

Each destination has its own queue, ordered by `(due_tick, enqueue_seq)`
where `enqueue_seq` is a global monotonic counter assigned at enqueue time.
Because `enqueue_seq` is globally unique, **two messages can never compare
equal**, so the order is total and no tie-break rule is needed.

### 5.2 Delay

Drawn uniformly from `[delay_min, delay_max]` ticks, inclusive.

**`delay_min >= 1` is required.** An effect produced during tick N is never
deliverable during tick N. Without this, a send could cascade into a receive
into a send within a single tick, and the resulting order would depend on
iteration sequence rather than on stated rules.

### 5.3 Loss

Independent per message, expressed in **permille as an integer**. Floating
point does not appear anywhere in the model — not in probabilities, not in
delays, not in the trace. This removes the question of float reproducibility
rather than answering it.

### 5.4 Partition

A partition is an assignment of each node to a group id. Two nodes can
exchange messages if and only if they are in the same group.

**Reachability is evaluated at delivery time, not at send time.** A message
already in flight when a partition opens is dropped. This is the physically
honest model — the radio link goes down while the packet is in the air — and
it is what makes the M2 partition-heal test meaningful: a message that
departed before the split must not magically arrive after it.

Partitions are driven by a script: `Vec<(tick, Partition)>`, applied at the
top of the named tick. M5's randomised partition churn will be a *seeded
generator of this same script*; the simulator engine does not change.

### 5.5 Queue bound

Every queue is bounded (`DESIGN.md` §7, "memory bound"). On overflow the
**oldest** message for that destination is dropped and counted. This applies
to the simulator's network queues (M0) and, by the same rule, to the causal
buffer (§9.3). No structure in this system is allowed to grow without a
stated bound — the causal buffer, the claim CRDT (`DESIGN.md` §11.4), and every future
addition inherit this requirement.

---

## 6. The determinism contract

*This is the load-bearing section.* Determinism is not a property of the code
being single-threaded; it is a property of this ordering being fixed and
total. Any change to it changes every trace produced after the change.

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

**R2 — RNG draws are unconditional and ordered.** Both values are drawn for
every effect, in the order `r_loss` then `r_delay`, even when the message is
about to be dropped for an unrelated reason. Drawing only for surviving
messages would make the RNG stream a function of partition state and queue
occupancy, so an unrelated change to the partition schedule would scramble
every subsequent draw and make two traces incomparable.

**R3 — Integer arithmetic only.** (§5.3)

**R4 — Iteration is by `NodeId`, ascending, everywhere.** Never by map
iteration order, never by insertion order. Every ascending-by-origin,
ascending-by-task, ascending-by-peer rule elsewhere in this document is an
instance of R4, not a separate decision.

### 6.1 Consequence

Changing R1–R4, the roster order, or the number of RNG draws per effect
changes every trace. This is intended and is why the rules live here rather
than only in code. A trace digest is a fingerprint of *the model*, not only of
the seed — which is what the `trace_observes_the_model_not_just_the_seed`
test relies on. Adding a field to what a trace record renders (as M3 did to
`ENTRY`, §7.2) is exactly such a model change: it changes every digest, and no
test should pin a literal digest value for that reason — every digest
assertion in the test suite compares two runs against each other.

---

## 7. Trace format

The trace is the record of one run: an ordered sequence of records with a
canonical one-line text encoding, and the substrate the replay capability of
`DESIGN.md` §5.2 will eventually build on.

Canonicality rules:

- fixed field order per record type
- integers zero-padded to fixed width, so lexicographic order matches numeric
  order
- no floating point
- no pointers, addresses, or hash-map iteration
- no wall-clock timestamps
- no source paths or line numbers

### 7.1 Record types

`TICK`, `SEND`, `ENQUEUE`, `DELIVER`, `DROP_LOSS`, `DROP_PARTITION`,
`DROP_OVERFLOW`, `PARTITION`, `APPLY`, `BUFFER`, `DROP_CAUSAL_OVERFLOW`,
`EQUIVOCATION`, `FINAL`.

The first nine exist from M0. `APPLY` (an entry was applied to derived
state), `BUFFER` (an entry was held pending causal delivery), and
`DROP_CAUSAL_OVERFLOW` (a buffered entry was evicted, §9.3) were added at M2,
derived by diffing a node's `State` before and after each `step` call — `step`
itself stays pure and returns only `Effect`s (§3.3); this diffing is
simulator bookkeeping, not a change to the core's contract. `EQUIVOCATION`
(a node independently verified a proof of equivocation against another,
§11.3) was added at M4, derived the same way.

### 7.2 Envelope rendering

`SEND` and `DELIVER` records render the `Envelope` they carry:

```
kind=ENTRY origin=NNN seq=SSSSSSSSSSSS body=CLAIM task=TTTTTTTTTTTT prio=PPP
kind=ENTRY origin=NNN seq=SSSSSSSSSSSS body=WITHDRAW task=TTTTTTTTTTTT
kind=ANTI_ENTROPY vv=[NNN:SSSSSSSSSSSS,...]
```

The body was added at M3: before M3 the body carried no information a demo or
a debugging session needed to distinguish, so `kind=ENTRY origin=... seq=...`
was sufficient. It is not sufficient once a claim and a withdrawal are both
just "an entry" — a trace a human cannot read is a trace a human cannot debug.

### 7.3 Two views

- `render() -> String` — the full text, so a failing equality assertion
  produces a readable diff rather than "two 32-byte arrays differ".
- `digest() -> [u8; 32]` — BLAKE3 over the rendered bytes; a one-line run
  fingerprint for comparing many runs cheaply.

---

## 8. Wire format: `Entry`

**The published message, the log record, and the proof object are the same
struct** (`DESIGN.md` §3) — one signature over one canonical encoding serves
all three roles.

### 8.1 Fields

| Field | Type | Meaning |
|---|---|---|
| `mission_id` | `[u8; 32]` | Roster Merkle root in the full design; a **fixed constant** in Phase 1 (`DESIGN.md`, "Alanları bugünden aç, doldurmayı ertele"). Prevents cross-mission replay once real values arrive. |
| `epoch` | `u32` | Roster version. Fixed at `0` in Phase 1. |
| `node` | `NodeId` | The author. |
| `seq` | `u64` | This node's monotonic log index. Starts at **0**; each successor is exactly `+1`. |
| `prev` | `Hash` (32 bytes) | BLAKE3 of the predecessor's full canonical encoding; `[0u8; 32]` for the genesis entry. |
| `deps` | `VersionVector` | Causal dependencies: a self-inclusive snapshot of the author's causal vector at authorship time (§9.2). Empty only for an author's very first entry. |
| `body` | `Body` | The record's meaning (§8.3). |
| `sig` | Ed25519 `Signature` (64 bytes) | Over the canonical signing bytes (§8.2). |

Fields are opened early and filled later, deliberately: adding a field after
the fact would invalidate every signature produced so far and break every
test fixture (`DESIGN.md`, item 1). `mission_id`, `epoch`, and `deps` were all
present, in their final byte layout, before anything used them for real.

### 8.2 Canonical encoding

There is exactly one byte encoding of an `Entry`. It is written explicitly,
field by field — **never serde**, whose output may change with library
version, field order, or compiler settings (`DESIGN.md`, item 2). Integers are
**big-endian** and fixed-width, so lexicographic order matches numeric order,
mirroring the trace rules (§7).

**Signing bytes:**

```
b"SWARM_ENTRY_V1"                  (14 bytes, domain separation tag)
|| mission_id                      (32 bytes)
|| epoch                           (4 bytes, u32 BE)
|| node                            (1 byte, u8)
|| seq                             (8 bytes, u64 BE)
|| prev                            (32 bytes)
|| deps                            (VersionVector encoding, below)
|| body                            (Body encoding, below)
```

The leading tag is domain separation (`DESIGN.md` §7): a signature valid in
this context must not be reusable in any future context (certificate
signatures, cross-signings) without an explicit new tag.

**`VersionVector` encoding:**

```
count                              (2 bytes, u16 BE)
|| (node u8 || seq u64 BE) * count, ascending by NodeId
```

Empty vectors encode as the two zero bytes `0000`. Ascending-by-`NodeId` order
is rule R4 (§6) and comes for free from `BTreeMap` iteration — `HashMap` is
unreachable in this crate (§3.2).

**`Body` encoding:**

```
variant tag                        (1 byte)
|| variant fields
```

| Variant | Tag | Fields | Since |
|---|---|---|---|---|
| `TaskClaim` | `0x00` | `task` (8 bytes, u64 BE) `\|\|` `priority` (1 byte, u8) | M1 |
| `Withdraw` | `0x01` | `task` (8 bytes, u64 BE) | M3 |
| `Spend` | `0x02` | `amount` (8 bytes, u64 BE) | M5 |

`priority` was opened at M1 although nothing used it until M3, because M3's
winner rule needed it and a later addition would have changed the wire
format. `Withdraw` added a tag at M3; it did not touch `TaskClaim`'s existing
bytes — a new enum variant adds a tag, it does not rewrite an existing
encoding, which is why both golden vectors that predate it (§8.5) still pass
byte for byte.

**Full encoding:** `signing bytes || sig (64 bytes)`. This is what the hash
chain hashes (§8.3) and what the golden vectors pin (§8.5).

### 8.3 Signing and the hash chain

**Signing.** Ed25519 over the signing bytes. Keys are **injected**, never
generated inside `swarm-core`: randomness does not enter the crate at all
(§3).

**Sequence numbers.** `seq` starts at **0** for the genesis entry and
increases by exactly 1 per entry. The next `seq` is derived from the chain
length, so a `seq` can never be reused: crash monotonicity (`DESIGN.md` §4.3)
holds structurally, because a pure state machine has no persistent tail to
lose. The fsync / secure-element concern `DESIGN.md` raises applies to
persistent nodes and arrives with real I/O (Phase 2).

**Chain links.** Genesis: `prev = [0u8; 32]`. Successor: `prev =
BLAKE3(predecessor's full canonical encoding)` — including the predecessor's
signature, so tampering with a signature, not only a body, breaks every
following link.

**Verification.** `verify_chain(roster, entries)` checks, for each entry in
order, and fails at the first violation, reporting the offending index:

1. `node` is present in the roster (membership).
2. `node` equals the first entry's node (a chain belongs to exactly one node).
3. `mission_id` equals the roster's (cross-mission replay rejected).
4. `epoch` equals the roster's.
5. `seq` equals the expected value (0, then +1). This is invariant I1
   (§14): a duplicated `(node, seq)` can never pass.
6. `prev` equals the expected link.
7. The Ed25519 signature verifies (strict verification) against the roster
   key of `node`.

`verify_next(roster, index, expected_seq, expected_prev, entry)` is the
single-entry form of the same seven checks minus the single-author rule
(which only makes sense across a batch); it is what causal delivery (§9.3)
calls per entry, since a receiver is not verifying a contiguous batch but one
entry at a time as its dependencies clear.

**`Entry` vs `VerifiedEntry`.** `Entry` is untrusted bytes from the outside
world. `VerifiedEntry(Entry)` is what verification produces; its constructor
is crate-private, so any function that must only see verified entries
declares that in its signature, and forgetting to verify becomes a **compile
error** rather than a runtime bug (`DESIGN.md`, item 4). Every function that
folds an entry into derived state — the causal-delivery apply step, `Claims`'
folding function (`DESIGN.md` §11.3) — takes `VerifiedEntry`, never `Entry`. The one
exception is a node's own freshly authored entry: it is verified by
construction (this node just signed it, over its own chain head, with its own
key), so it is wrapped through the crate-private constructor rather than
round-tripped through the verifier — the public API still cannot fold an
unverified `Entry`; only the crate's own authoring path can skip the
redundant check.

### 8.4 The log and its bound

`Log` is the per-node hash chain: it appends, signs, and links entries. It is
bounded; the capacity is stated at construction.

**Overflow policy: fail loudly.** Appending to a full log is an error
(`LogError::Full`); the log neither grows silently nor evicts. Eviction is
only safe once the MMR exists (`DESIGN.md` §4.3 makes the MMR the proof path
precisely so old entries can be pruned without losing provability), and the
MMR is not part of Phase 1. Until then, dropping history would make
end-to-end verification impossible, so the bound is enforced by refusal.

At every call site that authors an entry (§10.4, `DESIGN.md` §11.6), a full log is a
silent no-op — graceful degradation, not a crash. Not exercised at the
default `log_cap` used by `swarm-sim`.

### 8.5 Golden vectors

`swarm-core/tests/golden_vector.rs` pins, in hex, the signing bytes, the full
encoding, and the signature of known `Entry` values under a known key. Any
change to the wire format breaks these tests — **that is the point**: the
format must never change silently (`DESIGN.md`, item 5). A deliberate change
updates the golden vectors and states the reason in the commit message.

Four vectors, added as each shape of `Entry` first existed:

1. **M1** — `TaskClaim`, empty `deps`. The base case: single-variant body, no
   causal dependencies.
2. **M2** — `TaskClaim`, non-empty `deps` (two populated components). Proves
   the `VersionVector` encoding holds once the field actually carries data,
   without touching the M1 vector.
3. **M3** — `Withdraw`. Proves tag `0x01` and its single-field body, and that
   it produces different signed bytes than a `TaskClaim` naming the same task
   (so one signature can never be read as attesting to both).
4. **M5** — `Spend { amount: 1 }`. Proves tag `0x02` and its single-field body
   (8 bytes, u64 BE for `amount`).

All four remain byte-identical today. If any of them moves, something has
silently altered an already-frozen encoding.

---

## 9. Causal delivery and anti-entropy

*Three or more nodes; a real, unreliable network (§5).*

### 9.1 `Envelope`: what a message carries

```rust
enum Envelope {
    Entry(wire::Entry),
    AntiEntropy(causal::VersionVector),
}
```

`Event`/`Effect` do not carry a separate variant for anti-entropy — dispatch
happens on `Envelope` inside the existing shape:

```rust
enum Event { Tick, Recv { from: NodeId, payload: Envelope } }
enum Effect { Send { to: NodeId, payload: Envelope } }
```

**Decision, stated explicitly.** `DESIGN.md` §4.1 is prose, not a wire
protocol; it requires that anti-entropy *happen*, not that it own a distinct
`Event` variant. Dispatching on `Envelope` is one match arm cheaper, touches
fewer call sites, and needs no new RNG-draw or trace-record bookkeeping beyond
what a payload-type change already requires.

`Envelope` is `Clone` but not `Copy` (`Entry` owns a `Signature`;
`VersionVector` owns a `BTreeMap`), so `Event`/`Effect` are `Clone`-only too.

### 9.2 `deps`: population rule

An entry's `deps` is a **snapshot of the author's local causal version vector
at the moment of authorship, self-inclusive**:

```
deps = state.causal_vv.clone()          // taken BEFORE Log::append
Log::append(body, deps)                  // signs and links
state.causal_vv.bump(me, new_entry.seq)  // taken AFTER append
```

**Decision, stated explicitly.** `DESIGN.md` §4.1 gives delivery as two
conditions: `deps ≤ local_vv`, and separately, same-origin FIFO (`seq-1`
already delivered). This spec folds them into one. Because `causal_vv` is
bumped for the author's own `seq` immediately after every local append, any
entry `(X, s)` with `s > 0` has a `deps` that already contains `(X, s-1)` —
the predecessor. Checking `deps ≤ local_vv` at a receiver therefore *cannot*
pass unless the receiver has already applied `(X, s-1)`, which is exactly
same-origin FIFO. One predicate does both jobs; this is the standard
construction for vector-clock-based causal broadcast, not a weakening of
`DESIGN.md` §4.1.

The first entry from any author (`seq = 0`) has no self-dependency: at the
moment of its creation `causal_vv` does not yet contain that author, so
`deps` omits it — correct, since there is no predecessor to depend on,
exactly matching `prev = Hash::ZERO` for the same entry.

`prev` and `deps` remain functionally distinct despite both encoding "what
came before": `prev` is a tamper-evident hash link (identity, §8.3), `deps` is
a counting-only causal gate (delivery order). They cooperate — `prev` cannot
be checked until `deps` is satisfied and the predecessor is provably already
stored, and self-inclusive `deps` guarantees that ordering.

### 9.3 Causal delivery

On `Event::Recv { from, payload: Envelope::Entry(entry) }`:

1. **Already known.** If `state.causal_vv.highest(entry.node)` is `Some(k)`
   with `k >= entry.seq`, the entry is a duplicate — already applied, or the
   author re-sent it (e.g. via anti-entropy fill after the receiver already
   caught up some other way). Dropped silently. Not an error: honest
   duplication is expected traffic, not tampering.
2. **Deps satisfied.** If `entry.deps.le(&state.causal_vv)`, the entry is
   verified (§8.3's `verify_next`, against the chain hash of the author's
   last entry already held, or `Hash::ZERO` if this is the first entry from
   that author as seen by this node) and, on success, applied: pushed into
   `state.origins`, folded into `state.claims` (`DESIGN.md` §11.3), and
   `state.causal_vv.bump(entry.node, entry.seq)`. A verification failure is
   dropped silently — defensive only; never triggered by an honest
   transport, since the simulator drops and delays but does not forge
   (a lying node is M4's problem).
3. **Deps unsatisfied.** The entry is inserted into the bounded causal buffer
   (§9.4), keyed `(entry.node, entry.seq)`, unverified.

After every successful apply — including entries pulled out of the buffer —
the buffer is **drained to a fixed point**: rescan ascending by `(origin,
seq)` for any entry whose `deps.le(&causal_vv)` now holds; if found, remove
it, verify and apply it exactly as in step 2, then restart the scan (the
just-applied entry may have unblocked others). Repeat until one full pass
finds nothing more to apply. This is what turns "partition heals, one
anti-entropy fill arrives" into "every causally-ordered entry that fill
unblocked gets applied in the same `step` call," rather than requiring one
`step` per buffered entry.

**Security-relevant rule, stated even though the honest simulator cannot
violate it:** `causal_vv` only ever advances by *locally verifying and
applying* an entry — never by copying or merging a peer's self-reported
version vector. This is what keeps I2 true even against a peer that lies
about what it has seen; `Envelope::AntiEntropy`'s vector is read-only input to
a gap computation (§9.5), never assigned into `causal_vv` directly. This is
why `VersionVector::merge()` does not exist (§15).

### 9.4 The causal buffer

```rust
struct BufferedEntry { inserted_at: u64, entry: wire::Entry }
// keyed in State by (NodeId, u64) = (origin, seq)
```

`BTreeMap<(NodeId, u64), BufferedEntry>`, bounded by `buffer_cap` (stated at
`State` construction, per §5.5's "every structure has a stated bound").

**Insertion.** A key already present is a no-op (the existing `inserted_at`
is kept). A new key when the buffer is at `buffer_cap` evicts the entry with
the smallest `(inserted_at, origin, seq)` first. "Oldest" is defined by
`inserted_at` (the `LogicalTime` tick at which this node first saw the
entry), ties broken by `(origin, seq)` — both already available as `step`
arguments/entry fields, so eviction needs no new source of ordering and stays
within R4's spirit: deterministic, no RNG, no wall clock.

**Recovery.** A dropped-for-overflow entry is not lost forever: the next
anti-entropy round (§9.5) re-offers it, because the evicting node's own
`causal_vv` for that origin has not advanced past the eviction, so the gap is
still visible to whichever peer it next syncs with.

**Bound.** Default `buffer_cap = 32` (`swarm-sim`'s `SimConfig`) — half of the
default `log_cap = 1000`'s conceptual per-run working set, same order of
magnitude as the network `queue_cap`. Not derived from a formal budget;
revisit if a scenario needs more.

### 9.5 Anti-entropy

**Trigger.** Every node has its own `anti_entropy_period`. On `Event::Tick`,
if `now.0 % anti_entropy_period == 0` (period `0` disables it), the node
broadcasts `Envelope::AntiEntropy(state.causal_vv.clone())` to every peer,
ascending by `NodeId` (R4).

**Reply — advertise, then immediate push.** On receiving
`Envelope::AntiEntropy(their_vv)`, the receiver computes, for each origin
ascending by `NodeId`, the gap between what `their_vv` claims and what the
receiver itself holds, and returns one `Effect::Send` per missing entry,
ascending by `seq` within each origin. No new envelope kind, no explicit
"request" step, no batching envelope.

**The range re-sent, and why it overlaps by one.** For each origin, the reply
covers `their_vv.highest(origin).unwrap_or(0)` through the receiver's own
highest for that origin, inclusive — not `+ 1`. A version vector counts
entries; it does not identify them, so a peer's self-reported highest `seq`
for an origin says nothing about whether the entry the *receiver* holds at
that exact `seq` is the same entry. Re-sending the peer's own claimed head,
not just what lies strictly past it, is what lets equivocation (§11) surface
across a partition heal even when neither side's vector shows a numeric gap —
both sides already believe they are caught up on that origin, and only
comparing the actual entries at the shared `seq` reveals otherwise. The upper
end is clamped to the receiver's own highest: if the peer's claimed highest
for an origin already exceeds what the receiver holds, the receiver is the
one behind — exactly the position a victim of equivocation is left in, stuck
at the fork point while the peer has advanced past it — and the range must
stay empty rather than negative.

**Decision, stated explicitly.** A request/response/batch protocol
(advertise → explicit "send me these" → bulk reply) was considered and
rejected: it needs a third envelope shape and an extra round trip to express
what a single push-on-receipt already achieves, since both directions of a
gap close as soon as each side's own periodic advertisement reaches the
other. `DESIGN.md` §4.1's "periyodik VV değişimi + fark tamamlama" is
satisfied by this shape: the *exchange* is periodic, and the *completion* is
immediate once a gap is visible to either side.

**Ordering within `Event::Tick`.** Per-tick, if multiple things are due on
the same tick, the order is fixed (§10.4 restates this in full for M3):
entry creation, then withdrawals, then the anti-entropy advertisement.

**Fill-reply burst size.** Not capped per round. A long partition can produce
a reply containing many entries in one round; this flows through the
existing bounded, drop-oldest network queue (§5.5) rather than a new
mechanism. A fill entry evicted from the network queue is simply re-offered
on the sender's *next* anti-entropy period, by the same logic as causal-buffer
recovery (§9.4). No cap is added — the queue bound already provides the
memory-boundedness §5.5 requires.

### 9.6 `State`'s current shape

```rust
pub struct State {
    me: NodeId,
    roster: wire::Roster,
    members: Vec<NodeId>,               // roster members, me excluded, ascending
    entry_period: u64,
    anti_entropy_period: u64,
    recv_count: u64,
    sent_count: u64,
    log: log::Log,                                        // this node's own chain
    origins: BTreeMap<NodeId, Vec<wire::VerifiedEntry>>,   // received, per author
    causal_vv: causal::VersionVector,   // self-inclusive, §9.2
    buffer: BTreeMap<(NodeId, u64), BufferedEntry>,        // §9.4
    buffer_cap: usize,
    claims: state::Claims,                                 // §10.3
    poes: BTreeMap<NodeId, fault::Poe>,                     // §11.3
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

Panics if `roster.key(me).is_none()`: silently running as a non-member of
one's own mission is a configuration error that would otherwise stay
invisible. Panics if `buffer_cap == 0`, for the same class of reason (§5.5).

`origins[N]` and `claims` have no cap of their own. Both are bounded
transitively: `N`'s own `Log::append` refuses once `N`'s `log_cap` is reached
(§8.4), so no peer can ever hold more than `log_cap` entries authored by `N`,
and no more than `log_cap` claims/withdrawals derived from them. This assumes
a roster-uniform `log_cap`, true for `swarm-sim`'s homogeneous `SimConfig`;
stated here rather than enforced in code, since a second, redundant cap would
add a failure mode with no scenario that reaches it at Phase 1 scale.

`State::new`'s signature has not changed since M2: M3 added a field to the
struct but needed no new construction parameter, because the task number and
`priority` are both derived or fixed (§10.4).

---

## 10. The task-claim CRDT

`DESIGN.md` §4.2 names three data types and three difficulty levels for
replicated state. This section implements the third and hardest — task
claims, `Map<TaskId, ORSet<Claim>>` with the deterministic winner rule `min by
(priority, logical_clock, node_id)`. The LWW telemetry register and the
sensor-track OR-set are not implemented: no milestone's acceptance text names
them yet (§17).

Before M3, `TaskClaim`'s `task` field was populated with the author's own
`seq`, so no two nodes ever claimed the same task and there was no contest to
resolve. M3 gives claims real meaning.

### 10.1 `TaskId` and `Claim`

```rust
pub type TaskId = u64;

// Field order is deliberate: it IS the winner rule (§10.5).
pub struct Claim { priority: u8, lc: u64, node: NodeId, seq: u64 }
```

`TaskId` is deliberately abstract for all of Phase 1 (`DESIGN.md` §9: "Görev =
soyut bir `TaskId`"); nothing assigns real tasks from outside.

### 10.2 `logical_clock`: derived from `deps`, not carried

The winner rule needs three values. `priority` and `node_id` are already
fields of the entry. The third, `logical_clock`, is **derived** — no field, no
wire-format change:

```
lc(e) = Σ over (n, s) ∈ e.deps of (s + 1)
```

That is: the number of entries `e`'s author had applied at the moment it
authored `e`. `VersionVector` counts from `seq = 0`, so a component `(n, s)`
represents `s + 1` entries from `n`.

**Decision, stated explicitly.** The alternative was an explicit `lamport:
u64` field on `TaskClaim`. Rejected because it would change the frozen wire
format for a value the entry already determines — `priority` was opened at M1
*precisely* so this rule would need no format change (§8.2); deriving
`logical_clock` completes that intent.

**Why this is a valid logical clock.** The property the tie-break needs is
that a causally later claim never beats a causally earlier one:

> **Claim.** If `e1 → e2` (e1 happens-before e2), then `lc(e1) < lc(e2)`.
>
> **Proof.** `e2`'s author applied `e1` before authoring `e2`, so
> `deps(e2)[node(e1)] ≥ seq(e1)`. Causal delivery (§9.3) means that author
> could not have applied `e1` until `deps(e1) ≤` its own vector, so at
> authorship time its vector dominated `deps(e1)` componentwise. `deps(e2)` is
> a snapshot of that vector, hence `deps(e2) ≥ deps(e1)` componentwise. It is
> additionally strictly greater in the `node(e1)` component, because
> `deps(e1)` names at most `seq(e1) − 1` there (an entry's own `deps` is taken
> *before* its append, §9.2) while `deps(e2)` names at least `seq(e1)`.
> Summing `(s + 1)` over a componentwise-≥ vector that is strictly greater in
> one component, and may carry extra components (each contributing ≥ 1),
> gives `lc(e2) ≥ lc(e1) + 1`. ∎

Concurrent entries may share an `lc`; that is expected of any logical clock
and is why `node_id` follows it in the ordering.

**No wall clock, no randomness.** `lc` is a pure function of bytes already
inside the signed entry, so it cannot be spoofed independently of the
signature — `DESIGN.md` §7's objection to wall-clock tie-breaks does not
apply. A node *can* understate its own `deps` to lower its `lc`, but doing so
also weakens its causal position and, from M4 onward, is visible in its
chain; preventing it is not this milestone's problem.

**Honest limit.** `lc` counts *all* entries the author had seen, not only
task-related ones. Under a partition, a node in the larger group accumulates
entries faster and so carries a larger `lc` than an isolated node — the
isolated node tends to win contested tasks. This is inherent to Lamport-style
clocks (an explicit `L = max(L, L_recv) + 1` counter behaves identically) and
is deterministic and causally sound, but it is worth stating so it is not
mistaken for a bug when a demo shows it.

### 10.3 `Claims`: the OR-set

```rust
pub struct Claims {
    by_task:   BTreeMap<TaskId, BTreeSet<Claim>>,
    withdrawn: BTreeSet<(TaskId, NodeId)>,
}
```

**The OR-set's unique tag is `(node, seq)`** — the identity of the entry that
carries the claim. Two nodes claiming the same task with the same priority
and the same `lc` produce two distinct elements, never one merged element,
because the tag comes from the entry and needs no separate generation.

**Folding.** Exactly one function folds an entry into `Claims`, and it takes
a `VerifiedEntry`, never an `Entry` (§8.3):

- `TaskClaim { task, priority }` from entry `e` inserts
  `Claim { priority, lc: lc(e), node: e.node, seq: e.seq }` into
  `by_task[task]`.
- `Withdraw { task }` from entry `e` inserts `(task, e.node)` into
  `withdrawn`.

Both are set insertions, so folding is idempotent and commutative: the same
entry set yields the same `Claims` regardless of arrival order. That is
invariant I3 (§14) discharged structurally rather than by argument.

### 10.4 `Withdraw` does not remove the claim, and why

`Withdraw` does **not** remove the author's claim from `by_task`. The claim
set is grow-only.

**Decision, stated explicitly.** Making `Withdraw` an OR-set removal was
considered and rejected. It would require tombstone bookkeeping and, with it,
causal-stability-based garbage collection, which `DESIGN.md` §4.2 and §7 both
flag as mandatory before tombstones may exist at all ("tombstone GC'si için
causal stability kullan; yoksa state monoton büyür") — a decision not yet
made, and therefore not yet allowed into code. None of that is needed by the
acceptance criterion this satisfies, which asks for a record in the loser's
*log*, not a mutation of the claim set.

The consequence is a stronger property, not a weaker one: with a grow-only
set, the winner is a pure `min` over it, so §10.5's monotonicity holds and
convergence needs no argument beyond set-union commutativity. An OR-set whose
`remove` has no call site is not added.

The honest limit: a node cannot un-claim a task. Nothing in Phase 1 needs to.

**Bound.** `Claims` has no cap of its own; see §9.6.

### 10.5 The winner rule

```
winner(task) = min of by_task[task] by (priority, lc, node, seq)
             = None if the task has no claims
```

The first three keys are `DESIGN.md` §4.2's rule verbatim. **`seq` is
appended purely for totality**: without it two claims could compare equal
only if the same node claimed the same task twice with the same `lc`, which
§10.6's authoring rule never produces. It exists so the ordering is total by
construction rather than by assumption — the same reasoning §5.1 gives for
`enqueue_seq`.

`Claim`'s field order is chosen so that the derived `Ord` **is** this rule and
`BTreeSet::first()` **is** `winner`. There is no second place where the
ordering could drift out of sync with this spec.

**Losing is monotone.** Because `by_task[task]` only ever grows and the
winner is its minimum, the minimum can only decrease:

> Once a node is not the winner of a task, no future entry can make it the
> winner again.

Two consequences the implementation depends on: a `Withdraw` is never
regretted, so a node authors at most one per task, bounding log growth; and
two nodes that have seen the same entry set agree on the winner (I3, §14),
with neither later contradicted by an entry it has not yet seen becoming less
authoritative.

### 10.6 Authoring: what a node writes, and when

**All entry authorship happens in the `Event::Tick` arm, never in
`Event::Recv`.** Delivering an entry may emit anti-entropy fill replies
(§9.5) but never authors a new entry — a node that authored while draining
its causal buffer would interleave authorship with the fixed-point drain of
§9.3.

Fixed order within one tick:

1. **Claim.** If `entry_period` fires: author and broadcast
   `TaskClaim { task: k, priority: 1 }`, where `k` is the number of
   `TaskClaim` entries already in this node's own log. Every node therefore
   claims tasks `0, 1, 2, …` in order, so **every task is contested by every
   node** by default rather than by special configuration.
2. **Withdraw.** Then, in the same tick, for every task `t` this node has
   claimed, **ascending by `t`** (R4's spirit): if `winner(t).node != me` and
   this node's own log holds no `Withdraw { task: t }` yet, author and
   broadcast `Withdraw { task: t }`. By §10.5 this fires at most once per
   task. Emitting all pending withdrawals rather than one per tick keeps the
   rule stateless; the burst flows through the already-bounded network queue
   (§5.5).
3. **Advertise.** If `anti_entropy_period` fires: broadcast the version
   vector (§9.5).

`k` and "which tasks am I still owed a withdrawal for" are both **derived
from the node's own log** on each tick, never stored in a separate counter. A
redundant counter could drift out of step with the log after a refused
append; a derived value cannot. Cost is a scan of a `log_cap`-bounded vector
per tick, which `DESIGN.md` §9 declines to optimise at Phase 1 scale.

`priority` is fixed at `1` for autonomously authored claims. No per-node
priority knob exists: nothing in Phase 1 produces real priorities, and an
unused configuration field is exactly what `DESIGN.md` §11.4 forbids. The
`priority` term of the winner rule is exercised by hand-built claims in
`swarm-core/tests/claims.rs` instead.

---

## 11. Fault detection: proof of equivocation

*A deliberately faulty node; `DESIGN.md` §4.4.*

### 11.1 `Poe`: the proof is the two signatures

```rust
struct Poe { a: Entry, b: Entry }  // fault module
```

Two signed entries at the same `(node, seq)` with different content —
nothing else. `Poe::new(x, y)` returns `None` unless `x.node == y.node`,
`x.seq == y.seq`, and the two encode to different bytes (§8.2); two
deliveries of the identical entry are honest re-delivery (§9.3), not a proof
of anything.

**Canonical ordering.** The two entries are stored as `(a, b)` with
`a.encoded() <= b.encoded()` lexicographically, regardless of construction
order. This is what makes independent construction useful: two nodes that
each hold a different one of the pair first and receive the other later
build byte-identical `Poe`s, so a proof can be compared for equality or
deduplicated without re-deriving anything.

### 11.2 Verification: roster alone, no context

```rust
fn verify_poe(roster: &Roster, poe: &Poe) -> Result<(), PoeError>
```

Checks, in order: same author, same `seq`, distinct encodings, the accused
`node` is in `roster`, and both signatures verify under that node's roster
key (`Entry::signing_bytes`, §8.3). Nothing else is consulted — no log, no
peer, no simulator state, no agreement from any other node.

This is the property `DESIGN.md` §4.4 names directly: "kanıt kendi kendini
doğruladığı için suçlu node'u dışlamak konsensüs gerektirmez" (the proof
verifies itself, so excluding the guilty node needs no consensus). A third
party holding only the roster's public keys — never having run the
simulation, never having exchanged anything with either accuser — reaches
the identical verdict. `swarm-sim/tests/m4_equivocation.rs` exercises exactly
this: it rebuilds the roster from scratch and verifies a proof produced by
nodes it otherwise has no relationship to.

A tampered proof fails closed: flipping one bit of either signature turns
`Ok(())` into `Err(PoeError::BadSignature)`, and an entry claimed to be from
`node` but actually signed by a different key fails the same way — a peer
cannot frame an honest node by fabricating a "conflicting" entry, since
fabricating it requires the victim's own private key.

### 11.3 Detection: at delivery time, against everything held

```rust
fn held_at(state: &State, node: NodeId, seq: u64) -> Option<Entry>
fn detect_equivocation(state: &mut State, incoming: &Entry)
```

On every `Envelope::Entry` receipt, before the causal-delivery decision of
§9.3 runs, `detect_equivocation` looks up whatever this node already holds at
the incoming entry's `(node, seq)` — checking, in order, its own log (if
`node == me`), `origins` (already applied), and the causal buffer (§9.4,
not yet applied). All three must be checked: a conflicting second copy can
arrive while the first is still sitting unapplied in the buffer, and missing
that case would miss a real equivocation.

If something is held and it conflicts with the incoming entry, `Poe::new`
builds a proof and `verify_poe` checks it against the roster; on success it
is inserted into `state.poes`, keyed by the accused node. At most one proof
is kept per accused node — one is already sufficient to exclude that node,
and keeping more would grow `poes` without the bound every other structure
in this system is held to (§5.5). Once a node is proven faulty, further
conflicting entries from it are not re-checked.

`State::poes()` iterates the proofs a node currently holds, ascending by
accused `NodeId`; `State::is_proven_faulty(node)` is the yes/no form. Both
are read-only queries — accusing a node changes nothing about how its
*other*, non-conflicting entries are delivered or applied; `DESIGN.md` §4.4
is explicit that this is accountability, not exclusion enforced by the
protocol itself.

### 11.4 Why anti-entropy had to change

Detection depends entirely on two conflicting entries eventually reaching
the same node. §9.5's fill-reply range is the mechanism that makes this
happen across a partition heal, and it needed the overlap-by-one clamp
described there: without it, a receiver whose version vector already shows
it as "caught up" on the equivocator's genesis entry would never be re-sent
a copy to compare against its own, and the two forged genesis entries could
sit on either side of a healed partition forever, each side believing it
had nothing left to exchange.

### 11.5 Honest limits, restated from `DESIGN.md` §4.4

- **Post-hoc, not preventive.** A node eclipsed into never meeting the other
  partition never triggers detection — nothing here bounds how long an
  equivocation can go unproven, only that it will be proven once both
  conflicting copies reach one node.
- **Consistency, not truth.** The hash chain and signatures prove a node said
  two inconsistent things; they say nothing about whether either one was
  accurate. Sensor accuracy is a different problem, out of scope for this
  layer.

### 11.6 The simulator's faulty node

`SimConfig::equivocation: Option<Equivocation>` names one node and a set of
victim nodes. At the moment that node would broadcast its genesis entry
(`seq = 0`, the only entry `swarm-sim`'s scenarios forge — once a victim
holds it, the equivocator's later entries fail ordinary chain verification,
`BadPrevLink`, §8.3, at that victim, so no further forging is needed to keep
the two sides apart), the simulator substitutes a different, validly
re-signed body per victim rather than the one real entry every other peer
receives.

This is deliberately narrow: substitution only happens for effects produced
while authoring (`Event::Tick`), never for effects produced while relaying
(`Event::Recv`, e.g. an anti-entropy push reply) — a relay must always pass
through whatever is actually stored, honest or forged, or a victim could
never receive the genuine article from anyone, including the equivocator's
own later, honest replies about its own log. This keeps the channel itself
honest, per §15's "Byzantine transport" boundary: the simulator still only
drops and delays; the forging happens *as* the faulty node, at the protocol
layer, not *as* the network.

`build_roster` (used internally to construct every node's `State`) is public
for the same reason `verify_poe` needs only a roster: a test proving that a
third party can verify unilaterally has to be able to build that third
party's roster without also spinning up a simulation.

---

## 12. Escrow counter (`DESIGN.md` §M5)

The question "what if communication goes down entirely?" is answered by the
escrow counter, not by consensus. Each node is allocated a fixed budget at
mission start; within its own allocation it spends freely, without asking any
peer. Budget transfers (which would require a handshake) are not in M5's scope
and are deferred to Phase 2.

### 12.1 `Body::Spend`

`Body::Spend { amount: u64 }` records a node's expenditure. Tag `0x02`, encoding
length 9 bytes (tag + 8-byte big-endian amount). The body is a record — it does
not alter the `Claims` CRDT (§10), and the `Claims::observe` path is a no-op
when a `Spend` arrives.

### 12.2 `Escrow` structure

`Escrow { allocations: BTreeMap<NodeId, u64>, spent: BTreeMap<NodeId, u64> }`

- `allocations` is immutable once set (via `State::with_budgets`). It gives
  each node its spending ceiling.
- `spent` is cumulative: every applied `Spend` entry increments
  `spent[author]` by `amount`. The increment is `saturating_add`, so the
  counter cannot overflow.
- `remaining(node) = allocations[node] - spent[node]`, saturating at 0. A
  node absent from `allocations` has zero remaining.
- `can_spend(node, amount) = remaining(node) >= amount`.

### 12.3 Observation and authoring

Folding follows the same pattern as `Claims::observe` (§10.3): `Escrow::observe`
is called from `attempt_apply` (every received entry) and from `author` (self-
authored entries). Both paths feed only `VerifiedEntry`, so an unverified Spend
can never increase the counter.

On `Event::Tick`, after the existing task claim and withdrawal logic, the node
checks `escrow.can_spend(me, 1)`. If true, it authors `Body::Spend { amount: 1 }`
and broadcasts it. The spend rate is 1 unit per `entry_period` tick; when the
node's remaining budget reaches zero, authoring stops.

### 12.4 Invariant I4

> "tüm partisyonlardaki harcanabilir hakların toplamı ≤ yetkilendirilen toplam"

I4 holds **structurally**, not by consensus:

1. Each node `n` has a locally enforced per-entry cap: it cannot author
   `Spend { amount: x }` unless `remaining(n) >= x`.
2. The sum of per-node caps bounds the global sum: `Σ spent(n) ≤ Σ
   allocations(n)` for every `n`.
3. A partition cannot circumvent this, because the only node that can spend
   `n`'s budget is `n` itself — and `n` always carries its full history (its
   own chain includes every Spend it has authored). Even if no peer sees the
   spends, `n`'s own local cap stops it.

Budget *transfers* would require a two-round handshake and are not in M5's
scope; when they arrive, per-node caps will be the safety net that keeps I4
true while the handshake is re-proposed after partition heal.

### 12.5 Testing

- **Unit.** `swarm-core/tests/invariants.rs` — I4 via `step`: a node with
  budget 3 spends 3 and then stops; an observer tracking another node's Spend
  entries sees the correct remaining; total unique Spend across all origins
  never exceeds total allocation.
- **Integration.** `swarm-sim/tests/m5_escrow.rs` — 1000 seeds with random
  message loss, 200 seeds with partition + loss. At each seed, the union over
  every node's knowledge is checked: `Σ unique Spend amounts ≤ N ×
  budget_per_node`. The test includes a deliberate-bug case that fabricates
  Spending beyond budget and calls `swarm-verify::check_invariants` directly
  on the resulting state, asserting it reports an I4 violation — so the
  positive tests are not vacuously passing. The same fabricated-overspend
  scenario is also `swarm-verify/tests/i4_negative.rs`'s own regression test:
  `check_i4` used to reconstruct each node's budget from the state being
  checked rather than take it as a parameter, which made the comparison a
  tautology that could not fail. `check_invariants` now takes
  `budgets: &BTreeMap<NodeId, u64>` explicitly.

---

## 13. Bandwidth budget

*Per `DESIGN.md` §7's "baştan hesapla" requirement.*

From the frozen encoding (§8.2):

```
Entry (TaskClaim) = 14 (tag) + 32 (mission) + 4 (epoch) + 1 (node) + 8 (seq)
                   + 32 (prev) + (2 + 9·D) (deps, D = populated dep count)
                   + 10 (body: 1 tag + 8 task + 1 priority) + 64 (sig)
                   = 165 + 9·D bytes
Entry (Withdraw)   = same, but body is 9 B (1 tag + 8 task) instead of 10
                   = 164 + 9·D bytes
Entry (Spend)      = same, but body is 9 B (1 tag + 8 amount)
                   = 164 + 9·D bytes
VersionVector      = 2 + 9·N bytes   (N = roster size)
```

At the roster cap `N ≤ 20` (`DESIGN.md` §4.5): `AntiEntropy` ≤ 182 B, `Entry`
(worst case `D = N`) ≤ 345 B. A fill reply after a long partition costs
`(missing count) × Entry size`, self-limited by the network queue bound
(§5.5) rather than an explicit per-round cap.

M3 roughly doubled the steady-state entry rate: a node may now emit one claim
*and* one withdrawal per `entry_period` instead of one claim. M5 adds a Spend
entry per period while budget remains, so the peak authoring rate per period is
1 claim + 1 withdrawal + 1 Spend = 3 entries. The network queue (§5.5) and
per-node log cap (§8.4) still bound all three.

---

## 14. Trace and simulator internals

Covered by §7 (format) and §6 (the loop). Nothing in this section is separate
from those; listed here only as a pointer for anyone looking for "where is
the simulator specified" — the answer is §5–§7, not a separate document.

---

## 15. Invariants

Per `DESIGN.md` §11.7, invariants are written before the code that guards
them. This table reflects the current, cumulative status — not a per-milestone
snapshot.

| # | Invariant | Status |
|---|---|---|
| **I1** | At most one signed entry per `(node, seq)` | **Binding.** Enforced by construction (`seq` = chain length, §8.3) and by verification (§8.3 rule 5 rejects duplicates). Tested in `swarm-core/tests/invariants.rs`, and executable-checked in-process by the oracle, `swarm-verify::check_invariants`, across 5000 random seeds (`swarm-sim/tests/m6_property.rs`). **Also executable-checked externally, from bytes alone, by `swarm-verify::verify` (§20.5)**: every chain-verified entry in a `LogBundle`, grouped by `(author, seq)`, with a second, independent `verify_poe` re-check before any conflict is reported — `crates/swarm-verify/tests/fixtures.rs`'s `equivocation.bundle` (§20.7) is the fixture proof. |
| **I2** | An entry is not applied before its `deps` are delivered | **Binding.** §9.3's delivery rule is the enforcement; tested in `swarm-core/tests/causal.rs` (buffering, cross-node deps) and `swarm-core/tests/invariants.rs`. Executable-checked in-process by the oracle across 5000 random seeds — but the oracle's `check_i2` tests only the *final* `causal_vv`, which by the end of a run has grown to cover nearly everything: it catches "the dependency never arrived," not "this was applied before it arrived." **`swarm-verify::verify` (§20.5) checks the temporal property properly**: a causal fixed-point replay over the bundle's raw entries, independently reimplemented (`crates/swarm-verify/src/fold.rs`); an entry the replay cannot reach is direct evidence of the violation, not an inference from a monotone summary. The 5000-seed figure is real evidence for I1 and I4 above; for I2 it is evidence only that the oracle's *weaker* check passes, which is why it is not presented as equal-strength proof here — this is a calibration, not a weakness discovered late (the distinction was visible before M7's replay closed it; see §20.5, E7b). |
| **I3** | Two nodes that have seen the same entry set derive the same state | **Binding, and strengthened at M3.** "Derived state" now means `causal_vv`, the entry set, `claims`, **and `winner(t)` for every task `t`** — not just the version vector. Discharged structurally by §10.3 (set insertion is commutative and idempotent) and §10.5 (losing is monotone); tested in `swarm-core/tests/invariants.rs` and end to end by `swarm-sim/tests/m2_convergence.rs` and `swarm-sim/tests/m3_claim.rs`. Executable-checked in-process by the oracle across 5000 random seeds; the oracle's negative control is the `mutant-i3` cargo feature, which breaks `Claims::winner`'s tie-break to prefer the observing node's own claim — the same test run against that build reports a real I3 violation. **`swarm-verify::verify` (§20.5) checks I3 from a `LogBundle` alone**, with no access to any node's live `Claims` — it restates `winner(task)` itself, over its own independently-folded claims, comparing observer pairs whose applied `(author, seq)` key-sets coincide; `Undetermined` rather than `Satisfied` when fewer than two observers, or no two observers' key-sets, are comparable (silence is not evidence). Structurally independent of the oracle's fold in a way that is *demonstrated*, not just argued: `crates/swarm-sim/tests/m7_equivalence.rs::verify_does_not_inherit_the_mutant_i3_tie_break` asserts `verify` still reports I3 `Satisfied` over the tied-claim scenario `mutant_i3_detection` uses — an assertion that holds on both a clean build and a `mutant-i3` one, because `verify` never calls the function that feature changes. |
| I4 | Spendable rights across all partitions ≤ authorised total | **Binding.** Discharged structurally (§12.4): each node's spending is locally capped, so the global sum is bounded by the sum of per-node caps — no consensus required. Tested in `swarm-core/tests/invariants.rs` (unit), end to end by `swarm-sim/tests/m5_escrow.rs` (1000 seeds with random loss, plus a fabricated-overspend negative control that calls the oracle directly), and executable-checked in-process across 5000 random seeds by `swarm-verify::check_invariants`. **Also executable-checked externally by `swarm-verify::verify`**: `Spend` entries deduped by `(author, seq)` across every observer's replay-applied entries, summed per node against `spec.budgets`; `crates/swarm-verify/tests/verify.rs` includes the E3 independence proof (§20.3) — one bundle, opposite verdicts under a lowered vs. sufficient budget — and `overspend.bundle` (§20.7) is the fixture proof. |
| I5 | No safety-critical effect without a valid certificate in the log | **Binding, structural — not an executable check.** The `Action` trait does not tie `Cert` to `Class` at the type level — nothing stops a future type from implementing `Action` with `CLASS = SafetyCritical` and `Cert = ()`. What actually holds today is narrower and still real: no type in this crate implements `Action` with `CLASS = SafetyCritical` at all, so `commit` can never be called on one — proven by the `compile_fail` doctest on `policy::SafetyCriticalAction`. Neither the oracle nor `swarm-verify::verify` checks I5; there is nothing at runtime, and nothing in a `LogBundle`, to check (`Verdict::structural_note`, §20.4). |
| I6 | Every effect is traceable to a signed entry chain | **Binding, structural — not an executable check.** [`policy::author_and_commit`] is the single path through which any `Effect::Send` is created, and it always writes to the log first — a code-structure fact verifiable by reading `policy.rs`, not a property either the oracle or `swarm-verify::verify` runs a check for. |

I1 is what M4's proof of equivocation ultimately makes *accountable* rather
than merely enforced: verification (§8.3 rule 5) already refuses to apply a
second signed entry at a taken `(node, seq)` locally, but a node signing two
different entries and sending one to each side of a partition is a violation
no single receiver's local check can see by itself. §11's `Poe` is the
cross-node witness — proof that I1 was violated by a specific node,
verifiable by anyone holding the roster. Tested in
`swarm-sim/tests/m4_equivocation.rs`.

---

## 16. Deferred

Recorded so these are not silently decided by implementation accident. Each
is a real decision that was considered and consciously not made — not an
oversight.

- **`VersionVector::decode()`.** `Envelope` carries native in-memory
  `VersionVector`/`Entry` values through `swarm-sim` — nothing today
  serializes then deserializes a VV over real bytes. Gets a call site (and a
  test) only once `swarm-net` (Phase 2) does byte-level wire I/O.
- **`VersionVector::merge()`.** Not merely deferred — actively wrong to add,
  per §9.3's security-relevant rule: `causal_vv` must never absorb a peer's
  self-reported vector. A `merge()` free function sitting unused in the crate
  would be an invitation to violate that rule out of convenience later.
- **Anti-entropy fill-reply cap.** No per-round cap on how many entries one
  `AntiEntropy` reply may push (§9.5). Revisit only if a scenario demonstrates
  the existing network-queue bound is insufficient.
- **`Event::AntiEntropy` / `Effect::SendAntiEntropy` as dedicated variants.**
  Considered and rejected (§9.1); `Envelope` dispatch is preferred. Revisit
  only if a future milestone needs anti-entropy traffic distinguishable from
  entry traffic at the `Event`/`Effect` level itself (e.g. different
  RNG/backpressure treatment).
- **OR-set `remove`, tombstones, causal-stability GC.** Rejected for now
  (§10.4), not merely postponed: adding `remove` without the GC `DESIGN.md`
  §4.2 and §7 require would create an unbounded structure. Revisit only when a
  milestone needs a claim genuinely retracted rather than recorded as
  stood-down.
- **Per-node `priority`.** No configuration knob (§10.6). Add one when a
  scenario produces real priorities.
- **LWW telemetry register and sensor-track OR-set** (`DESIGN.md` §4.2). Not
  in any milestone's acceptance text yet.
- **Task assignment from outside.** Tasks are numbered `0, 1, 2, …` by each
  node's own claim count (§10.6). A real mission would receive assignments;
  Phase 1's `TaskId` is deliberately abstract.
- **Per-link RNG streams.** The simulator uses one global stream, so activity
  on one link perturbs draws on every other. Traces stay deterministic but
  are fragile — adding a message anywhere shifts everything after it. Turned
  out not to be needed through M4; if it becomes painful debugging M5–M6,
  switch to a per-link stream derived from `seed ⊕ H(src, dst)`.
- **Message duplication as a simulator feature.** Not modelled directly.
  Anti-entropy already produces duplicates naturally, which is the more
  realistic source; revisit only if that proves insufficient.
- **Byzantine transport.** M4's cheating node lies at the *protocol* layer,
  not the channel layer (§11.6). The simulator stays honest: it drops and
  delays, it does not forge.
- **A cap on `State::poes`.** Bounded by roster size already — at most one
  proof per accused node (§11.3) — so no separate cap was added.
- **Roster changes mid-run.** Out of scope for all of Phase 1 (`DESIGN.md`
  §7).
- **Forward-compatible wire format.** No "skip what you don't understand"
  for unrecognised `Body` tags (§20.1's `DecodeError::UnknownBodyTag`); an
  unrecognised tag is an error, full stop.

---

## 17. Dependencies

```
swarm-core:  blake3, ed25519-dalek (default-features = false, alloc feature)
swarm-sim:   swarm-core, rand_chacha, blake3, ed25519-dalek
```

`swarm-core` started at zero dependencies (M0) and gained `blake3` and
`ed25519-dalek` at M1 — both `default-features = false` so the crate remains
`no_std` and the thumbv7em cross-compile (§3) keeps proving it. `serde` is
and stays absent (§8.2). M2 and M3 added no further `swarm-core` dependency.

`swarm-sim` gained `ed25519-dalek` as a real (non-dev) dependency at M2: the
simulator signs on behalf of each simulated node to construct its `State`,
which M0/M1 never needed.

**On the RNG.** `DESIGN.md` lists `rand`. The concrete generator is
`rand_chacha::ChaCha8Rng` rather than `rand::rngs::StdRng`, because `StdRng`'s
documentation explicitly disclaims value stability across releases: a routine
`cargo update` could silently change what seed 42 produces and break the
determinism claim without breaking the build. `ChaCha8Rng` carries a
reproducibility guarantee. This is a refinement of `DESIGN.md`'s dependency
list, not a departure from it.

`turmoil` and `madsim` are excluded, per `DESIGN.md`: both are built on tokio
and would force async throughout.

---

## 18. Roadmap

*Not yet binding.* This section exists so "what comes next" is answered in
the same document as "what exists today," per the goal of this consolidation.
It restates `DESIGN.md` §9's acceptance criteria for context; it does not
make any of the implementation decisions those milestones will need — those
get written here, in the relevant section above, when each milestone starts,
exactly as M1–M4 did.

**M5 — Escrow counter and I4.** ✅ **Done.** Each node is granted a fixed,
pre-authorized spending budget (3 units per node, per `DESIGN.md` §M5). A node
spends 1 unit per `entry_period` tick while its budget remains, authoring
`Body::Spend { amount: 1 }`. The escrow counter folds every applied Spend entry
into a cumulative `spent` map; `remaining(node)` is budget minus spent. I4
holds structurally: each node's per-entry cap bounds its own spending, so the
global sum is bounded by the sum of per-node allocations — no consensus, no
quorum. Tested under random partitions and message loss across 1000 seeds.
Budget transfers, the `policy/` module, and I5/I6 are deferred to M6.

**M6 — Invariant checker and property tests.** ✅ **Done.** I1–I4 are
executable checks (`swarm-verify::check_invariants`), run across 5000 random
seeds with `proptest` — zero violations. I5 and I6 are not executable checks;
they are structurally discharged by the `policy/` module (`Action`/`Class`/
`commit` from `DESIGN.md` §4.5): `commit()` is the single path to effect
creation (I6), and no type in the crate implements `Action` with `CLASS =
SafetyCritical` (I5) — `SafetyCriticalAction`'s `compile_fail` doctest is the
concrete proof. `swarm-verify` crate provides an independent invariant checker
(`check_invariants`) usable from any test or tool. A `mutant-i3` build —
a cargo feature, off by default, that makes `Claims::winner` prefer the
observing node's own claim — makes the checker report a real I3 violation
where the clean build reports none; `scripts/verify.sh` runs both directions
and requires the mutant one to fail (§1, §15). The
`step` cloning cost (§3.3) was revisited and remains acceptable — Phase 4's
folding scheme depends on the pure signature.

After M6, Phase 1's exit criteria (`DESIGN.md`, "Faz 1 çıkış kriteri") are:
thousands of seeds, zero invariant violations, and a failing run on a
deliberately broken build; a single terminal demo tellable in 90 seconds; and
this document being readable by someone who was not in the room when it was
written.

---

## 19. Changelog

| Milestone | Change |
|---|---|
| M0 | Sans-I/O boundary; channel semantics; determinism contract; trace format; placeholder node behaviour (retired). |
| M1 | `Entry`, canonical encoding with domain separation, Ed25519 signatures, per-node hash chain, end-to-end verifier with the `VerifiedEntry` type gate, bounded log, golden vector, I1. |
| M2 | `Envelope` (`Entry \| AntiEntropy`) replaces the M0 placeholder payload; self-inclusive `deps` population; causal delivery with fixed-point buffer drain; bounded causal buffer with drop-oldest eviction; advertise-then-push-reply anti-entropy; `State` gains `log`, `origins`, `causal_vv`, `buffer`; I2 and I3 promoted to binding. |
| M3 | `Body::Withdraw`; `logical_clock` derived from `deps`; `state` module with `Map<TaskId, ORSet<Claim>>` and the `min by (priority, lc, node, seq)` winner rule; grow-only claim set with withdrawal as a log record, not a set removal; tick-phase-only authoring with claim → withdraw → advertise ordering; `State` gains `claims`; I3 strengthened to cover derived CRDT state; third golden vector; entry bodies rendered in the trace. |
| — | Cleanup pass (post-M3, pre-M4): four per-milestone spec files consolidated into this single topic-organized document; removed two unused public methods (`State::origins()`, `Entry::verify_signature()`) that had no call site outside their own tests. |
| M4 | `fault` module: `Poe` (proof of equivocation) and `verify_poe`, self-verifying against the roster alone; `State` gains `poes` and the `detect_equivocation` check run on every entry receipt against the log, `origins`, and the causal buffer; anti-entropy's fill-reply range changed to overlap by one with the peer's claimed head, clamped to this node's own highest, so a numeric-gap-free fork still surfaces; `swarm-sim` gains `SimConfig::equivocation` and the `Equivocation` scenario type; trace gains the `EQUIVOCATION` record. |
| M5 | `Body::Spend { amount: u64 }`, tag `0x02`; `Escrow` struct (`allocations` + cumulative `spent`) in `state` module; `State` gains `escrow` with `with_budgets` builder; spending logic in `Event::Tick` — 1 unit per `entry_period` while budget remains; `Escrow::observe` folds Spend entries in `attempt_apply` and `author`; I4 discharged structurally (per-node caps bound the global sum); fourth golden vector; `SimConfig` gains `budget_per_node` (default 3); integration test `m5_escrow.rs` with 1000 random-loss seeds + 200 partition+loss seeds + deliberate-overspend negative test. |
| M6 | `policy` module: `Class` enum (`Degradable`, `ExclusiveCostly`, `SafetyCritical`), `Action` trait (`CLASS`, `Cert`, `body`), `commit()` gate — the single path through which entries produce effects (I6); concrete Phase 1 actions (`TaskClaim`, `Withdraw`, `Spend`) all `Degradable` with `Cert = ()`; `SafetyCriticalAction` defined but not `Action` in Phase 1 — I5 structurally discharged because no `Action` impl has `CLASS = SafetyCritical` at all (not because `Cert` is type-tied to `Class`; nothing enforces that), proven by a `compile_fail` doctest. `author()` removed; all authorship and effect emission now goes through `policy::author_and_commit`. `swarm-verify` crate: independent `check_invariants(states, budgets) -> Vec<Violation>` checking I1–I4 (I5/I6 are structural, not executable-checked); added to workspace members. `proptest` integration: `swarm-sim/tests/m6_property.rs` runs 5000 random seeds with loss, checking invariants via `swarm-verify`; a `mutant-i3` cargo feature (off by default) breaks `Claims::winner`'s tie-break, and the same test run against that build reports a real I3 violation — `scripts/verify.sh` runs both directions. I5/I6 promoted to binding (structural) in invariants table. |
| M7 | External verifier (§20). `swarm_core::codec`: `decode_entry`/`decode_version_vector`/`decode_body`, the exact inverse of §8.2's encoders, `no_std` + `alloc`, canonicity-enforcing (`DecodeError::NonCanonical` rejects a non-strictly-ascending `VersionVector`); `decode_entry_exact` for single-buffer use. `swarm_core::bundle`: `LogBundle` (observer-keyed, then author-keyed, raw signed `Entry` only — no derived state), `Spec` (`mission_id`, `epoch`, `roster`, `budgets`, `log_cap`; unsigned in Phase 1 by design), both with canonical codecs; `State::export_bundle()`; `LogBundle::merge`. `swarm-verify` gains `verdict` (`Verdict`, `InvariantResult`, `Witness`, `ChainFinding`/`ChainProblem` — every `Violated` carries raw signed `Entry` values, never a formatted string; `input_attestable: bool`, always `false`), `fold` (an independent causal-replay and winner-rule reimplementation — never calls `swarm_core::state::{Claims, Escrow}`, so it structurally cannot inherit a bug planted in that fold, demonstrated against `mutant-i3`), and `verify(bundle, spec) -> Verdict`, checking I1 (cross-observer equivocation scan, each hit re-confirmed via `verify_poe`), I2 (a genuine temporal check via fixed-point replay, stronger than the oracle's final-`causal_vv` check), I3 (`Undetermined` without ≥2 comparable observers, never a vacuous `Satisfied`), and I4 (deduped-by-`(author,seq)` overspend) directly from bundle bytes. The old `check_invariants` is unchanged and kept as the *oracle*; presented in prose as an in-process trust-by-construction checker, not the product surface. New `swarm-verify` binary (`--bundle`, `--spec`, `--json`; exit `0`/`1`/`2`); `swarm-sim`'s `phase1` example gains `--equivocation`/`--export-bundle`/`--export-spec` (byte-identical output with no flags). Six-fixture corpus (`clean`, `equivocation`, `overspend`, `broken_chain`, `missing_node`, `truncated`) with a shared-source regenerator and a reproducibility test. `scripts/verify.sh` gains the `thumbv7em` cross-compile and `cargo clippy -D warnings` steps; `README.md` and `LICENSE` added. |

---

## 20. External verification (M7)

Everything above this section describes a system that can only be checked
from *inside* the process that ran it: `check_invariants` (now called the
*oracle*, §20.5) takes a live `&BTreeMap<NodeId, State>`. M7 does not change
what is checked — I1–I4 mean exactly what §15 says they mean — it changes
*where* the check can run: from bytes on disk, by a party with no access to
the process that produced them.

Three terms recur through this section:

- **Oracle** — a checker that runs from inside the simulation, trusted by
  construction. `check_invariants` (§20.5) is this, and is honestly
  presented as this rather than as a product surface.
- **Verifier** — a function that judges a claim from serialized evidence
  alone, with no access to the process that produced it. `verify` (§20.5) is
  this.
- **Witness** — the minimal evidence carried inside a `Violated` result: raw
  signed `Entry` values a reader checks independently, never a summary
  string or a derived value (§20.4).

### 20.1 Decode: the inverse of §8.2's encoders

`swarm_core::codec` adds what §8.2 never needed until now: a decoder for
every one of §8.2's encoders. `decode_entry`, `decode_version_vector`, and
`decode_body` are pure `&[u8] -> Result<(T, usize), DecodeError>` functions —
no I/O, `no_std` + `alloc`, living in `swarm-core` alongside the encoders
they invert. The `usize` is bytes consumed, so a caller can decode several
values back to back out of one buffer without knowing any one value's length
in advance — required by `LogBundle` (§20.2), which packs many entries into
one file.

**A decoder answers "what do these bytes mean," never "is this correct."**
Out-of-`seq`-order entries, gaps, and duplicate `seq` within a chain all
decode without error — whether that is a violation is `verify`'s question
(§20.5), not the decoder's. This split matters concretely: a duplicate `seq`
inside a decoded chain is exactly how a proof of equivocation survives the
round trip to be caught downstream, rather than being rejected as a format
error before `verify` ever sees it.

**Canonicity of the pieces themselves is not optional, though.** If two
distinct byte strings could decode to the same `VersionVector`, an attacker
could manufacture bytes that verify as a second, conflicting signed entry at
a `(node, seq)` an honest node already holds — a fabricated proof of
equivocation framing an innocent signer (`DESIGN.md` §7). `decode_version_vector`
therefore rejects any encoding whose `(NodeId, seq)` components are not
**strictly** ascending by `NodeId` — equal (a repeated `NodeId`) or
descending both fail — with `DecodeError::NonCanonical("version_vector_order")`.

`DecodeError` variants:

| Variant | Meaning |
|---|---|
| `Truncated` | Fewer bytes were present than the format requires at this point. |
| `UnknownBodyTag(u8)` | A `Body` tag outside `0x00..=0x02`. There is no "skip what you don't understand": forward compatibility is out of scope for Phase 1 (§16), so an unrecognised tag is an error, full stop. |
| `BadDomainTag` | The leading bytes do not match `wire::DOMAIN_TAG`. |
| `TrailingBytes` | Bytes remained after a value expected to consume the whole buffer. `decode_entry` itself never raises this — it reports bytes consumed and lets the caller decide, since `LogBundle`/`Spec` decode many entries out of one buffer and check for trailing bytes only once, after the last one. `decode_entry_exact` is the single-value form that does check, used where a buffer is known to hold exactly one entry (the golden-vector reverse tests). |
| `NonCanonical(&'static str)` | The bytes do not correspond to the canonical encoding of anything. The string names which rule was violated, for diagnostics; two `NonCanonical` values compare equal only if the reason string matches too, but callers should match on the variant, not the string. |
| `BadVerifyingKey` | 32 bytes that do not decode to a valid Ed25519 point (used by `Spec`'s roster decoding, §20.3). |

Body tags are written in `codec.rs` as the literals from the table in §8.2,
independently of `wire::Body::encode`, following the golden vectors'
existing convention (`tests/golden_vector.rs`'s header comment: "computed
independently from the spec layout") — an inverse that shares code with the
thing it inverts can share a bug with it too.

**Testing.** `swarm-core/tests/decode.rs` proves `decode_entry_exact(&e.encoded()) == e`
for 5000 `proptest`-generated entries (dev-dependency only — the `no_std`
build carries no new dependency). `swarm-core/src/codec.rs`'s own unit tests
cover the five canonicity failures by hand-built bytes: out-of-order
`VersionVector`, a repeated `NodeId`, a truncated entry, trailing bytes (via
`decode_entry_exact`), and an unknown body tag. All four golden vectors
(§8.5) are tested in both directions in `tests/golden_vector.rs`: the
existing forward `encode(known Entry) == pinned hex` tests are joined by
`decode(pinned hex) == known Entry` — the same fixture, proving the format
losslessly both ways.

### 20.2 `LogBundle`: the raw evidence

A `LogBundle` is the only thing a node hands to a verifier: raw signed
`Entry` values, nothing derived. `claims`, `escrow`, `causal_vv` never
appear — accepting derived state as an input would mean assuming the answer
to the question `verify` (§20.5) exists to ask.

```rust
pub struct LogBundle {
    pub mission_id: [u8; 32],
    pub epoch: u32,
    pub views: BTreeMap<NodeId, BTreeMap<NodeId, Vec<Entry>>>,
}
```

**Keyed by observer, not only by author.** An earlier shape of this type —
`chains: BTreeMap<NodeId, Vec<Entry>>`, one chain per author — cannot express
I3 at all: `Divergence` (§20.4) compares what two different *observers*
derived from what they each hold, and an author-only bundle has no observers
in it. It would also only witness equivocation if a single author's "chain"
were allowed to hold two entries at one `seq`, which is exactly the shape a
canonical per-author log must reject. `views[observer][author]` is
`observer`'s own copy of `author`'s chain: two observers may (and, across a
genuine equivocation, do) hold different entries at the same `(author, seq)`.

**Canonical encoding:**

```text
b"SWARM_BUNDLE_V1"                 (15 bytes)
|| mission_id                      (32 bytes)
|| epoch                           (4 bytes, u32 BE)
|| view_count                      (2 bytes, u16 BE)
|| per view, observer strictly ascending by NodeId:
     observer                      (1 byte)
     chain_count                   (2 bytes, u16 BE)
     || per chain, author strictly ascending by NodeId:
          author                   (1 byte)
          entry_count              (4 bytes, u32 BE)
          entry * count            (full canonical Entry encoding, §8.2)
```

Within a chain, entries are written in whatever order they are held —
normally an author's `seq` order, but **the decoder does not check it**.
Disorder, a gap, or a duplicate `seq` inside a decoded chain is a *finding*
(§20.5's chain check, or I1 itself for a duplicate), not a format error: the
decoder's job stops at "what do these bytes mean." A duplicate `seq` with
two different encodings is precisely how a proof of equivocation survives
the round trip to be caught downstream — rejecting it here would make I1
unreachable through this format.

**A missing view is normal, not an error.** A node may have crashed, been
captured, or gone silent — the bundle simply lacks an observer for it, and
`verify` reports the affected invariants `Undetermined` rather than treating
absence as either a violation or a clean pass (§20.4, §20.5): silence is
ambiguous, and the verdict says so honestly instead of guessing.

**`State::export_bundle() -> LogBundle`** produces the single-observer
bundle for `self.me`, reading only `log` and `origins` — the same two
fields `State::entries()` already reads (§9.6), grouped by author instead of
flattened. **`LogBundle::merge(self, other) -> LogBundle`** unions two
bundles' `views`, for assembling one file that covers a whole run out of
each node's individual export. Where both sides hold a chain for the same
`(observer, author)` pair, the longer one wins — two honest exports of the
same chain can only differ in how much of it each side had seen at export
time, so the longer is always a superset, never a conflicting alternative.
An actual conflict at that pair is exactly what I1 exists to catch, inside
`verify`, downstream of this merge — `merge` does not adjudicate it.

**Testing.** `swarm-core/tests/bundle.rs`: round-trip on a hand-built
multi-observer, multi-author bundle and on the empty bundle; a real
two-node simulation run whose `export_bundle()` output encodes, decodes, and
re-encodes byte-identically; `merge` unioning two single-observer bundles
and preferring the longer chain on a shared `(observer, author)` pair; and
the canonicity/format negative cases (bad domain tag, truncated, trailing
bytes, observers out of order, authors out of order within a view).

### 20.3 `Spec`: the rules to check the evidence against

```rust
pub struct Spec {
    pub mission_id: [u8; 32],
    pub epoch: u32,
    pub roster: Roster,
    pub budgets: BTreeMap<NodeId, u64>,
    pub log_cap: u32,
}
```

Before M7, `check_invariants`'s second argument was
`budgets: &BTreeMap<NodeId, u64>` alone — an ad hoc, unsigned, contextless
parameter, with `mission_id`, the roster, and the budget map never gathered
in one place. That made "check this log against a *different* spec" — the
independence test below — impossible to even state. `Spec` is that one
place.

**Canonical encoding:**

```text
b"SWARM_SPEC_V1"                   (13 bytes)
|| mission_id                      (32 bytes)
|| epoch                           (4 bytes, u32 BE)
|| roster_count                    (2 bytes, u16 BE)
|| (node u8 || verifying_key 32 bytes) * count, strictly ascending by NodeId
|| budget_count                    (2 bytes, u16 BE)
|| (node u8 || budget u64 BE) * count, strictly ascending by NodeId
|| log_cap                         (4 bytes, u32 BE)
```

A roster key that does not decode to a valid Ed25519 point is
`DecodeError::BadVerifyingKey`.

**`log_cap` is not decoration.** `verify` (§20.5) enforces
`chain.len() <= spec.log_cap` per author — the field has a call site from
the day it exists, per `DESIGN.md` §11.4's rule against unused configuration
knobs.

**Not signed in Phase 1.** An operator-signed `Spec` is Phase 2
(`DESIGN.md` §11.4 — nothing is opened before it has a use). The verifier
assumes the `Spec` it was handed is the right one for the mission; it does
not authenticate the spec itself. This is a stated limit, not a silent
assumption: `input_attestable` (§20.4) already carries the honest ceiling
this implies for the whole verdict.

`mission_id` now flows from `Spec` rather than only from the
`PHASE1_MISSION_ID` constant — the constant remains the default value
`swarm-sim` uses, but the path from a real `Spec` to `verify`'s mission-match
check is open.

**Testing.** `swarm-core/tests/spec.rs`: round-trip (including the empty
`Spec`) and re-encoding byte-identity; the format/canonicity negative cases
(bad domain tag, truncated, trailing bytes, roster out of order, budgets out
of order, a 32-byte value that is not a valid curve point). The
independence proof — the same bundle verifies clean against its real `Spec`
and reports an I4 violation against one with a lowered budget — needs
`verify` and is in `swarm-verify`'s test suite (§20.5), not here.

### 20.4 `Verdict` and `Witness`: a verdict that carries its own evidence

Before M7, the checker's output was:

```rust
pub struct Violation { pub invariant: &'static str, pub detail: String }
```

`detail` is formatted text. A reader cannot re-check it independently — they
can only trust that whatever computed the string got it right, which is
exactly the trust the whole project exists to remove. `Verdict` replaces
this for the external surface (`check_invariants`/`Violation` remain, as the
*oracle*'s own output — §20.5):

```rust
pub enum InvariantResult {
    Satisfied,
    Violated(Box<Witness>),
    Undetermined(&'static str),
}

pub enum Witness {
    Equivocation(Poe),
    UnmetDependency { observer: NodeId, entry: Entry, missing: (NodeId, u64) },
    Divergence { a: NodeId, b: NodeId, task: TaskId, winner_a: Option<Claim>, winner_b: Option<Claim> },
    Overspend { node: NodeId, budget: u64, entries: Vec<Entry> },
}

pub struct Verdict {
    pub chains: Vec<ChainFinding>,
    pub i1: InvariantResult,
    pub i2: InvariantResult,
    pub i3: InvariantResult,
    pub i4: InvariantResult,
    pub structural_note: &'static str,
    pub input_attestable: bool,
}
```

`Violated` boxes `Witness`: `Poe` alone carries two full `Entry` values, and
leaving it inline would size every `InvariantResult` — including every
`Satisfied` one — to the largest witness (`clippy::large_enum_variant`
catches this; the fix is one word, not a design change).

**Two deltas from a flat "one witness per invariant" design, both forced by
§20.2's per-observer bundle shape.** `UnmetDependency` names `observer`: with
per-observer views, the same entry can be timely for one observer and early
for another, so "unmet" is only meaningful relative to a specific observer's
replay. And `Verdict` gains `chains: Vec<ChainFinding>`, entirely outside
I1–I4:

```rust
pub struct ChainFinding {
    pub observer: NodeId,
    pub author: NodeId,
    pub error: ChainError,   // swarm_core::log::ChainError, reused as-is
    pub entries: Vec<Entry>,
}
```

A chain that fails `verify_chain` (§8.3) — wrong membership, mixed authors,
wrong mission or epoch, a `seq`/link break, a bad signature — or exceeds
`spec.log_cap` is not evidence *for or against* an invariant, it is malformed
evidence. Forcing it into I1–I4 would either hide it or misreport it as one
of the four; `chains` says it plainly instead. (The E7a fixture corpus's
`broken_chain.bundle` is exactly this case, and is why the doc's original
`Verdict` — which had nowhere to put this outcome — needed the addition.)

**The witness rule.** Every `Witness` variant carries the *minimal* raw
signed `Entry` values that demonstrate the violation, never a summary, a
hash, or a derived value. A reader must be able to check the signatures
against `spec.roster` themselves, with no code from this crate in the loop.
`Witness::Equivocation`'s `Poe` is the model case: `swarm_core::fault::verify_poe(roster, poe)`
checks it standing completely alone.

**`input_attestable: bool`, always `false` in Phase 1.** Deleting this field
because "it's always false anyway" is precisely the over-claim the project
exists to prevent: an absence of rule violations is not evidence that the
input itself was genuine (a bundle could be internally consistent and still
be a total fabrication signed by keys nobody vetted). Keeping it as a typed
field — not a doc comment, not a README caveat — means no future caller can
silently start reading `all_satisfied()` as "this run definitely happened."
Nothing in Phase 1 can set it to `true` (§5, "Kapsam dışı": no attestation
mechanism, including a TEE, is in scope).

**Testing.** `crates/swarm-verify/src/verdict.rs`'s own unit tests: a `Poe`
built independently, wrapped in `Witness::Equivocation`, still verifies
against the roster with none of `verdict.rs`'s own code touched — the E4
acceptance criterion; and `Verdict::all_satisfied`/`any_violated` behave
correctly on an all-`Satisfied` verdict and on one with `Undetermined`
results (which count as neither satisfied nor violated).

### 20.5 `verify(bundle, spec) -> Verdict`

```rust
pub fn verify(bundle: &LogBundle, spec: &Spec) -> Verdict;
```

`crates/swarm-verify/src/verify.rs`. Reads only its two arguments. No
simulator, no live `State`, no access to the process that produced `bundle`.

**Step 1 — chain verification, per `(observer, author)`.**
`swarm_core::log::verify_chain(&spec.roster, entries)` (§8.3: membership,
single-author, mission, epoch, `seq`, link, signature) plus
`entries.len() <= spec.log_cap`. A chain that fails either check is reported
as a `ChainFinding` and excluded from every step below — it contributes no
evidence to I1–I4, because malformed evidence is not evidence for or against
anything (§20.4). Chains that pass carry their raw `Entry` values forward
unchanged; nothing derived is computed here yet.

**Step 2 — causal replay, per observer.** `crates/swarm-verify/src/fold.rs`'s
`causal_replay` takes one observer's chain-verified chains and replays them
to the fixed point described in §9.3, reimplemented independently rather
than calling `swarm-core`'s own `drain_buffer`. Because Step 1 already
guarantees each chain's own `seq` is contiguous from zero, only cross-author
`deps` are left to resolve. `TaskClaim` entries in application order are
folded into `swarm-verify`'s own `BTreeMap<TaskId, Vec<Claim>>` — never
`swarm_core::state::Claims::observe`.

**Why an independent fold, not a call into `swarm_core::state`.** A verifier
that reconstructs derived state by calling the very code the system under
test uses for that reconstruction is a mirror, not a second opinion: it can
agree with a bug as readily as it agrees with correct behaviour, because it
is running the same computation. `swarm-verify` therefore restates the
winner rule itself — `winner(task)` is `Claim`'s derived `Ord`, `min` over
the folded `Vec<Claim>`, written fresh in `verify.rs` — and restates spend
accounting itself (I4, below), rather than calling
`swarm_core::state::{Claims, Escrow}`.

**The consequence, stated rather than discovered later.** The `mutant-i3`
feature (§15, §1) breaks the tie-break *inside*
`swarm_core::state::Claims::winner`. `verify` never calls that function, so it
cannot inherit that bug: on a `mutant-i3` build, the oracle
(`check_invariants`, below) reports an I3 violation over the tied-claim
scenario, and `verify` — computing the same winner independently, from
scratch, off the identical raw entries — still reports `Satisfied`. This is
not a gap to be closed; it is the entire point of doing the fold twice.
`crates/swarm-sim/tests/m7_equivalence.rs::verify_does_not_inherit_the_mutant_i3_tie_break`
demonstrates it directly, unconditionally, on both a clean and a `mutant-i3`
build — unlike `m6_property.rs::mutant_i3_detection`, it is not expected to
start failing under the mutant feature, because it asserts what `verify`
computes, not what the oracle computes.

**I1 — equivocation.** Every chain-verified entry in the whole bundle,
across every observer and author, is grouped by `(author, seq)`. Two
differently-encoded entries at the same key are handed to `Poe::new` and
confirmed with `verify_poe(&spec.roster, &poe)` before being reported —
`verify` does not trust its own grouping, it re-checks the proof the same
way an outside reader would. `Undetermined` only when the bundle holds zero
chain-verified entries anywhere.

**I2 — unmet dependency.** For each observer, any entry [`fold::Replay`]
could not reach at the fixed point is the witness directly: `first_missing_dep`
names the first `(origin, seq)` component (ascending by `NodeId`) that
observer's own held view does not cover. This is the temporal check §7b (E7b)
asks for, and it is stronger than the pre-M7 `check_i2`
(`crates/swarm-verify/src/lib.rs`), which tests only the *final* `causal_vv`
— by the end of a run that vector has grown to cover nearly everything, so
it catches "the dependency never arrived" but not "this was applied before
it arrived." A fixed-point replay that cannot reach an entry catches both,
because it is the same gate `swarm-core`'s own delivery rule (§9.3) applies,
independently re-run over the raw log. `Undetermined` only when the bundle
has no observers.

**I3 — divergence.** For every pair of observers whose *applied* `(author,
seq)` key-sets are identical — the same reasoning the pre-M7 `check_i3` used
(`crates/swarm-verify/src/lib.rs`): matching by key rather than by full
entry content is what lets a same-key/different-content pair (an
equivocation) surface as a derived-state disagreement rather than being
silently skipped — `winner(task)` is compared for every task either
observer holds a claim for. The first disagreement found becomes
`Witness::Divergence`. `Undetermined` when fewer than two observers are
present, *and* when two or more are present but no pair's key-sets coincide:
reporting `Satisfied` on zero comparable pairs would claim evidence the
bundle does not contain, which is exactly the over-claim §20.4's
`input_attestable` field exists to prevent elsewhere in this same type —
the same discipline applies here even though nothing forces it structurally.
This is narrower than the oracle's I3, which additionally compares full
`Claims` equality (including the withdrawn set); `verify`'s I3 covers
`winner(task)` agreement, which is `DESIGN.md` §4.2's stated property.

**I4 — overspend.** `Spend` entries from every observer's replay-applied
entries are deduplicated by `(author, seq)` — the same entry can legitimately
appear in more than one observer's view — and summed per node. A sum
exceeding `spec.budgets[node]` is reported with every one of that node's
`Spend` entries as the witness. `Undetermined` only when the bundle has no
observers; zero `Spend` activity is `Satisfied`, not `Undetermined` —
absence of spending is itself sufficient evidence that spending did not
exceed a budget, unlike I3's need for a comparison pair.

**`check_invariants` remains, unchanged in behaviour, as the oracle.** The
existing 5000-seed proptests (`crates/swarm-sim/tests/m6_property.rs`) keep
running against it untouched. It is presented in prose as what it always
was — a checker trusted by construction because it runs inside the
simulation it is checking — not as the project's external-facing product
surface; that role now belongs to `verify`.

**Testing.**

- `crates/swarm-verify/src/fold.rs`: independent chains all applying with no
  leftover; a satisfied cross-author dependency applying in the right order
  regardless of map iteration order; a missing cross-author dependency
  leaving an entry stuck, with `first_missing_dep` naming it correctly.
- `crates/swarm-verify/tests/verify.rs`: a clean single-observer bundle
  (I1/I2/I4 `Satisfied`, I3 `Undetermined` — one observer is not two); I1
  triggered by two observers holding differently-signed entries at the same
  `(author, seq)`; I2 triggered by an entry whose dependency the bundle
  never contains; I3 triggered by two observers sharing an `(author, seq)`
  key-set but holding different content at one key (built via a second,
  differently-signed copy of one entry — a live equivocation, which
  necessarily also trips I1, independently checked); I4 triggered by
  overspend, and the E3 independence proof (§20.3) — the identical bundle
  clean against a `Spec` with a sufficient budget, violated against one with
  a lowered budget; a broken signature and an over-`log_cap` chain each
  reported in `Verdict::chains`, contributing no evidence to any invariant;
  an entirely empty bundle `Undetermined` on all four.
- `crates/swarm-sim/tests/m7_equivalence.rs`: the acceptance criterion —
  5000 random seeds, clean build, `check_invariants(...).is_empty() ==
  !verify(...).any_violated()` on every one, plus a sanity check that an
  honest export never produces a `ChainFinding`. Scoped to *violation
  presence*, not full-output equality, and deliberately not claiming
  `Undetermined == Satisfied` — see the module's own header comment for why.
  The `mutant-i3` divergence above is demonstrated separately in the same
  file, not folded into this 5000-seed run.

### 20.6 The CLI: what a stranger runs

```text
swarm-verify --bundle <path> --spec <path> [--json]
```

`crates/swarm-verify/src/bin/swarm-verify.rs`. Argument parsing and JSON
rendering are both hand-written — no `clap`, no `serde` — the same
dependency discipline `wire`'s canonical encoding has always followed
(§8.2): a format this project claims is normative is never handed to a
general-purpose library to decide the shape of.

**This binary is where file I/O lives.** `swarm-core` stays sans-I/O (§3):
its decoders take `&[u8]`, never a path. `swarm-verify`'s library crate
(`verify`, `verdict`, `fold`) also touches no filesystem. Only this binary
calls `std::fs::read`/`std::fs::write` — the same sans-I/O boundary §3 draws
for `swarm-core`, held one crate further out.

**Exit codes:** `0` — every invariant `Satisfied` and `Verdict::chains`
empty. `1` — at least one invariant `Violated`, or at least one
`ChainFinding`. `2` — a usage, file, or decode error (missing argument,
unreadable file, malformed bundle or spec). `Undetermined` alone never
changes the exit code, but is always printed — a verdict that quietly
downgraded "I don't know" to a passing exit code would reintroduce exactly
the over-claim `input_attestable` (§20.4) exists to prevent. Every run,
human or `--json`, prints `input_attestable: false (Phase 1 — no input
attestation)` unconditionally.

**JSON.** Every raw `Entry` referenced anywhere in the output — inside a
`Witness`, inside a `ChainFinding` — is rendered as `{"author", "seq",
"hex"}`, where `hex` is that entry's full canonical encoding (§8.2), not a
summary. A reader can decode and check every one of them independently,
exactly the discipline `Witness` itself is built on (§20.4).

**`swarm-sim`'s `phase1` example gains three flags**, all optional and
additive: `--equivocation`, `--export-bundle <path>`, `--export-spec
<path>`. With none given, the example's behaviour and output are
byte-identical to the pre-M7 version — verified directly (`diff` against a
captured run of the previous commit). `--export-bundle`/`--export-spec`
write, as files, the states this same run already computed — no second
simulation, no different code path: `export_bundle_for` folds every node's
own `State::export_bundle()` (§20.2) into one `LogBundle` via `merge`.
`--equivocation` selects *which* of the example's two built-in scenarios
gets exported — the honest 5-node cohort (§1-4 of the example's narration)
by default, or §5's 3-node equivocation scenario when given. Without the
flag, `swarm-verify` reports every invariant `Satisfied`; with it, I1
`Violated` — the equivocator is node 2 in that scenario's 3-node roster.

**Testing.** `swarm-verify.rs`'s own unit tests cover argument parsing
(accepting all three flags, defaulting `--json` to off, rejecting a missing
`--bundle`/`--spec`, rejecting an unrecognised flag) and the JSON string
escaper. The exit-criterion scenario itself — §4's real acceptance test —
is run by hand end to end, not simulated in a unit test: `cargo run -p
swarm-sim --example phase1 -- --equivocation --export-bundle ...
--export-spec ...` followed by `cargo run -p swarm-verify -- --bundle ...
--spec ...` reproduces `I1: Violated (Equivocation by node 2)`, exit `1`,
exactly as this section and `README.md` state; the same two commands
without `--equivocation` report every invariant `Satisfied`, exit `0`.

### 20.7 The fixture corpus

`crates/swarm-verify/tests/fixtures/` — six scenarios, each a committed
`<name>.bundle` / `<name>.spec` pair:

| Fixture | Expected verdict |
|---|---|
| `clean` | All four invariants `Satisfied` |
| `equivocation` | I1 `Violated(Equivocation)` |
| `overspend` | I4 `Violated(Overspend)` |
| `broken_chain` | A `ChainFinding` (a tampered signature) — not an I1–I4 result |
| `missing_node` | I3 `Undetermined` — only one observer's view is present |
| `truncated` | `LogBundle::decode` fails with `DecodeError::Truncated` before `verify` ever runs |

**This corpus, not this document's prose, is what a second implementation
is measured against.** Two implementations can agree on every sentence of
§20.1–§20.6 and still disagree on a byte; a fixed set of files with a fixed
expected outcome is checkable mechanically, the way the golden vectors
(§8.5) already are for the wire format alone. The negative cases carry the
real weight: a corpus of only `clean` would show that the verifier doesn't
crash, not that it catches anything.

**Provenance.** Every fixture is built by one function in
`crates/swarm-verify/tests/support/fixture_data.rs` (fixed, seeded keys
throughout — the same discipline as the golden vectors), included by both
`crates/swarm-verify/examples/gen_fixtures.rs` (the regenerator:
`cargo run -p swarm-verify --example gen_fixtures` overwrites the committed
files) and `tests/fixtures.rs` (which asserts the regenerated bytes equal
the committed ones). One definition, two consumers — the alternative, a
regenerator that duplicates the test's own construction logic, is exactly
the kind of divergence this corpus exists to rule out elsewhere.

**Testing.** `crates/swarm-verify/tests/fixtures.rs`:
`regenerated_fixtures_match_committed_bytes` (byte-for-byte, all twelve
files); one test per fixture loading the *committed* bytes (not
`fixture_data`'s in-memory values — this is what a stranger who cloned the
repo and never ran the regenerator actually has) and asserting the table
above.
