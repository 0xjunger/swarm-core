# Swarm Core — Design

## Purpose

This document records **why** the design is what it is: the problem it
answers, the alternatives it rejected, and the trade-offs it accepted.
`SPEC.md` records **what** the rules are. The two documents do not repeat
each other.

`DESIGN.md` is **non-normative**. If this document and `SPEC.md` ever
disagree, `SPEC.md` has priority, and the disagreement is a defect — report
it rather than trusting whichever document you read first.

Two categories of content that used to live in this file have moved to a
dedicated home rather than being dropped:

- The project's working glossary is now `docs/GLOSSARY.md`.
- The rules for working on this codebase (no `std::net` in `swarm-core`,
  write the invariant before the code, and so on) are now in
  `CONTRIBUTING.md`.

Each record below is numbered and links to the section of `SPEC.md` it
explains.

---

## D-001: Accountability, not prevention

**Status:** Accepted

### Context

A swarm operating under contested or unreliable communications needs two
distinct guarantees, on two separate axes: **consensus** — did the group
agree on something — and **provability** — was what was agreed upon
actually authorized. A protocol that only guarantees the first can still let
a protocol-compliant but compromised node produce an unauthorized decision
that the rest of the group accepts by agreement alone.

The motivating case for this project is a drone swarm operating under
jamming: a swarm dependent on a central command-and-control node goes blind
the moment that link is degraded, and swarms built to resist jamming tend to
be locked inside a single manufacturer's closed stack. A civil-use framing
of the same core problem exists too: the "adversary" is not a jammer but a
second party with conflicting interests — a competing operator sharing a
logistics space, a manufacturer that would rather not carry liability, or a
regulator that wants proof of compliance without being handed raw
telemetry. In the civil case, the distributed-systems layer is the primary
driver and the verification layer is optional; in the defense case, that
priority reverses.

### Decision

Classic Byzantine fault tolerance (quorum protocols in the style of
Tendermint or HotStuff) is the wrong primitive here. Such protocols require
a `2f+1` quorum to make progress; when the network partitions, the minority
side simply stops. For a physical drone, stopping is not a safe pause — it
is a lost mission.

The actual requirement runs the other way: under a partition, every node
keeps acting, and safety comes not from preventing an unauthorized action
but from making every action **provably accountable** after the fact. This
is the project's central design stance: *act optimistically, prove
accountably* — an optimistic-rollup-style argument applied to a physical
swarm instead of a ledger.

### Alternatives that the project rejected

- **Classic quorum-based BFT consensus.** Rejected because a
  partition-induced halt is unacceptable for a physical vehicle — a stopped
  drone is a lost mission, not a safely paused one.

### Consequences

This choice makes continued operation under partition possible, at the cost
of an entirely different safety argument: instead of a consensus proof that
an action was authorized before it happened, the project provides a
post-hoc proof that a violation, if one occurred, is checkable afterward.
One direct consequence is that silence itself becomes ambiguous — see
D-006, which is an open gap this decision creates rather than closes.

**See:** `SPEC.md` §3.3 (consistency classes), §6 (invariants).

---

## D-002: The sans-I/O boundary

**Status:** Accepted

### Context

Three goals compete against a conventional networked implementation: the
ability to run deterministic simulations at scale (thousands of seeds,
looking for a rare invariant violation), the ability to replay a run
byte-for-byte after the fact, and the ability, later, to retarget the same
decision logic onto a hardware-attested binary or a zero-knowledge circuit
without rewriting it (D-011). All three need the core decision logic to be
free of network access, wall-clock access, and randomness.

### Decision

`swarm-core`'s only entry point is a pure function that takes a state, an
event, and a logical time, and returns a new state and a list of effects.
Nothing inside the crate reaches the network, a clock, or a randomness
source; all three are always supplied from outside as parameters.

This is enforced three separate ways rather than by convention alone:

1. The crate is built without the standard library, which makes
   `std::net`, `std::time`, and the standard hash-based collections
   unreachable from inside it at all.
2. A lint configuration disallows the equivalent standard-library types and
   methods (hash-based collections, `SystemTime`, `Instant`,
   `rand::thread_rng`, `std::thread::sleep`) in every other crate in the
   workspace, where the standard library *is* available.
