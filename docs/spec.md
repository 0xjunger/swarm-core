# swarm-core — Technical Specification

> `DESIGN.md` (Turkish) is the project's source of truth for *what* and *why*.
> This document is the normative specification for *how*: the exact rules the
> code implements. Per `DESIGN.md` §11.6, a decision is written here **before**
> it enters code; per §11.5, a wire-format change updates the golden vectors
> (§8.5) and states the reason in the commit message.
>
> This is a single living document, organized by topic rather than by
> milestone. Earlier milestones did not each get a frozen file — a decision
> made at M2 and refined at M3 is described once, in its current form, in the
> section that owns the topic. §16 (Roadmap) tracks what is implemented and
> what is not; §17 (Changelog) is the per-milestone history for anyone who
> needs it. If you are looking for "what did M2 add," read the changelog; if
> you are looking for "how does causal delivery work today," read §9 — it will
> not send you chasing three other files to find out.

---

## 1. Status

**Implemented: M0, M1, M2, M3.** Sections below describe the system as it
exists today, not as it was at any past milestone. Where a rule changed shape
between milestones (e.g. `deps`, invariant I3), only the current shape is
normative; the change itself is recorded in §17.

**Not yet implemented:** M4 (equivocation detection), M5 (escrow / I4), M6
(property-based invariant checker), and everything in Phase 2+. §16 sketches
these without freezing decisions that are not yet made — do not treat §16 as
binding.

---

## 2. Repository layout

```
Cargo.toml            workspace
rust-toolchain.toml   pinned toolchain — part of the reproducibility claim
clippy.toml           mechanical enforcement of §3's I/O ban
DESIGN.md             source of truth (Turkish)
docs/spec.md           this file
crates/swarm-core/    no_std, no I/O, minimal dependencies (§15)
crates/swarm-sim/     std; the simulator that drives swarm-core
```

`DESIGN.md` §5 draws the crates as a flat tree. They live under `crates/` here
only because the workspace root directory is itself named `swarm-core`, and a
nested `swarm-core/swarm-core/` path is needlessly confusing. The module names
*inside* `swarm-core` (`wire`, `causal`, `log`, `state`, `policy`, `fault`)
follow §5 and are created as each milestone needs them; `wire`, `causal`,
`log`, and `state` exist today (M1–M3). `policy` and `fault` arrive at M5 and
M4 respectively.

`swarm-verify` and `swarm-net` do not exist yet (M6 and Phase 2 respectively).

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
derived logical clock, §11.2). The simulator itself has no access to real
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
stated bound — the causal buffer, the claim CRDT (§11.4), and every future
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
`FINAL`.

The first nine exist from M0. `APPLY` (an entry was applied to derived
state), `BUFFER` (an entry was held pending causal delivery), and
`DROP_CAUSAL_OVERFLOW` (a buffered entry was evicted, §9.3) were added at M2,
derived by diffing a node's `State` before and after each `step` call — `step`
itself stays pure and returns only `Effect`s (§3.3); this diffing is
simulator bookkeeping, not a change to the core's contract.

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
|---|---|---|---|
| `TaskClaim` | `0x00` | `task` (8 bytes, u64 BE) `\|\|` `priority` (1 byte, u8) | M1 |
| `Withdraw` | `0x01` | `task` (8 bytes, u64 BE) | M3 |

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
   (§13): a duplicated `(node, seq)` can never pass.
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
folding function (§11.3) — takes `VerifiedEntry`, never `Entry`. The one
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

At every call site that authors an entry (§10.4, §11.6), a full log is a
silent no-op — graceful degradation, not a crash. Not exercised at the
default `log_cap` used by `swarm-sim`.

### 8.5 Golden vectors

`swarm-core/tests/golden_vector.rs` pins, in hex, the signing bytes, the full
encoding, and the signature of known `Entry` values under a known key. Any
change to the wire format breaks these tests — **that is the point**: the
format must never change silently (`DESIGN.md`, item 5). A deliberate change
updates the golden vectors and states the reason in the commit message.

