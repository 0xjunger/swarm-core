# swarm-core — M1 Normative Specification

> `DESIGN.md` (Turkish) is the project's source of truth. `docs/spec.md` is the
> normative specification for M0 and remains binding. This file is the normative
> specification for **M1**: the exact rules M1's code implements, written here
> **before** the code per `DESIGN.md` §11.6.
>
> **Status: M1.** Sections marked *Normative — M1* are binding. M0's normative
> sections in `docs/spec.md` (the sans-I/O boundary, the determinism contract,
> the trace format) remain binding unchanged; M1 adds no network behaviour, so
> none of them are touched.

---

## 1. Scope

Phase 1, milestone M1: `Entry` + Ed25519 signatures + a per-node hash chain,
on a **single node**. There is no network yet (`DESIGN.md` §M1: "Henüz ağ yok,
tek bir node var"). The simulator from M0 is unchanged and its tests remain as
regression guards; the M0 placeholder behaviour (`Payload`, echo) stays in place
until M2, when nodes begin broadcasting entries.

One node produces a sequence of records. Each record is signed, and each
contains the hash of its predecessor. A separate verifier function checks the
chain end to end.

**M1 acceptance** (`DESIGN.md` §M1, "Bitti sayılır"):

1. A chain of **1000 entries** is produced and verified end to end.
2. When a single byte of a record in the **middle** of the chain is altered by
   hand, verification **fails**. This test is mandatory — it is the only
   concrete evidence of the tamper-resistance claim.

---

## 2. The `Entry`

*Normative — M1.* Verbatim field list from `DESIGN.md` §3:

| Field | Type | Meaning at M1 |
|---|---|---|
| `mission_id` | `[u8; 32]` | Roster Merkle root in the full design; a **fixed constant** in Phase 1 (`DESIGN.md`, "Alanları bugünden aç, doldurmayı ertele"). Prevents cross-mission replay once real values arrive. |
| `epoch` | `u32` | Roster version. Fixed at `0` in Phase 1. |
| `node` | `NodeId` | The author. |
| `seq` | `u64` | This node's monotonic log index. Starts at **0**; each successor is exactly `+1`. |
| `prev` | `Hash` (32 bytes) | BLAKE3 of the predecessor's full canonical encoding; `[0u8; 32]` for the genesis entry. |
| `deps` | `VersionVector` | Causal dependencies. **Empty at M1** — there is nothing to depend on before there is a network. The field exists now; M2 fills it. |
| `body` | `Body` | The record's meaning. Single variant at M1 (§5). |
| `sig` | Ed25519 `Signature` (64 bytes) | Over the canonical signing bytes (§4). |

Fields are opened now and filled later, deliberately: adding a field later
would invalidate every signature produced so far and break every test fixture
(`DESIGN.md`, item 1).

---

## 3. Canonical encoding

*Normative — M1.*

There is exactly one byte encoding of an `Entry`. It is written explicitly,
field by field — **never serde**, whose output may change with library
version, field order, or compiler settings (`DESIGN.md`, item 2).

Integers are **big-endian** and fixed-width, so lexicographic order matches
numeric order, mirroring the M0 trace rules (`docs/spec.md` §7).

### 3.1 Signing bytes

```
b"SWARM_ENTRY_V1"                  (14 bytes, domain separation tag)
|| mission_id                      (32 bytes)
|| epoch                           (4 bytes, u32 BE)
|| node                            (1 byte, u8)
|| seq                             (8 bytes, u64 BE)
|| prev                            (32 bytes)
|| deps                            (§3.2)
|| body                            (§3.3)
```

The leading tag is the domain separation label (`DESIGN.md` §7): a signature
valid in this context must not be reusable in any future context (certificate
signatures, cross-signings) without an explicit new tag.

### 3.2 `VersionVector` encoding

```
count                              (2 bytes, u16 BE)
|| (node u8 || seq u64 BE) * count, ascending by NodeId
```

Empty at M1: the two count bytes are `0000`. Ascending-by-`NodeId` order is
rule R4 (`docs/spec.md` §6) and comes for free from `BTreeMap` iteration —
`HashMap` is unreachable in this crate, and `NodeId` deliberately does not
derive `Hash` (`docs/spec.md` §3.1.1).

### 3.3 `Body` encoding

```
variant tag                        (1 byte)
|| variant fields
```

| Variant | Tag | Fields |
|---|---|---|
| `TaskClaim` | `0x00` | `task` (8 bytes, u64 BE) `||` `priority` (1 byte, u8) |

### 3.4 Full encoding

```
signing bytes (§3.1) || sig        (64 bytes)
```

The full encoding is what the hash chain hashes (§4.3) and what the golden
vector test pins (§7).

---

## 4. Signatures and the hash chain

*Normative — M1.*

### 4.1 Signing

Ed25519 over the signing bytes (§3.1). Keys are **injected**, never generated
inside `swarm-core`: randomness does not enter the crate at all
(`DESIGN.md` §11.1, `docs/spec.md` §3).

### 4.2 Sequence numbers

`seq` starts at **0** for the genesis entry and increases by exactly 1 per
entry. The next `seq` is derived from the chain length, so a `seq` can never
be reused: crash monotonicity (`DESIGN.md` §4.3) holds structurally at M1.
The fsync / secure-element concern applies to persistent nodes and arrives
with real I/O (Phase 2); there is no crash to survive inside a pure state
machine.

### 4.3 Chain links

- Genesis: `prev = [0u8; 32]`.
- Successor: `prev = BLAKE3(predecessor's full canonical encoding)` — the
  encoding **including the predecessor's signature**. Chaining over the
  signature means tampering with a signature, not only with a body, breaks
  every following link.

### 4.4 Verification

`verify_chain(roster, entries)` checks, for each entry in order, and fails at
the first violation, reporting the offending index:

1. `node` is present in the roster (membership).
2. `node` equals the first entry's node (a chain belongs to exactly one node).
3. `mission_id` equals the roster's (`cross-mission replay` rejected).
4. `epoch` equals the roster's.
5. `seq` equals the expected value (0, then +1). This is invariant I1 at M1:
   a duplicated `(node, seq)` can never pass.
6. `prev` equals the expected link (§4.3).
7. The Ed25519 signature verifies over the signing bytes (strict
   verification) against the roster key of `node`.

An entry that has passed these checks is handed back as `VerifiedEntry`.

### 4.5 `Entry` vs `VerifiedEntry`

*Normative — M1.* `Entry` is untrusted bytes from the outside world.
`VerifiedEntry(Entry)` is what verification produces; its constructor is not
public, so any function that must only see verified entries declares that in
its signature, and forgetting to verify becomes a **compile error** rather
than a runtime bug (`DESIGN.md`, item 4). At M1 no state function consumes
entries yet; the type gate is established anyway, because M2 will.

---

## 5. `Body`: one variant

*Normative — M1.* `Body` is an enum with exactly one variant, `TaskClaim`,
carrying `task: u64` and `priority: u8`. `priority` is opened now because M3's
deterministic winner rule is `min by (priority, logical_clock, node_id)`
(`DESIGN.md` §4.2); adding it later would change the wire format. No further
variant is added before a test demands it (`DESIGN.md` §11.4), and every
variant comes with a test — the golden vector (§7) covers `TaskClaim`.

---

## 6. The log and its bound

*Normative — M1.*

`Log` is the per-node hash chain: it appends, signs, and links entries. It is
bounded, per `DESIGN.md` §7 ("log — üçü de sınırlı"). The capacity is stated
at construction.

**Overflow policy: fail loudly.** Appending to a full log is an error
(`LogError::Full`); the log neither grows silently nor evicts. Eviction is
only safe once the MMR exists (`DESIGN.md` §4.3 makes the MMR the proof path
precisely so old entries can be pruned without losing provability), and the
MMR is not part of M1. Until then, dropping history would make end-to-end
verification impossible, so the bound is enforced by refusal instead.

---

## 7. Golden vector

*Normative — M1.* A test file pins, in hex, the signing bytes, the full
encoding, and the signature of one known `Entry` under one known key. Any
change to the wire format breaks this test — and **that is the point**: the
format must never change silently (`DESIGN.md`, item 5). If a change is
deliberate, the golden vector is updated and the reason is stated in the
commit message (`DESIGN.md` §11.5).

---

## 8. Invariants at M1

*Normative — M1.* Per `DESIGN.md` §11.7 the invariants are written before the
code they guard. Of I1–I6, only I1 is testable at M1 — single node, no
network, no CRDT, no escrow:

| # | Invariant | Status at M1 |
|---|---|---|
| **I1** | At most one signed entry per `(node, seq)` | **Binding.** Enforced by construction (`seq` = chain length) and by verification (rule 4.4.5 rejects duplicates). Tested in `tests/invariants.rs`. |
| I2 | An entry is not applied before its `deps` are delivered | Documented placeholder — activates at M2 (causal delivery). |
| I3 | Two nodes that have seen the same entry set derive the same state | Documented placeholder — activates at M2 (partition-heal convergence). |
| I4 | Spendable rights across all partitions ≤ authorised total | Documented placeholder — activates at M5 (escrow). |
| I5 | No safety-critical effect without a valid certificate in the log | Documented placeholder — activates with the policy gate (M5). |
| I6 | Every effect is traceable to a signed entry chain | Documented placeholder — activates when `step` derives effects from entries (M2+). |

---

## 9. Dependencies

*Normative — M1.* `swarm-core` gains its first dependencies, exactly as
`docs/spec.md` §8 anticipated: `blake3` and `ed25519-dalek`, both with
`default-features = false` (plus `ed25519-dalek`'s `alloc` feature) so the
crate remains `no_std` and the thumbv7em cross-compile in `docs/spec.md` §3
keeps proving it. No other dependency is added; `serde` stays out (§3).

---

## 10. Changelog

| Milestone | Change |
|---|---|
| M1 | Entry, canonical encoding with domain separation, Ed25519 signatures, per-node hash chain, end-to-end verifier with the `VerifiedEntry` type gate, bounded log, golden vector, I1. |