3. The crate is cross-compiled to a bare-metal embedded target as part of
   the project's verification gate, so the no-standard-library claim is
   proven by a build that either succeeds or fails, not merely asserted in
   a comment.

**Why no wall clock enters the system anywhere, at any layer, ever.** A
tie-break rule that depended on wall-clock time would let an attacker win a
contested claim simply by spoofing GPS time, since GPS is the only real
time source available to a fielded node and it is not trustworthy against a
capable adversary. Every tie-break in this project is instead derived from
already-signed data — a logical clock computed from causal history and the
node's own identifier — which cannot be influenced without also forging a
signature.

All hardware-facing and I/O-performing code is kept in separate crates —
today the simulator that drives `swarm-core` in tests; a real network
adapter and a hardware bridge are planned as separate crates for a later
phase and do not exist yet.

**A concrete defense this boundary buys, found by experiment rather than
designed in:** the node identifier type used throughout this project
deliberately does not support being used as a hash-table key. An attempt to
force it into a hash-based collection anyway, to test whether doing so
would reproduce a suspected nondeterminism bug on purpose, did not compile
— the type system rejected the bug before any lint or test was even run.
For the record, the experiment was completed anyway by temporarily adding
hash-key support: two runs of the same program at the same random seed then
produced 916 differing lines of trace output, while every invariant-guard
test continued to pass — proving that the determinism property, not
correctness, was what had broken.

**Why the core function is pure (returns a new state rather than mutating
one in place):** this costs a full copy of the state on every event, which
is measurable but currently irrelevant at the project's current scale. It
is kept because it is exactly the shape a folding-based proof scheme needs
later (each step being a pure function of the previous state and one
event) — the claim that a later, proof-carrying phase of this project
"comes for free" rests specifically on the same function being retargeted
to a different substrate, not rewritten (D-011).

### Alternatives that the project rejected

- **An implementation built on real sockets, real threads, and real wall
  time from the start.** Rejected as a first-day, effectively irreversible
  decision: if the simulator used to test this project is written after the
  protocol rather than before it, the protocol tends to get built directly
  on real sockets and real timers, and cannot be made deterministic
  afterward without a rewrite.
- **Adopting an existing deterministic-simulation testing library.**
  Considered and rejected because the available options are built on an
  async runtime and would force the entire project into an async style;
  the actual simulation loop this project needs is a small, plain, seeded
  loop, which is simpler, faster, and fully deterministic without that
  dependency.

### Consequences

The pure-state-return signature costs a clone of the state per event
(quadratic over a full run). This is accepted as irrelevant at the current
scale; if it ever stops being irrelevant, an internal mutable
implementation can be hidden behind the same public, pure-looking signature
without changing anything that depends on it.

**See:** `SPEC.md` §7.4 (the determinism requirement), §10 (non-normative
note on the simulator).

---

## D-003: The `no_std` constraint and the two build targets

**Status:** Accepted

### Context

The project intends to run this same decision logic on constrained
embedded hardware eventually, and wants the "this crate has no hidden
standard-library dependency" claim to be something a build either proves or
disproves, not something a reader has to take on trust from a comment.

### Decision

The core crate is built without the standard library. As part of the
project's verification gate, it is cross-compiled to a bare-metal ARM
target, so the claim is checked by an actual build on every run of that
gate, not merely stated. Every collection used inside the crate is an
ordered tree-based structure rather than a hash-based one, both because
hash-based collections are unavailable without the standard library and
because their absence is exactly what D-002's determinism argument depends
on. Every dependency the core crate takes on must itself support this
build mode.

### Alternatives that the project rejected

- **Keeping the core crate as an ordinary standard-library crate and
  relying on code review to catch a violation of the I/O ban.** Rejected
  because a discipline-only rule erodes silently over time; a build that
  can fail, and a lint that can flag a violation immediately, catch the
  problem the moment it is introduced instead of relying on a reviewer
  noticing it.

### Consequences

Every dependency choice for the core crate is constrained by this
requirement, which has already ruled out convenient general-purpose
libraries (a serialization framework, in particular — see D-008) in favor
of hand-written equivalents.

