# Swarm Core — Specification

## 1. Status and conventions

### 1.1 Version and date

Version: 0.1.0. Date: 2026-08-10.

This specification describes the system as it exists today. It is not a
historical record — for the history of how each rule was added, see
`CHANGELOG.md`. For the reasoning behind each rule, see `DESIGN.md`.

### 1.2 Compatibility policy

A change to the signing bytes layout (§5.3), to a domain separation tag
(§5.2), or to a canonical encoding rule (§5.1) is a breaking change. A
breaking change requires an increase of the major version number.

Every implementation detail in this document — a crate name, a function
name, a Rust type — appears only inside a paragraph or note marked
**non-normative**. A non-normative note explains how the current
implementation realizes a rule; it does not add a rule of its own. Removing
a non-normative note must never change what a conforming implementation is
required to do.

### 1.3 Keyword usage

The keywords MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY in this document
carry the meaning defined in RFC 2119. This document does not use SHALL;
MUST carries that same meaning.

### 1.4 Notation

`V(log, spec) -> Verdict` names the function this specification defines: it
takes a log of signed actions and a specification of permitted actions, and
produces a machine-checkable verdict. A plain-language sentence follows any
mathematical or set notation on its first use. A Rust code identifier (for
example `LogBundle`, `swarm_core::wire::Entry`) keeps its exact spelling,
including `snake_case` and `::` — an identifier is not prose and is not
translated or reworded.

---

## 2. Scope

### 2.1 What this specification defines

- The wire encoding of a signed log entry, and every value nested inside it
  (§4, §5).
- The rule under which an entry is causally delivered and folded into
  derived state (§4.3, §7.2).
- The six invariants a swarm's behaviour must satisfy (§6).
- The input format `V` accepts — a bundle of raw signed entries and a
  specification of the mission's roster and rules (§4.4, §4.5) — and the
  output format it produces (§4.6).
- The verification procedure itself: what `V` checks, in what order, and
  what evidence it reports for a violation (§7).
- The test-vector corpus that makes this specification checkable by a
  second, independent implementation (§8).

### 2.2 What this specification does not define

- A network transport. This specification describes what a signed entry
  means and how a set of entries is checked; it does not describe how
  entries travel between nodes.
- Wall-clock synchronization. No rule in this specification depends on real
  time; logical time and sequence numbers are the only ordering this system
  uses.
- Hardware or flight integration of any kind.
- An attestation mechanism (for example a trusted execution environment).
  Nothing in this phase of the project can mark an input as attested — see
  §2.3.
- An economic mechanism. Accountability in this system is forensic and
  operational: a violation is recorded and machine-checkable. Nothing in
  this specification imposes an economic penalty.

### 2.3 The epistemic ceiling

`V` proves procedural compliance. `V` does not prove factual truth.

A node that reports a false sensor input, then acts consistently on that
false input, produces a valid, internally consistent, fully verifiable proof
of a wrong decision. Nothing in this specification distinguishes that case
from an honest one, because nothing in the signed log carries independent
evidence of what actually happened in the physical world — only of what the
node claimed and signed.

For this reason, a verdict carries two separate dimensions, not one:

1. Whether a rule violation is present in the log (§6, the four checkable
   invariants I1–I4, and the two structurally-held invariants I5–I6).
2. Whether the input itself is attestable — that is, whether there is any
   basis, beyond the signatures being valid, for trusting that the log
   reflects what actually happened. In this phase, this is always `false`
   (§4.6, §9). An absence of rule violations is never, by itself, evidence
   that a bundle's contents are genuine.

A reader MUST NOT treat a verdict with no violations as a claim that the
underlying events occurred as logged. It is a claim that, taken at face
value, the log does not show a rule being broken.

---

## 3. Model

### 3.1 The function `V(log, spec) -> Verdict`

`V` receives a log of the signed actions of a swarm and a specification of
the actions permitted for that swarm's mission, and produces a verdict: a
machine-checkable statement of whether the log shows a rule violation, for
each of the invariants in §6.

**Non-normative.** The current implementation names this function
`verify(bundle: &LogBundle, spec: &Spec) -> Verdict` (§4.4–§4.6, §7).

### 3.2 Nodes, logs, and authority

A **node** is a member of a mission's roster, identified by a fixed
identifier. The roster is fixed for the scope of this specification: no
rule here describes adding or removing a node mid-mission.