Three vectors, added as each shape of `Entry` first existed:

1. **M1** — `TaskClaim`, empty `deps`. The base case: single-variant body, no
   causal dependencies.
2. **M2** — `TaskClaim`, non-empty `deps` (two populated components). Proves
   the `VersionVector` encoding holds once the field actually carries data,
   without touching the M1 vector.
3. **M3** — `Withdraw`. Proves tag `0x01` and its single-field body, and that
   it produces different signed bytes than a `TaskClaim` naming the same task
   (so one signature can never be read as attesting to both).

All three remain byte-identical today. If any of them moves, something has
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
   `state.origins`, folded into `state.claims` (§11.3), and
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
why `VersionVector::merge()` does not exist (§14).

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
    claims: state::Claims,                                 // §11.3
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
them yet (§16).

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
invariant I3 (§13) discharged structurally rather than by argument.

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
two nodes that have seen the same entry set agree on the winner (I3, §13),
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

## 11. Bandwidth budget

*Per `DESIGN.md` §7's "baştan hesapla" requirement.*

From the frozen encoding (§8.2):

```
Entry (TaskClaim) = 14 (tag) + 32 (mission) + 4 (epoch) + 1 (node) + 8 (seq)
                   + 32 (prev) + (2 + 9·D) (deps, D = populated dep count)
                   + 10 (body: 1 tag + 8 task + 1 priority) + 64 (sig)
                   = 165 + 9·D bytes
Entry (Withdraw)   = same, but body is 9 B (1 tag + 8 task) instead of 10
                   = 164 + 9·D bytes
VersionVector      = 2 + 9·N bytes   (N = roster size)
```

At the roster cap `N ≤ 20` (`DESIGN.md` §4.5): `AntiEntropy` ≤ 182 B, `Entry`
(worst case `D = N`) ≤ 345 B. A fill reply after a long partition costs
`(missing count) × Entry size`, self-limited by the network queue bound
(§5.5) rather than an explicit per-round cap.

M3 roughly doubled the steady-state entry rate: a node may now emit one claim
*and* one withdrawal per `entry_period` instead of one claim. Still
self-limited by the same bounded queue; no new cap was introduced.

---

## 12. Trace and simulator internals

Covered by §7 (format) and §6 (the loop). Nothing in this section is separate
from those; listed here only as a pointer for anyone looking for "where is
the simulator specified" — the answer is §5–§7, not a separate document.

---

## 13. Invariants

Per `DESIGN.md` §11.7, invariants are written before the code that guards
them. This table reflects the current, cumulative status — not a per-milestone
snapshot.

| # | Invariant | Status |
|---|---|---|
| **I1** | At most one signed entry per `(node, seq)` | **Binding.** Enforced by construction (`seq` = chain length, §8.3) and by verification (§8.3 rule 5 rejects duplicates). Tested in `swarm-core/tests/invariants.rs`. |
| **I2** | An entry is not applied before its `deps` are delivered | **Binding.** §9.3's delivery rule is the enforcement; tested in `swarm-core/tests/causal.rs` (buffering, cross-node deps) and `swarm-core/tests/invariants.rs`. |
| **I3** | Two nodes that have seen the same entry set derive the same state | **Binding, and strengthened at M3.** "Derived state" now means `causal_vv`, the entry set, `claims`, **and `winner(t)` for every task `t`** — not just the version vector. Discharged structurally by §10.3 (set insertion is commutative and idempotent) and §10.5 (losing is monotone); tested in `swarm-core/tests/invariants.rs` and end to end by `swarm-sim/tests/m2_convergence.rs` and `swarm-sim/tests/m3_claim.rs`. |
| I4 | Spendable rights across all partitions ≤ authorised total | Not yet implemented — activates at M5 (escrow). |
| I5 | No safety-critical effect without a valid certificate in the log | Not yet implemented — activates with the policy gate (M5). |
| I6 | Every effect is traceable to a signed entry chain | Partially discharged: entries cause `Effect::Send`s directly, and a withdrawal is traceable to the claims that caused it. The full policy-gated claim is M5+. |