**See:** `SPEC.md` §5.1 (canonical encoding, written by hand rather than
through a general-purpose library, for the same underlying reason).

---

## D-004: Why `VersionVector::merge()` does not exist

**Status:** Accepted

### Context

A causal-broadcast system's record of "what have I seen from each peer"
can, in principle, be advanced in one of two ways: by locally verifying and
applying an entry, or by trusting and absorbing a peer's own self-reported
claim about what it has seen. Only the first of these preserves the
property that a node's recorded knowledge is exactly what it has actually,
independently verified — the second lets a lying peer inflate what a node
believes it has already checked.

### Decision

There is no merge operation on this project's version-vector type anywhere
in the codebase, and none should ever be added. A node's version vector
advances only as the direct, structural result of verifying and applying an
entry locally. A peer's self-reported vector, received during periodic
resynchronization, is read-only input used only to compute which specific
entries are missing; it is never written into a node's own tracked vector.

### Alternatives that the project rejected

- **A `merge()` convenience function that takes the componentwise maximum
  of two vectors.** Rejected outright, not merely left unwritten: even
  sitting unused in the codebase, such a function would be a standing
  invitation to violate the "advance only by local verification" rule the
  first time someone reached for a shortcut while extending the
  resynchronization protocol. This is the actual mechanism that keeps the
  causal-delivery invariant true even against a peer that lies about what
  it has seen — not a style preference.

### Consequences

The periodic resynchronization protocol must be phrased entirely as "read
what the peer claims to have, compute what I am missing relative to what I
hold, send or request exactly those entries" — never as "combine the two
vectors." Every future extension to this protocol inherits the same
constraint.

**See:** `SPEC.md` §4.3 (the version vector; "advanced only by local
verification" is stated there as a MUST, and a conforming implementation is
explicitly required not to provide a merge operation at all).

---

## D-005: The consistency classes

**Status:** Accepted

### Context

Not every action a swarm takes needs the same safety treatment. A local
formation adjustment can tolerate temporary disagreement. A shared,
exhaustible resource needs a bounded allocation that holds even under
partition. An action with real consequences — authorizing an engagement, for
instance — needs a positive, checkable grant of authority before it can
happen at all, and needs that requirement to be impossible to bypass by
oversight.

A pure conflict-free replicated data structure can give convergence — two
nodes that see the same entries eventually agree — but it cannot, by
itself, give mutual exclusion. If two nodes claim the same task, both may
act on that claim for a while before one of them withdraws once it learns
it lost; the system is temporarily unsafe and only eventually consistent.
That is an acceptable behaviour for a locally-decided action and is not
acceptable for a shared, exhaustible resource.

### Decision

Every action belongs to exactly one of three consistency classes, and the
distinction is enforced by the type system rather than by a runtime check:

- **Degradable** — a locally decided action (formation, sensor tasking,
  relay assignment). Convergence is the only guarantee needed, and a pure
  replicated data structure provides it.
- **ExclusiveCostly** — an action drawn from a shared, bounded resource.
  This needs a certificate scoped to the current partition, obtained in one
  round trip with no leader election — see `SPEC.md` §6.4 (I4) for how the
  actual bound is enforced.
- **SafetyCritical** — an action that must not occur without an explicit
  grant of authority (an operator's own signature, or a signature
  threshold reached across the roster). If the required certificate cannot
  be assembled, the action does not happen. A request for this class of
  authority that is refused is itself written to the log — "I asked for
  authority and did not receive it, and here is the proof" is treated as
  valuable a record as the action itself would have been.

The single function capable of producing an effect at all requires a
certificate of the type appropriate to the action's class; an action
belonging to the `SafetyCritical` class simply has no way to compile into
an effect-producing call without one.

**Why the roster is fixed for the scope of a mission.** Supporting dynamic
membership — a node joining or leaving mid-mission — is a reconfiguration
protocol in its own right, and a hard one. Starting instead from a
mission-scoped, fixed roster with an operator-signed version number removes
most of that complexity while still leaving room to add reconfiguration
later without breaking the wire format (the roster version is already a
signed field on every entry).