Each node keeps its own **log**: an append-only, per-node sequence of signed
entries (§4.1, §4.2). A node's authority to take an action is expressed
through the consistency class of that action (§3.3) and, where required, a
certificate recorded in the log alongside it.

### 3.3 Consistency classes

Every action a node can take belongs to exactly one of three consistency
classes:

| Class | Certificate required | Meaning |
|---|---|---|
| Degradable | None | A locally decided action. Convergence, not exclusion, is the guarantee (§4.3, I3). |
| ExclusiveCostly | A quorum certificate | An action drawn from a shared, bounded resource. The certificate is scoped to one partition; it does not establish global mutual exclusion on its own — the bound in §6.4 (I4) does. |
| SafetyCritical | An operator signature or a global threshold certificate | An action that must not occur without an explicit, checkable grant of authority. No effect of this class may be produced without a certificate present in the log (§6.5, I5). A request for this class of authority that is refused MUST still be recorded, so the refusal itself is checkable evidence. |

A conforming implementation MUST NOT provide any way to produce an effect of
one class using the certificate requirements of another.

---

## 4. Data model

### 4.1 The log entry

An entry is the unit this whole system is built from: it is simultaneously
the message a node publishes, the record in that node's log, and the proof
object a verifier checks. One signature over one canonical encoding serves
all three roles.

| Field | Type | Meaning |
|---|---|---|
| `mission_id` | 32 bytes | Identifies the mission. Prevents an entry signed for one mission from being replayed as valid in another. |
| `epoch` | `u32` | The roster version this entry was authored under. |
| `node` | `u8` | The identifier of the authoring node. |
| `seq` | `u64` | This node's monotonic log index. The genesis entry has `seq = 0`; each successor entry has `seq` exactly one greater than its predecessor. A `seq` value is never reused within one node's log. |
| `prev` | 32 bytes | The hash of the predecessor entry's full canonical encoding (§4.2). All-zero for the genesis entry. |
| `deps` | version vector | A snapshot of the author's own causal knowledge at the moment of authorship, self-inclusive (§4.3). |
| `body` | one of the variants below | The entry's meaning. |
| `sig` | 64 bytes | An Ed25519 signature over the entry's canonical signing bytes (§5.3). |

**Body variants:**

| Variant | Fields |
|---|---|
| `TaskClaim` | `task` (`u64`), `priority` (`u8`) |
| `Withdraw` | `task` (`u64`) |
| `Spend` | `amount` (`u64`) |

A decoder MUST reject a body tag outside this set as an error. This
specification does not define forward compatibility for an unrecognized
body variant — an implementation MUST treat an unknown tag as a decode
failure, not as a value to be skipped.

### 4.2 The per-node hash chain

A node's log is an append-only chain of entries, bounded to a stated
capacity. Appending to a full log MUST fail rather than silently drop or
overwrite an entry, and MUST NOT evict an existing entry to make room.

**Chain linkage.** The genesis entry (`seq = 0`) has `prev` equal to 32
zero bytes. Every successor entry has `prev` equal to the BLAKE3 hash of its
predecessor's full encoding — signing bytes together with the signature —
so that altering a previously-signed entry, including its signature alone,
breaks every following link.

**Chain verification.** Given a roster and a sequence of entries claimed to
belong to one node's chain, a verifier MUST check, in order, for each entry,
stopping at the first failure and reporting the failing index:

1. The entry's `node` is a member of the roster.
2. The entry's `node` equals the first entry's `node` — a chain belongs to
   exactly one author.
3. The entry's `mission_id` equals the roster's.
4. The entry's `epoch` equals the roster's.
5. The entry's `seq` equals the expected value — `0` for the first entry,
   the previous entry's `seq + 1` thereafter. A duplicate `(node, seq)`
   pair can never pass this check on a single chain (§6.1, I1).
6. The entry's `prev` equals the expected chain link.
7. The entry's Ed25519 signature verifies against the roster's recorded
   key for `node`.

A single-entry form of the same seven checks (minus the single-author rule,
which only applies across a full chain) MUST be available for verifying one
entry against a known predecessor hash and expected `seq`, for use where
entries are checked one at a time as their causal dependencies clear (§7.2).

**Verified vs. unverified entries.** An entry that has not passed chain
verification MUST NOT be folded into any derived state (§4.3, §6.3). A
conforming implementation MUST make this distinction impossible to skip by
accident — for example, by using the type system so that only a value
produced by successful verification can be passed to a folding function.