---

## 14. Deferred

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
  are fragile — adding a message anywhere shifts everything after it. If this
  becomes painful when debugging M4–M6, switch to a per-link stream derived
  from `seed ⊕ H(src, dst)`. Not needed yet.
- **Message duplication as a simulator feature.** Not modelled directly.
  Anti-entropy already produces duplicates naturally, which is the more
  realistic source; revisit only if that proves insufficient.
- **Byzantine transport.** M4's cheating node lies at the *protocol* layer,
  not the channel layer. The simulator stays honest: it drops and delays, it
  does not forge.
- **Roster changes mid-run.** Out of scope for all of Phase 1 (`DESIGN.md`
  §7).

---

## 15. Dependencies

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

## 16. Roadmap

*Not yet binding.* This section exists so "what comes next" is answered in
the same document as "what exists today," per the goal of this consolidation.
It restates `DESIGN.md` §9's acceptance criteria for context; it does not
make any of the implementation decisions those milestones will need — those
get written here, in the relevant section above, when each milestone starts,
exactly as M1–M3 did.

**M4 — Equivocation detector.** A deliberately faulty node signs two
different entries at the same `(node, seq)` and sends one to each side of a
partition. On reunion, any node holding both signed entries can produce a
~200-byte proof-of-equivocation that a third node, with no other context,
verifies unilaterally. Adds `fault/` (`DESIGN.md` §5). This is where the
"Byzantine transport" boundary in §14 gets exercised for the first time — the
simulator still does not forge messages; the faulty *node* does, at the
protocol layer.

**M5 — Escrow counter and I4.** Each node is granted a fixed, pre-authorized
spending budget it can spend without coordination. Under randomized
partition/merge churn across 1000+ seeds, total spend never exceeds the
authorized total. Activates I4 and I5, and introduces `policy/`
(`Action`/`Class`/`commit` from `DESIGN.md` §4.5). This is also where the
`step` cloning cost (§3.3) gets revisited if it has become a real cost, per
the note there.

**M6 — Invariant checker and property tests.** I1–I6 become executable
checks run across thousands of seeds with `proptest`; a deliberately broken
variant (e.g. tie-break on `NodeId` replaced with something non-deterministic)
must fail the suite, so the green baseline is proven to catch something.
`swarm-verify` is created here.

After M6, Phase 1's exit criteria (`DESIGN.md`, "Faz 1 çıkış kriteri") are:
thousands of seeds, zero invariant violations, and a failing run on a
deliberately broken build; a single terminal demo tellable in 90 seconds; and
this document being readable by someone who was not in the room when it was
written.

---

## 17. Changelog

| Milestone | Change |
|---|---|
| M0 | Sans-I/O boundary; channel semantics; determinism contract; trace format; placeholder node behaviour (retired). |
| M1 | `Entry`, canonical encoding with domain separation, Ed25519 signatures, per-node hash chain, end-to-end verifier with the `VerifiedEntry` type gate, bounded log, golden vector, I1. |
| M2 | `Envelope` (`Entry \| AntiEntropy`) replaces the M0 placeholder payload; self-inclusive `deps` population; causal delivery with fixed-point buffer drain; bounded causal buffer with drop-oldest eviction; advertise-then-push-reply anti-entropy; `State` gains `log`, `origins`, `causal_vv`, `buffer`; I2 and I3 promoted to binding. |
| M3 | `Body::Withdraw`; `logical_clock` derived from `deps`; `state` module with `Map<TaskId, ORSet<Claim>>` and the `min by (priority, lc, node, seq)` winner rule; grow-only claim set with withdrawal as a log record, not a set removal; tick-phase-only authoring with claim → withdraw → advertise ordering; `State` gains `claims`; I3 strengthened to cover derived CRDT state; third golden vector; entry bodies rendered in the trace. |
| — | Cleanup pass (post-M3, pre-M4): four per-milestone spec files consolidated into this single topic-organized document; removed two unused public methods (`State::origins()`, `Entry::verify_signature()`) that had no call site outside their own tests. |