A shared, exhaustible resource that needs true redistribution under
partition — a node handing part of its own allocation to another node
mid-mission — is out of this phase's scope entirely: the current mechanism
gives each node a fixed allocation it spends without asking anyone, which
is sufficient to keep the global total bounded, but a two-round handshake
would be needed to let nodes actually trade allocation with each other, and
that handshake does not exist yet.

### Alternatives that the project rejected

- **A runtime permission check performed before each action.** Rejected in
  favor of a compile-time gate: an action of the `SafetyCritical` class
  that has no way to supply the required certificate type has no way to
  compile into code that produces an effect at all, which turns "forgot to
  check permission" from a possible runtime bug into an impossible program.
- **Treating a task-claim conflict as something a data structure alone
  must fully resolve (with tombstone-based removal on withdrawal).**
  Rejected for the current phase because a tombstone requires
  garbage-collection based on causal stability, which is a real design
  problem this project has not yet solved; adding a tombstone mechanism
  without solving that problem first would let internal state grow without
  bound. Withdrawal is recorded as a log fact instead, without removing
  anything from the underlying claim set — see `SPEC.md` §6.3.

### Consequences

A safety-critical capability cannot be added to this system by accident —
it requires deliberately implementing the class and its certificate type,
not merely calling an existing function with different arguments. The
project also cannot currently offer true budget redistribution between
nodes; only a fixed, locally-enforced per-node allocation.

**See:** `SPEC.md` §3.3 (consistency classes), §6.4 (I4), §6.5 (I5).

---

## D-006: Silence is architecturally ambiguous — an open gap

**Status:** Accepted (as a stated limitation, not a solved problem)

### Context

Jamming, physical destruction, and capture of a node all produce the
identical observable signature from outside: the node stops producing
entries. A detection layer built purely from observing traffic patterns
cannot, by itself, tell these three causes apart.

### Decision

**This project does not currently implement a general resolution to this
ambiguity, and no such mechanism — a lease, a timeout, or any other
expiring grant of authority — exists anywhere in the codebase today.** The
one related problem the project does solve is narrower: when a node
equivocates (signs two different, conflicting entries at the same log
position), that specific violation becomes provable, but only once both
conflicting entries happen to reach a common observer (D-007). A node that
simply goes silent — for any of the three reasons above — is not
distinguished by anything in this system; a verifier reading a log with a
missing observer reports the affected checks as `Undetermined`, which is an
honest statement of insufficient evidence, not a resolution of which of the
three causes applies.

### Alternatives that the project rejected

The project has no record of a rejected alternative for this decision —
this is a stated, open gap, not a choice made between implemented options.

### Consequences

A verifier cannot currently distinguish "this node was jammed," "this node
was destroyed," and "this node was captured and is now silent by an
adversary's choice." Closing this gap — for example with an
authority mechanism that must be periodically renewed and lapses if it is
not — remains unimplemented future work, not a design decision this project
has already made.