**Non-normative.** The reference implementation expresses this as two
distinct Rust types, `Entry` (untrusted) and `VerifiedEntry` (the output of
verification), with a construction path reserved for a node's own freshly
authored entry, since that entry is correct by construction (it was just
signed, over its own chain head, with its own key) and does not need to be
round-tripped through the verifier that exists to check untrusted input.

### 4.3 The version vector

A version vector records, for a set of nodes, the highest `seq` seen from
each. It is a mapping from node identifier to `seq`, containing no more than
one entry per node.

An entry's `deps` field is a version vector: a snapshot of the author's own
local version vector at the moment of authorship, taken before the entry is
appended to the author's own log, and therefore self-inclusive of every
prior entry the author itself had already applied — including, where the
entry is not the author's first, the author's own immediately preceding
entry.

**Delivery rule.** An entry `e` MUST NOT be folded into a receiver's derived
state until every component of `e.deps` is less than or equal to the
receiver's own current version vector — that is, until the receiver has
already applied every entry `e`'s author had itself already applied at the
time it authored `e`. This is invariant I2 (§6.2).

**The version vector is advanced only by local verification.** A version
vector MUST be advanced only as the direct result of verifying and applying
an entry locally. It MUST NOT be advanced by copying, merging, or otherwise
absorbing a peer's self-reported version vector. A vector received from a
peer is read-only input to computing what is missing (a gap), never a value
assigned into a node's own vector. **A conforming implementation MUST NOT
provide any operation that merges two version vectors into a receiver's own
tracked vector.**

### 4.4 `LogBundle`

A `LogBundle` is the only form of evidence `V` accepts about what happened:
raw signed entries, and nothing derived from them. A bundle MUST NOT carry
any pre-folded state (a claim set, a spend total, a version vector) — `V`
exists to compute those things itself from the raw entries, and accepting
them as input would let a caller assume the very answer `V` is asked for.

A bundle is organized by **observer**, not only by author: each observer's
entry names the set of chains that observer holds a copy of, one chain per
author. This shape is required, not incidental — invariant I3 (§6.3) is a
statement about what two different observers each derived from what they
hold, and a bundle that recorded only one chain per author, with no notion
of which observer held which copy, could not express that comparison at
all.

A view missing for a given observer is normal, not an error — a node may
have crashed, been captured, or simply gone silent. `V` MUST treat a missing
view as a reason an invariant is `Undetermined` (§4.6) for that observer,
never as either a violation or a clean pass.

**Merging.** Combining two bundles' views into one MUST prefer, for any
`(observer, author)` pair present in both, the longer of the two chains —
two honest exports of the same chain can only differ in how much of it had
been seen at export time, so the longer is always a superset. An actual
conflict at that pair (two different entries at the same `seq`) is not
something a merge operation resolves; it is exactly what invariant I1 exists
to catch, downstream, inside `V` itself.

### 4.5 `Spec`

A `Spec` is the set of rules a bundle is checked against: the mission
identifier and epoch a bundle's entries are expected to carry, the roster
(node identifiers and their verifying keys), a spending budget per node, and
a capacity bound on how long any one node's chain may be.

A `Spec` in this phase of the project is not signed. `V` assumes the `Spec`
it is given is the correct one for the mission; it does not authenticate the
`Spec` itself. This is a stated limit on what a verdict means, not a silent
assumption — it is part of why `input_attestable` (§4.6, §2.3) exists as an
explicit field rather than an implied always-true condition.

The capacity bound in a `Spec` MUST be enforced by `V`: a chain longer than
the bound is reported as malformed evidence (§4.6, `ChainFinding`), the same
as a chain that fails verification for any other reason.

### 4.6 `Verdict` and `Witness`

`V`'s output is a `Verdict`, containing:

- One result per invariant I1 through I4 (§6.1–§6.4). Each result is one of:
  **Satisfied** (checked, no violation found), **Violated** (a `Witness` is
  attached — see below), or **Undetermined** (there was not enough evidence
  in the bundle to check this invariant at all; a named reason accompanies
  this result). `V` MUST NOT report `Satisfied` on evidence it did not
  actually check — for example, an invariant that needs two comparable
  observers and finds none MUST be `Undetermined`, never `Satisfied`.
