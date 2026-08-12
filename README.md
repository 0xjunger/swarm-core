# Swarm Core

Swarm Core receives a signed, cryptographically linked log of a swarm's
actions and a specification of the actions permitted for its mission. It
produces a machine-checkable verdict: whether any node acted outside its
authority, checked directly against the raw signed log — never trusted from
a summary or a report.

## What this does not prove

This is the most important limit of the project, so it comes first.

Swarm Core proves **procedural compliance**. It does not prove **factual
truth**. A node that reports a false sensor input, and then acts
consistently on that false input, produces a fully valid, fully checkable
proof of a wrong decision — nothing in a signed log carries independent
evidence that what a node claimed actually happened in the physical world.
For this reason, every verdict this project produces is accompanied by an
explicit statement of whether the input itself is attestable at all. In the
current phase, it never is — see `SPEC.md` §2.3 and §9.

## The working demonstration

The verification path is two commands. The first runs a scripted mission
and writes out the log and the mission specification as two files; the
second reads only those two files, with no access to the first command's
memory:

```bash
cargo run -p swarm-sim --example phase1 -- --equivocation \
    --export-bundle /tmp/run.bundle --export-spec /tmp/mission.spec
cargo run -p swarm-verify -- --bundle /tmp/run.bundle --spec /tmp/mission.spec
# -> I1: Violated (Equivocation by node 2), exit 1
```

The second command reads no memory from the first — the two files on disk
are the entire interface between them.

Run `cargo run -p swarm-sim --example phase1` with no flags for the full
narrated walkthrough: partition, contested task claims, healing,
convergence, and an equivocating node caught with no consensus involved at
all.

## Design constraints

Swarm Core's core crate follows two constraints as non-negotiable design
discipline, not as a preference:

- **The sans-I/O boundary.** The core crate has no access to the network,
  to wall-clock time, or to randomness. All three enter only as parameters
  supplied from outside.
- **The `no_std` status.** The core crate builds without the standard
  library, and is cross-compiled to a bare-metal target as part of the
  verification gate, so the claim is proven by a build rather than merely
  asserted.

Four rules hold across the whole project without exception:

1. The signing bytes layout does not change without a major version
   increase.
2. The core crate has no network access, no clock access, and no source of
   randomness.
3. The core crate never depends on the standard library.
4. There is no operation anywhere in this project that merges one node's
   version vector with a peer's self-reported one.

See `DESIGN.md` for why each of these holds.

## Repository map

| Path | Contents |
|---|---|
| `SPEC.md` | The normative specification: wire formats, invariants, the verification procedure. |
| `DESIGN.md` | Why the design is what it is — decision records, rejected alternatives, trade-offs. |
| `CHANGELOG.md` | The project's history, by released capability. |
| `spec/vectors/` | The test-vector corpus a second, independent implementation is checked against. |
| `scripts/verify.sh` | The single command that decides whether this phase's exit criteria are met. |
| `crates/swarm-core/` | The pure, `no_std`, sans-I/O verification and protocol core. |
| `crates/swarm-sim/` | The deterministic simulator used to produce and test scenarios. |
| `crates/swarm-verify/` | The external verifier: `LogBundle` + `Spec` → `Verdict`, from bytes alone. |
| `LICENSE` / `NOTICE` | Licensing. |
| `SECURITY.md` | Scope and reporting process for a security concern. |
| `CONTRIBUTING.md` | How to build, how to test, and the project's writing rules. |
| `CODE_OF_CONDUCT.md` | Community conduct expectations. |

## Building and testing

```bash
cargo test --workspace                              # the full test suite
cargo build -p swarm-core --target thumbv7em-none-eabihf   # the no_std, bare-metal target
./scripts/verify.sh                                  # the full exit-criteria gate
```

`scripts/verify.sh` runs the full workspace test suite; rebuilds the core
crate with a deliberately broken negative-control build and requires that
build's own test to fail (a checker that cannot fail against a known-broken
build is not a checker); does the same for the external verifier against
its own negative control; runs the two-command scenario above in both
directions — a run containing a known violation must be rejected, a clean
run must be accepted, both from exported files alone; cross-compiles the
core crate to a bare-metal target to prove the `no_std` claim; and runs the
lint suite with warnings denied. A green run means this phase's exit
criteria are met; nothing else in this repository decides that on its own.

## Status

The wire format, the causal-delivery protocol, the task-claim CRDT,
equivocation detection, the escrow counter, and the external verifier are
implemented and tested, and `scripts/verify.sh` is green on a clean
checkout. Not yet implemented: a real network transport, signed mission
specifications, Merkle-Mountain-Range-based log pruning, and any kind of
field validation — everything in this repository has run in simulation
only.

Two later phases are planned to keep `V`'s semantics unchanged while
changing only the substrate that executes it: a phase where the same
decision logic runs inside a hardware-attested binary, and a phase where it
is recompiled as a zero-knowledge circuit. Neither phase has a committed
date.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