**See:** `SPEC.md` §4.4 (a missing observer view is normal, not an error),
§6.3 (I3's `Undetermined` result).

---

## D-007: Equivocation, and not economic slashing

**Status:** Accepted

### Context

Many distributed-ledger systems answer "how is a misbehaving participant
held accountable" with an economic penalty against a staked bond. A drone
swarm has no meaningful stake to put at risk and no market that would make
slashing a real deterrent rather than a symbolic gesture.

### Decision

Accountability in this project is forensic and operational, not economic.
Two differently-encoded, validly signed entries at the same log position
are, by themselves, complete proof that a specific node broke the
one-entry-per-position rule — no quorum, no third-party attestation, and no
consensus round is needed to establish this. A third party holding only the
roster's public keys, who never ran the mission and never exchanged
anything with whoever raised the accusation, reaches the identical verdict
from the two signatures alone. The set of nodes with a proven violation only
ever grows, and a verified proof needs no further agreement from anyone to
be trusted — which is exactly why excluding a proven-faulty node from
being believed further requires no vote.

### Alternatives that the project rejected

- **An economic slashing mechanism.** Rejected because there is no
  meaningful stake or market context for a drone swarm; a penalty of this
  kind would be symbolic rather than functional.
- **A quorum vote to formally expel a faulty node.** Rejected because it
  reintroduces exactly the consensus dependency D-001 exists to avoid — the
  proof is designed to stand on its own, without a vote confirming it.

### Consequences

A node's accountability for equivocating is entirely evidentiary: an
unforgeable, independently checkable record exists that it broke the rule.
What happens as a result operationally (grounding it, excluding it from a
future mission) is a decision made outside this system, using the proof as
input. This mechanism is post-hoc, not preventive, by design (D-001): a node
whose two conflicting entries never both reach a common observer produces a
violation that goes unproven for as long as that isolation continues — a
real and openly stated limitation, not an oversight.

**See:** `SPEC.md` §6.1 (I1).

---

## D-008: The specification as an interface contract

**Status:** Accepted

### Context

If a log's format, its byte encoding, and the boundary between what counts
as attestable evidence versus debug-only output are not fixed and
explicit, two implementations — or two versions of the same
implementation — can silently disagree about what a signed byte string
means. For a security-relevant proof object, that disagreement is a
correctness failure, not a compatibility inconvenience.

### Decision

The published message a node sends, the record kept in its log, and the
proof object a verifier checks are all the same struct: one canonical
encoding, one signature, serving all three roles at once, rather than three
separately-serialized, separately-signed layers that would triple the
signing cost for no real benefit, since all three uses always travel
together in practice.

**Canonical encoding** means exactly one byte representation exists for any
given value, written out explicitly field by field rather than produced by
a general-purpose serialization library — such a library's output can
change silently with its version or build configuration, which this
project's byte-exact guarantees cannot survive.

**Domain separation** — a fixed tag prefixed to a value before it is
signed — stops a signature that is valid in one context from being
replayed as valid in a different one (a certificate signature being
mistaken for an ordinary entry signature, for instance).

**The hash chain and its proof path.** Each node's log is linked by hashing
each entry's full encoding into the next entry's link field, so tampering
with anything about a previously-signed entry, including only its
signature, breaks every link that follows. Neighboring nodes periodically
cross-sign each other's chain heads, adapted from the same gossip idea
Certificate Transparency uses to stop a log's own operator from rewriting
history unilaterally — a node cannot quietly rewrite its own past once
another node has witnessed and signed its head. An authenticated,
append-only proof structure for the chain (a Merkle Mountain Range) is the
intended long-term proof path, so that old entries can eventually be pruned
without losing the ability to prove what they contained; this structure is
not part of the current phase.

**Crash monotonicity.** If a node crashes and loses its own record of how
far its log had grown, and then reuses a sequence number it had already
used before the crash, it has — entirely by accident — equivocated against
itself and now looks provably faulty by its own record (the same shape of
problem some proof-of-stake systems call "slashing on restart"). The fix is
to make the sequence number durable *before* the corresponding entry is
sent — either by writing it to persistent storage synchronously or by using
a hardware monotonic counter — but this concern only becomes real once a
node performs actual persistent I/O, which is out of the current
input/output-free phase entirely.

The append-only log format itself is treated as an interface contract:
versioned explicitly, with a breaking change (to the signing byte layout, a
domain tag, or a canonical encoding rule) requiring a major version
increase rather than a silent drift. The raw, signed log is kept strictly
separate from any debug-only trace output a simulator might produce — the
former is what a verifier checks and carries security weight; the latter
exists only to make a simulated run readable by a human and carries none.

### Alternatives that the project rejected

- **Serializing the published message, the log record, and the proof
  object as three separately signed structures.** Rejected because it
  triples the signing and verification cost for a "layer independence"
  benefit the project has never needed — all three uses always travel
  together.

### Consequences

Every field that will ever need to be signed had to be present in the
struct's byte layout from the very beginning of the project — adding a
field later would invalidate every previously issued signature and every
existing test fixture. Fields the mission does not yet assign real values
to are opened early with a fixed placeholder value specifically to avoid
this cost later.

**See:** `SPEC.md` §4.1 (entry fields), §4.2 (the hash chain), §5 (encoding,
entirely).

---

## D-009: The external verifier

**Status:** Accepted

### Context

A checker that only ever runs from inside the same process that produced
the data it is checking cannot, by itself, demonstrate that a stranger with
no access to that process would reach the same conclusion — and that
demonstration is the actual claim this project makes.

### Decision

An in-process checker (one that reads live internal state from inside a
running simulation) is kept, but is explicitly not treated as the
project's real, product-facing surface. The real surface is a verifier
that takes only two files — a bundle of raw signed entries and a
specification of the mission's rules — with no access to the process that
produced them, exercised by an actual two-command scenario: one command
runs a scripted mission and writes out two files; a second, separate
command reads only those two files and produces a verdict, with no shared
memory between the two. This two-command scenario, run in both directions —
a bundle containing a known violation must be rejected, and a clean bundle
must be accepted — is the thing this project treats as its actual exit
criterion, not the in-process checker's agreement with itself.

The in-process checker remains useful two ways: as a second opinion to
compare the external verifier's answer against on the same underlying run,
and as the host for a negative-control test that the external verifier
cannot itself serve as a check on its own correctness (since the external
verifier's whole design goal is not to share code with the thing it is
checking — see below).

### Alternatives that the project rejected

- **Treating the in-process checker as sufficient on its own.** Rejected
  because a checker that shares code and process state with the system it
  is checking can agree with a bug in that system as readily as it agrees
  with correct behaviour — it may simply be re-running the same
  computation, not independently confirming it.

### Consequences

The external verifier's causal-delivery replay and its winner-rule
computation had to be written completely from scratch, never calling into
the same folding logic the simulator itself uses to derive its own state —
an independent reimplementation cannot inherit a bug planted in code it
never calls. A deliberately broken build that corrupts the shared folding
logic is expected to fool the in-process checker, which relies on that
logic, and is expected *not* to fool the external verifier — which
demonstrates the independence claim directly rather than merely asserting
it.

**See:** `SPEC.md` §7.1–§7.2 (inputs and the ordered checks, including the
requirement that the causal replay be an independent implementation),
§1.1 (compatibility policy references the exit gate).

---

## D-010: The typed `Witness`

**Status:** Accepted

### Context

A checker whose output names a violation only as a formatted,
human-readable string is not independently checkable — a reader can only
trust that whatever computed the string got it right, which is exactly the
trust this project exists to remove.

### Decision

A violation's evidence is a typed value carrying the minimal raw signed
data a reader needs to check the claim for themselves — for an
equivocation, the two conflicting signatures; for other violations, the
specific entries the finding was computed from. A reader checks this
evidence directly against the roster's public keys, with no code from this
project in the loop, rather than trusting a prose description of what was
found.

### Alternatives that the project rejected

- **A formatted string description of the violation.** This was the
  original shape of the in-process checker's output and is explicitly
  replaced for the external-facing verifier; the string form is retained
  only for the in-process checker's own, lower-stakes output (D-009).

### Consequences

Each invariant needed its own evidence shape, rather than one generic
"violation" type, since the minimal proof differs by invariant (two
signatures for an equivocation; a sum and a list of entries for an
overspend). This is a small amount of extra type complexity in exchange for
every violation being independently re-checkable by a reader who trusts
nothing about this project's own code.

**See:** `SPEC.md` §4.6 (`Verdict` and `Witness`, entirely), §6 (the
"Witness" row under every invariant).

---

## D-011: Substrate independence across the phases

**Status:** Accepted

### Context

This project's stated ambition spans several very different execution
substrates over time: native code today; a hardware-attested binary later;
and, further out, a zero-knowledge circuit. A design that has to be
rewritten for each new substrate makes every later phase expensive and
risky in proportion to how much of the earlier design has to be thrown
away.

### Decision

The verification function's meaning — what counts as a violation, what is
checked, what a verdict means — does not change across substrates; only the
substrate executing it changes.

- **Phase 1** (the current phase) runs the verification function as native
  Rust code.
- **Phase 2** runs the same decision logic inside a hardware-attested
  binary, so its output additionally carries proof that specific, unmodified
  code produced it, without changing what is being checked.
- **Phase 3** recompiles the same logic as a zero-knowledge circuit, so its
  output carries a succinct proof of correct execution without revealing
  the underlying inputs — again, without changing what is being checked.

The pure, allocation-conscious shape of the core function (D-002) is chosen
specifically because it is also the shape a folding-based proof scheme
needs later: the claim that a later phase "comes for free" rests on the
same function being retargeted to a new substrate, not rewritten from
scratch for it.

Two further, more exploratory directions appear in the project's working
notes beyond the three phases above — a folding/incrementally-verifiable
computation scheme for the verifier's replay step, and eventually proving
properties of a perception model itself. Both are recorded here for
completeness, since deleting them would remove real rationale from the
project's history, but neither is a commitment: both are explicitly
speculative in the project's own notes, and neither is referenced by
`SPEC.md` or `README.md`, which describe only the three phases above.

### Alternatives that the project rejected

- **Designing the current phase without regard to later substrates, and
  retrofitting proof-friendliness afterward.** Rejected because retrofitting
  a pure, deterministic shape onto code that was not designed that way from
  the start is close to impossible in practice; the cost of this discipline
  is paid up front specifically so a much higher cost is not paid later.

### Consequences

Some choices that look like unnecessary overhead if judged by the current
phase alone (the state-copying cost of a pure step function; strict
determinism discipline in places where it is not yet load-bearing) are
deliberate investments toward later phases, not premature optimization.

**See:** `SPEC.md` §1.4 (`V` is defined independently of any one substrate),
§10 (non-normative note on implementation names).

---

## D-012: The epistemic ceiling

**Status:** Accepted

### Context

A machine-checkable proof that a log contains no rule violation can easily
be mistaken for a proof that the events the log describes actually
happened as described. These are two different claims, and treating them
as one overstates what this project's verdict actually means.

### Decision

The verification function proves procedural compliance: that a signed log,
taken at face value, does not show a rule being broken. It does not prove
factual truth: nothing in a signed log carries independent evidence that
what a node reported — a sensor reading, a position — was accurate. A node
that consistently and honestly-by-the-log reports a false sensor input
produces a fully valid, fully checkable proof of a wrong decision.

For this reason, a verdict carries two separate dimensions rather than one:
whether a rule violation is present, and whether the input itself is
attestable at all — in this phase, always no, since no attestation
mechanism exists yet. Removing that second dimension because "it is always
false anyway" would itself be exactly the kind of overstatement this
decision exists to prevent: keeping it as an explicit, typed field means no
future caller can begin silently reading an all-clear verdict as "this
mission definitely happened as logged."

### Alternatives that the project rejected

- **A single pass/fail verdict with no separate attestability dimension.**
  Rejected because it would let "no rule violation found" be read as "this
  is what happened" — a claim this project is not in a position to make
  without an actual attestation mechanism, which is out of scope for the
  current phase entirely.

### Consequences

Every verdict, however clean, is presented alongside an explicit statement
that the genuineness of its input was not established. This is a permanent
property of the current phase, not a temporary omission meant to be
quietly dropped once the project matures.

**See:** `SPEC.md` §2.3 (the epistemic ceiling, entirely), §4.6
(`input_attestable`), §9 (security considerations).

---

## D-013: Real-time isolation and the bandwidth-budget-first discipline

**Status:** Accepted

### Context

Two operational constraints recur throughout the project's working notes
without fitting naturally under any of the decisions above: the
coordination layer this project builds must never be able to block a
vehicle's real-time flight-control loop, and every wire-format decision
needs to be checked against the link's actual bandwidth budget before it is
adopted, not after.

### Decision

The coordination layer is designed to run on its own thread with a bounded
message queue, dropping messages rather than blocking under overload; if
the coordination layer itself fails outright, the vehicle is expected to
fall back to safe autonomous behaviour rather than depending on the
coordination layer for basic flight safety. Every field considered for the
wire format is checked, before being adopted, against the arithmetic in
`SPEC.md` §10 (entry size, multiplied by message rate, multiplied by roster
size, against link capacity) — a format decision that would exceed the
link budget is treated as the format being wrong, not as something a later
optimization pass is expected to rescue.

### Alternatives that the project rejected

The project has no record of a rejected alternative for this decision.

### Consequences

Every structure in this system has a stated, enforced bound (a log's
capacity, a buffer's capacity, a network queue's capacity) rather than
being allowed to grow without limit, and the wire format has never been
allowed to grow ad hoc without a bandwidth check first.

**See:** `SPEC.md` §10 (bandwidth is stated as arithmetic derived from the
frozen encoding, not a measurement).