- A structural note, stating that invariants I5 and I6 (§6.5, §6.6) are not
  checked from a bundle at all — they are properties the implementation
  holds by construction, outside what any bundle-level check can observe.
- A list of chain-level findings, entirely separate from I1–I4: a chain that
  fails verification (§4.2), exceeds the capacity bound (§4.5), or is filed
  under a node identifier that does not match its actual signer, is
  malformed evidence, not evidence for or against a specific invariant.
  Reporting it as a chain finding, rather than folding it silently into one
  of the four invariant results, keeps a verifier from either hiding a
  malformed chain or mis-attributing it.
- `input_attestable`: a boolean, always `false` in this phase (§2.3, §9).

**The witness rule.** Every `Violated` result MUST carry a witness
consisting of the minimal set of raw signed entries sufficient to
demonstrate the violation on their own — never a summary string, a hash, or
any other derived value. A reader MUST be able to check a witness
independently, against the roster's public keys alone, without running any
code from this project.

The witness for each invariant:

| Invariant | Witness carries |
|---|---|
| I1 (equivocation) | The two conflicting signed entries themselves. |
| I2 (unmet dependency) | The observer, the entry that could not be reached, and the missing `(node, seq)` component of its `deps`. |
| I3 (divergence) | The two disagreeing observers, the task, and each observer's derived winning claim. |
| I4 (overspend) | The node, its budget, and every one of its `Spend` entries the sum was computed from. |

A chain-level finding carries the observer, the author, the specific
problem found, and the raw entries in question.

---

## 5. Encoding

### 5.1 Canonical encoding rules

Every value defined by this specification has **exactly one** valid byte
encoding. An implementation MUST write each encoding explicitly, field by
field, and MUST NOT delegate the encoding to a general-purpose
serialization library — such a library's output can change with its
version, its build configuration, or field-declaration order, none of which
this specification's byte-exact guarantees can survive.

Integers are big-endian and fixed-width throughout, so that lexicographic
byte order matches numeric order.

**Canonicity MUST be enforced on decode, not only on encode.** If two
distinct byte strings could decode to the same value, an attacker could
manufacture a second, differently-encoded value that decodes identically to
one already held — which, for an entry, means fabricating a proof of
equivocation against an innocent signer, or hiding a real one behind a
non-canonical encoding a decoder silently accepts. Concretely: a version
vector's components (§4.3) MUST decode only from a strictly ascending
sequence of node identifiers; an encoding with a repeated or
descending-order identifier MUST be rejected, not corrected or accepted.

### 5.2 Domain separation tags

A domain separation tag is a fixed byte string prefixed to a value before
it is signed or encoded, so that a signature or encoding valid in one
context cannot be misread as valid in another.

| Tag | Bytes | Used for |
|---|---|---|
| `SWARM_ENTRY_V1` | 14 | An entry's signing bytes (§5.3). |
| `SWARM_BUNDLE_V1` | 15 | A `LogBundle`'s canonical encoding. |
| `SWARM_SPEC_V1` | 13 | A `Spec`'s canonical encoding. |

### 5.3 The signing bytes layout

**This layout is a red line: it MUST NOT change without a major version
increase (§1.2), and no document in this project may describe it
inconsistently.**

An entry's signing bytes are:

```
b"SWARM_ENTRY_V1"                  (14 bytes, domain separation tag)
|| mission_id                      (32 bytes)
|| epoch                           (4 bytes, u32 big-endian)
|| node                            (1 byte, u8)
|| seq                             (8 bytes, u64 big-endian)
|| prev                            (32 bytes)
|| deps                            (version vector encoding, below)
|| body                            (body encoding, below)
```

**Version vector encoding:**

```
count                              (2 bytes, u16 big-endian)
|| (node: u8, seq: u64 big-endian) * count, strictly ascending by node
```

An empty version vector encodes as the two zero bytes `0000`.

**Body encoding:**

```
variant tag                        (1 byte)
|| variant fields
```

| Variant | Tag | Fields |
|---|---|---|
| `TaskClaim` | `0x00` | `task` (8 bytes, u64 BE) `\|\|` `priority` (1 byte, u8) |
| `Withdraw` | `0x01` | `task` (8 bytes, u64 BE) |
| `Spend` | `0x02` | `amount` (8 bytes, u64 BE) |

**Full entry encoding:** signing bytes, followed by the 64-byte Ed25519
signature over those signing bytes. This is the encoding a hash chain link
(§4.2) hashes, and the encoding the golden-vector test corpus pins byte for
byte.

