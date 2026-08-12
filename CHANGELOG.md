# Changelog

All notable changes to this project are documented in this file. The
format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and version numbers follow [Semantic Versioning](https://semver.org/).

## [0.1.0] — 2026-08-10

The first published version. Phase 1 is closed: `scripts/verify.sh` is
green on a clean checkout.

### Added

- **M0** — A deterministic network simulator: a node list, a message
  queue, a tick loop, and a seeded model of loss, delay, and partition.
- **M1** — The signed log entry (`Entry`), canonical encoding with domain
  separation, Ed25519 signatures, a per-node hash chain, an end-to-end
  chain verifier with a compile-time-enforced verified/unverified type
  split, a bounded log, and the first golden-vector test.
- **M2** — Causal message delivery with a self-inclusive version-vector
  dependency rule, a fixed-point causal buffer drain, a bounded causal
  buffer with drop-oldest eviction under overflow, and periodic
  anti-entropy resynchronization between nodes.
- **M3** — The task-claim CRDT: a deterministic winner rule
  (`priority`, then a derived logical clock, then node identifier, then
  sequence number), a grow-only claim set, and withdrawal recorded as a
  log fact rather than a set removal.
- **M4** — Equivocation detection: a self-verifying proof of equivocation
  built from two conflicting signed entries alone, checkable by a third
  party holding only the roster's public keys, with no consensus round
  required.
- **M5** — The escrow counter: a fixed per-node spending allocation,
  enforced structurally so that the global spending total across every
  partition never exceeds the authorized total.
- **M6** — An independent, in-process invariant checker exercised across
  thousands of randomized scenarios, plus a deliberately broken build used
  as a negative control to prove the checker can actually fail.
- **M7** — An external verifier: a `LogBundle`/`Spec` pair, read from
  files alone with no access to the process that produced them, checked by
  a `swarm-verify` binary that produces a typed `Verdict` with an
  independently checkable `Witness` for every violation. Added the
  two-command, file-only verification scenario as the project's exit
  criterion.
- Added a check that detects a signed chain filed in a bundle under a node
  identifier different from its actual signer, closing a case where a
  genuine equivocation could otherwise land in two different comparison
  buckets and go unreported.
- Added an automated, scripted run of the two-command external-verification
  scenario, in both directions, as part of the project's exit-criteria
  gate.
- Added a negative control for the external verifier's own equivocation
  check, mirroring the existing negative control for the in-process
  checker.
- Added a negative-control test demonstrating that the causal-delivery
  invariant, checked externally by the verifier, is discharged by a
  genuinely independent replay rather than by trusting the log's producer.
- Closed Phase 1: every exit criterion in `SPEC.md` §1.1 is met on a clean
  checkout.
- Rewrote the project's public documentation: `SPEC.md` (a normative
  specification, restructured and translated), `DESIGN.md` (decision
  records, restructured and translated from the project's original working
  notes), `README.md`, `docs/GLOSSARY.md`, `docs/STE-DEVIATIONS.md`, and
  the `spec/vectors/` test-vector corpus.
