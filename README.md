# swarm-core

Verifiable swarm coordination: a distributed-systems layer for drone (or any
partition-tolerant) swarms that acts optimistically and proves accountably.
Nodes coordinate through a signed, hash-linked log rather than a consensus
protocol, so a network partition never stops the swarm — it only delays
convergence. Every claim the system makes ("no node equivocated," "no node
overspent its budget") is checked against the raw signed log, not trusted from
a report.

As of M7, that check no longer has to happen inside this repo's own
simulator. `swarm-verify` is a standalone binary: given a `LogBundle` (the raw
signed entries a run produced) and a `Spec` (the mission's roster and rules),
it produces a `Verdict` from the bytes alone. Nobody verifying a run needs to
trust the process that generated it.

## Running it

The exit gate for Phase 1 is one command:

```bash
./scripts/verify.sh
```

It runs the full workspace test suite, rebuilds `swarm-core` with the
`mutant-i3` negative control and requires that build to fail (a checker that
cannot fail on a deliberately broken build is not a checker), cross-compiles
`swarm-core` for `thumbv7em-none-eabihf` to prove the `no_std` claim, and runs
`cargo clippy --workspace -- -D warnings`. Green means Phase 1's criteria are
met; nothing else in this repository decides that.

The external-verification path — the actual point of the project — is two
commands. The first runs a scripted demo simulation and writes out the log and
the mission spec as files; the second reads only those two files, with no
access to the first command's memory:

```bash
cargo run -p swarm-sim --example phase1 -- --equivocation \
    --export-bundle /tmp/run.bundle --export-spec /tmp/mission.spec
cargo run -p swarm-verify -- --bundle /tmp/run.bundle --spec /tmp/mission.spec
# -> I1: Violated (Equivocation by node 2), exit 1
```

Run `cargo run -p swarm-sim --example phase1` with no flags for the full
narrated walkthrough (partition, contested claims, healing, convergence, an
equivocator caught with no consensus at all).

## Status

**Phase 1.** The wire format, the causal-delivery protocol, the task-claim
CRDT, equivocation detection, the escrow counter, and the external verifier
are implemented and tested (see `docs/spec.md` §1 and §18 for the milestone
history). What Phase 1 does **not** have: a real network transport
(`swarm-net`), signed mission specs, MMR-based log pruning, or any field
validation — everything here has run in simulation only.

## `DESIGN.md` vs `docs/spec.md`

`DESIGN.md` (Turkish) is the project's source of truth for *what* and *why* —
the motivating problem, the design principles, the phase roadmap. `docs/spec.md`
is the normative specification for *how*: the exact wire formats, protocol
rules, and invariants the code implements, kept current rather than frozen
per milestone. A decision is written into `spec.md` before it enters code.

## License

Apache-2.0. See [LICENSE](LICENSE).