### 5.4 The round-trip requirement

For every value this specification defines, decoding a value's canonical
encoding MUST reproduce the original value exactly, and re-encoding that
decoded value MUST reproduce the original bytes exactly. A conforming
implementation's encoder and decoder for every value in this specification
MUST be checked against the shared test-vector corpus (§8), not only
against each other — two implementations agreeing with themselves proves
nothing about whether they agree with a third.

---

## 6. Invariants

Each invariant below states its exact claim, the check that establishes it,
and the witness a violation produces. An invariant marked **structural** is
held by construction rather than checked from a `LogBundle`; this
specification does not require, and the reference implementation does not
provide, a bundle-level check for a structural invariant.

### 6.1 I1 — At most one signed entry per `(node, seq)`

**Claim.** No two differently-encoded, validly signed entries exist for the
same `(node, seq)` pair.

**Check.** Locally, chain verification (§4.2, rule 5) rejects a duplicate
`seq` outright. Across a bundle, `V` MUST group every chain-verified entry
by its signer and `seq` — not by the map key it happened to be filed under
(§4.4) — and, for any key with more than one distinct encoding present,
independently confirm the conflict using nothing but the two entries and
the roster's public keys, before reporting a violation.

**Witness.** The two conflicting signed entries (§4.6), stored as an
ordered pair sorted by ascending full encoding (§5.3) regardless of which
one a given observer saw first — so that two observers, each first holding
a different one of the pair, construct byte-identical witness pairs once
each has both.

### 6.2 I2 — An entry is not applied before its dependencies are delivered

**Claim.** An entry is never folded into any derived state before every
entry named in its `deps` has itself already been applied (§4.3).

**Check.** Structurally, a node's own delivery logic enforces this at apply
time. From a `LogBundle`, `V` MUST establish this by an independent,
fixed-point causal replay over the raw entries — repeatedly applying any
entry whose `deps` are now satisfied until no more entries can be applied —
and MUST be able to name the first entry, if any, that a given observer's
held view cannot causally reach.

**Known limitation.** A weaker, in-process check that inspects only the
*final* version vector at the end of a run can show that a dependency
eventually arrived; by the end of a long run that vector has grown to cover
nearly everything, so this weaker form cannot by itself distinguish "the
entry was applied late" from "the entry was applied on time." The
fixed-point replay this specification requires is the actual check for the
temporal property the claim above states; a reader should not treat a
final-state-only check as equivalent evidence for I2 that it would be for
the other invariants in this section.

**Partial compensation.** The property is enforced structurally by delivery
logic, exercised directly by unit tests built around out-of-order delivery,
and checked for real by the independent fixed-point replay above, which is
temporal rather than final-state-only.

**Witness.** The observer, the entry that could not be reached, and the
first missing `(node, seq)` component of its `deps` (§4.6).

### 6.3 I3 — Two observers who applied the same entries derive the same state

**Claim.** If two observers have each applied the identical set of
`(author, seq)`-keyed entries, they compute the same winning claim for every
task either of them holds a claim for.

**Folding rule.** A `TaskClaim` entry `e` for task `t` MUST be folded into
that task's claim set as a tuple `(priority, lc, node, seq)`, where
`priority` and `node`/`seq` are read directly from `e`, and `lc` — the
entry's logical clock — is **derived**, not read from any field:

```
lc(e) = Σ over (n, s) in e.deps of (s + 1)
```

That is, `lc(e)` is the count of entries `e`'s author had itself already
applied at the moment it authored `e` (a version vector counts from
`seq = 0`, so a component `(n, s)` represents `s + 1` entries from `n`).
`lc` MUST be computed this way, from the already-signed `deps` field, so
that it cannot be set independently of the signature and needs no wire
format of its own.

A `Withdraw` entry for task `t` from node `n` MUST be recorded as a fact
(available as evidence, and required to disambiguate a `Withdraw` body from
a `TaskClaim` body when both are signed by the same node for the same task
— see §5.3) but MUST NOT remove `n`'s existing claim, if any, from `t`'s
claim set, and MUST NOT otherwise change the result of the winner rule
below. The claim set for a task only ever grows.

**Winner rule.** For a task `t` with a non-empty claim set, the winner is
the tuple that sorts lowest by `(priority, lc, node, seq)` in that field
order — ascending numeric order on each field, `node`/`seq` present purely
to make the ordering total. A task with an empty claim set has no winner.

