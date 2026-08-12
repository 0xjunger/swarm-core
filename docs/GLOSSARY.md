# Glossary

Every technical term used outside plain language in this project's
documents, with a plain-language definition. A term's first use in a
document carries a short inline explanation; after that, the term is used
alone.

| Term | Kind | Definition |
|---|---|---|
| Anti-entropy | Technical Name | The process by which two nodes periodically compare what each has seen and close the gap. It catches a lost message eventually, without a separate retry mechanism. |
| Append-only log | Technical Name | A record that only accepts new entries. No entry changes, and no entry is removed. |
| Attestation | Technical Name | A cryptographic statement that specific, unmodified code produced a given output. This project does not implement attestation in its current phase — see `SPEC.md` §2.3. |
| Blake3 | Technical Name | A hash function. This project uses it to link entries into a hash chain and to fingerprint recorded runs. |
| Byzantine fault tolerance (BFT) | Technical Name | A family of consensus protocols that keep working correctly even if some participating nodes are faulty or malicious. Most require a supermajority ("quorum") of nodes to agree before making progress. |
| Canonical encoding | Technical Name | An encoding rule under which one value has exactly one valid byte representation. |
| CBOR | Technical Name | A general-purpose binary data format. This project does not use CBOR — every encoding in `SPEC.md` §5 is written out by hand, deliberately, instead of relying on a general-purpose format (`DESIGN.md` D-008). |
| CRDT (conflict-free replicated data type) | Technical Name | A data structure that lets two copies, updated independently and without coordination, always merge back into the same result once they see the same updates. |
| Domain separation | Technical Name | A fixed tag placed in front of a value before it is signed or encoded, so a signature or encoding valid in one context cannot be mistaken for one valid in a different context. |
| Ed25519 | Technical Name | A digital signature algorithm. Every signed entry in this project is signed with Ed25519. |
| Epoch | Technical Name | A version number for a mission's roster. |
| Equivocation | Technical Name | Signing two different, conflicting entries at the same log position. |
| Escrow | Technical Name | A fixed spending allocation given to a node at the start of a mission, spent without needing to ask any other node. |
| Golden vector | Technical Name | A test that pins the exact expected bytes of a known value's encoding, so an accidental change to the wire format is caught immediately. |
| Hash chain | Technical Name | A sequence of records in which each record contains the hash of the one before it, so that changing an earlier record breaks every link after it. |
| Lease | Technical Name | An authority grant that expires after a stated time or count. **Not implemented in this project** — see `DESIGN.md` D-006, an openly stated design gap, not a feature in use. |
| LogBundle | Technical Name | The input format the verifier accepts: a collection of raw signed entries, organized by which node observed which other node's chain. |
| Logical clock | Technical Name | A counter derived from causal history, rather than from wall-clock time, used to order events consistently without trusting any clock. |
| Merkle Mountain Range (MMR) | Technical Name | An append-only, authenticated data structure that supports proving a record's presence in a growing log without holding the whole log. Named in this project's design as the intended path for pruning old log entries; not yet built. |
| Merkle tree | Technical Name | A tree of hashes in which each parent node is the hash of its children, letting a large data set be summarized by one small root hash. |
| `no_std` | Technical Name | A Rust build mode that excludes the standard library, keeping a crate portable to constrained or embedded environments. |
| Predicate | Technical Name | A rule that classifies an action as permitted or not permitted. A `Spec` (below) is a predicate document. |
| Property-based testing | Technical Name | Testing by stating a rule that must always hold, then having a tool generate many random scenarios and check the rule against each one, rather than writing each scenario by hand. |
| Proof of equivocation (PoE) | Technical Name | The two conflicting signed entries produced by an equivocation, which together are sufficient evidence on their own. |
| Quorum certificate | Technical Name | Proof that enough nodes signed the same statement. |
| Roster | Technical Name | The fixed list of a mission's member nodes and their public keys. |
| Sans-I/O | Technical Name | A design in which the core logic performs no input and no output of its own — no network, no clock, no randomness — with all of those supplied from outside. |
| Spec | Technical Name | The document a `LogBundle` is checked against: a mission's identity, roster, spending budgets, and log-length limit. |
| TEE (trusted execution environment) | Technical Name | A hardware-isolated area of a processor that can produce a cryptographic proof that specific code ran inside it. |
| Verdict | Technical Name | The output of the verification function: which invariants are satisfied, violated, or undetermined, and why. |
| Version vector | Technical Name | A record, for each node, of the highest position in that node's log a party has seen. |
| Witness | Technical Name | The minimal raw evidence attached to a violated invariant, sufficient for a reader to check the violation independently. |
| Zero-knowledge proof | Technical Name | A method for proving that a computation was performed correctly without revealing the inputs to that computation. |
| to encode / to decode | Technical Verb | To convert a value to its canonical byte representation, and back. |
| to hash | Technical Verb | To compute a fixed-size fingerprint of a value using a hash function. |
| to sign / to verify | Technical Verb | To produce a digital signature over a value with a private key, and to check that signature against the corresponding public key. |