**Check.** Structurally, this follows from folding being a set insertion
(commutative, idempotent) and the winner rule being monotone — once an
observer's applied set no longer contains the winning claim for a task, no
later entry can restore it. From a `LogBundle`, `V` MUST compare, for every
pair of observers whose applied `(author, seq)` key-sets are identical, the
winning claim each independently derives for every task either holds a
claim for, using the folding and winner rules above, and report the first
disagreement found.

`V` MUST report `Undetermined` — never `Satisfied` — when fewer than two
observers are present in the bundle, or when no pair of present observers
has an identical applied key-set: reporting `Satisfied` on zero comparable
pairs would claim evidence the bundle does not contain.

**Witness.** The two disagreeing observers, the task, and each one's
derived winning claim (§4.6).

### 6.4 I4 — Spendable rights across all partitions never exceed the authorized total

**Claim.** The sum, across every node, of everything that node has recorded
spending never exceeds the sum of what the `Spec` authorized for that node.

**Check.** Structurally, this holds because each node enforces its own
per-entry spending cap locally, before authoring a `Spend` entry — no peer
can spend another node's budget, and a partitioned node still carries its
own complete spending history. From a `LogBundle`, `V` MUST deduplicate
`Spend` entries by `(author, seq)` across every observer's replay-applied
entries (the same entry may legitimately appear in more than one observer's
view), sum the result per node, and compare each sum against that node's
budget in the `Spec`.

`V` MUST report `Undetermined` only when the bundle has no observers at
all; a bundle with observers but no recorded spending is `Satisfied` —
absence of spending is itself sufficient evidence that no budget was
exceeded, which is not true of I3's need for a comparable pair.

**Witness.** The node, its authorized budget, and every one of its `Spend`
entries the sum was computed from (§4.6).

### 6.5 I5 — No safety-critical effect without a valid certificate in the log — structural

**Claim.** No effect of the `SafetyCritical` consistency class (§3.3) is
ever produced without a valid certificate recorded in the log alongside it.

**Check.** Structural only. This specification requires that the only way
to produce an effect at all is through a single, gated operation, and that
no action of the `SafetyCritical` class can reach that operation without a
certificate of the required type. A conforming implementation MUST make a
missing or wrongly-typed certificate a build-time or equivalent static
error, not a runtime possibility. There is no bundle-level check for this
invariant — nothing in a `LogBundle` alone distinguishes "no certificate
exists" from "the certificate check was skipped"; the guarantee instead
comes from there being no code path that can skip it.

**Witness.** None — `V` does not report a `Witness` for I5; it is named
only in the structural note (§4.6).

### 6.6 I6 — Every effect is traceable to a signed entry chain — structural

**Claim.** Every effect a node produces can be traced back to a signed
entry in that node's own log.

**Check.** Structural only. This specification requires that the single
gated operation of §6.5 always writes its entry to the log before any
effect derived from it can be produced, so that an effect with no
corresponding logged entry cannot occur. As with I5, this is a property of
which code paths exist, not something a `LogBundle` on its own can
demonstrate or refute.

**Witness.** None — as with I5, named only in the structural note.

---

## 7. The verification procedure

### 7.1 Inputs and preconditions

`V` takes exactly two inputs: a `LogBundle` and a `Spec` (§4.4, §4.5). `V`
MUST read nothing else — no network, no clock, no random source, no access
to any process, live or otherwise, beyond the two values it was given.

### 7.2 The ordered checks

**Step 1 — chain verification, per `(observer, author)` pair.** For each
chain a bundle's observer holds for a given author, `V` MUST check, in
order:

1. **Misfiling.** The chain's first entry's `node` field MUST match the
   author identifier the chain is filed under. A chain filed under the
   wrong key MUST be reported as a chain finding (`Misfiled`) and excluded
   from every later step, even if the chain would otherwise be valid or
   would otherwise fail for a different reason too. This check MUST run
   before the next two — a chain that is misfiled and also internally
   invalid is still reported as misfiled, not swallowed by a different
   error. Skipping this check can hide a genuine equivocation: two validly
   signed, conflicting chains from the same signer, one filed under the
   correct key and one filed under a different one, would otherwise never
   be compared against each other.
2. **Chain verification** (§4.2), the seven-point check.
3. **Capacity.** The chain's length MUST NOT exceed the `Spec`'s stated
   capacity bound for that node (§4.5).

A chain that fails any of these three is reported as a chain finding
(§4.6) and contributes no evidence to any of the four invariant checks
below. A chain that passes carries its raw entries forward unchanged.

**Step 2 — causal replay, per observer.** For each observer, `V` MUST
replay that observer's chain-verified entries to the fixed point described
in §4.3 — repeatedly applying any entry whose `deps` are now satisfied,
starting over after every successful application, until one full pass
applies nothing more. This replay MUST be an independent implementation:
it MUST NOT call, share code with, or otherwise depend on whatever
computed derived state on the node that produced the bundle in the first
place. A verifier that reuses the exact logic under test can agree with a
bug in that logic as readily as it agrees with correct behaviour; an
independent reimplementation cannot.

**Step 3 — the four invariant checks (§6.1–§6.4)** run over the
chain-verified, replayed data from Steps 1 and 2.

### 7.3 The construction of the verdict

`V` MUST assemble its output (§4.6) from the results of §7.2: the list of
chain findings from Step 1; one `InvariantResult` per I1–I4 from Step 3; the
fixed structural note for I5/I6 (§6.5, §6.6); and `input_attestable`, always
`false` in this phase (§2.3).

### 7.4 The determinism requirement

`V` MUST be a pure function of its two inputs: the same `LogBundle` and
`Spec`, given to `V` any number of times, MUST always produce the same
`Verdict`. `V`'s implementation MUST NOT read a wall clock, a random source,
or any external state while computing a verdict.

---

## 8. Test vectors

### 8.1 The format of a vector

A test vector is a named scenario, given as a canonically-encoded
`LogBundle` file and a canonically-encoded `Spec` file, together with a
short, separate text file recording the expected result: which of I1–I4 are
`Satisfied`, `Violated` (and with which `Witness` variant), or
`Undetermined`, and whether any chain finding is expected.

To add a vector: construct the scenario's bundle and spec from the same
canonical encoders this specification defines (§5), write both files, and
record the expected `V` output alongside them in the format above.

### 8.2 The positive corpus

`spec/vectors/positive/` holds bundles for which every invariant `V` can
check is `Satisfied` and no chain finding is present.

### 8.3 The negative corpus

`spec/vectors/negative/` holds one scenario per way the checks in §6–§7 can
find a problem: an equivocation (I1), an overspend (I4), a chain that fails
verification, a chain filed under the wrong author, a bundle too sparse to
determine I3, and a bundle that fails to decode at all before `V` ever
runs.

---

## 9. Security considerations

This specification and its reference implementation are not independently
audited. The cryptographic constructions used — Ed25519 for signatures,
BLAKE3 for hashing — are standard, widely reviewed primitives, but their use
here has not itself received an independent security review.

`input_attestable` is always `false` in this phase of the project (§2.3,
§4.6). A `Verdict` with no violations is evidence that a log is internally
consistent with itself and with the `Spec` it was checked against; it is
not evidence that the log is genuine. A bundle can be entirely
self-consistent and still be a fabrication signed by keys nobody
independently vetted.

Canonicity (§5.1) is a security property, not a convenience. A decoder that
accepts more than one byte representation for a single value opens a path
to forging a false equivocation witness, or to hiding a genuine one behind
a non-canonical encoding a lenient decoder lets through.

---

## 10. Non-normative notes

- **The channel model is not part of `V`.** A network simulator's delivery,
  delay, loss, and partition behaviour, and any trace format used to record
  a simulated run, are testing tools used to produce inputs for `V`; they
  are not part of what this specification defines and a conforming
  implementation of `V` has no obligation toward them.
- **Rust identifiers name the current implementation, not the
  specification.** `LogBundle`, `Spec`, `Verdict`, `Witness`,
  `VerifiedEntry`, and similar names describe one realization of this
  document. A second, independent implementation in another language is
  conformant if it satisfies §4 through §7 and passes the corpus in §8 —
  it owes this document's Rust names nothing.
- **Bandwidth is arithmetic, not a measurement.** From the frozen encoding
  in §5.3, an entry's size follows directly from its `deps` size, and is
  not a claim about achieved throughput or latency in any deployment.
- **This document does not repeat the reasoning behind its rules.** Every
  rule above has a corresponding rationale in `DESIGN.md`, cross-referenced
  in `DESIGN.md` itself by the section of this document it explains.
