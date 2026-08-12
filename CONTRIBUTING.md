# Contributing

## Contribution policy

This project is not accepting external pull requests yet. If you find a
problem, open an issue describing it — that is welcome and useful. Revisit
this page later if you want to contribute code directly; the project will
update this section when that changes.

## Building and testing

```bash
cargo build --workspace                                    # build everything
cargo test --workspace                                      # the full test suite
cargo build -p swarm-core --target thumbv7em-none-eabihf    # the no_std target
cargo clippy --workspace -- -D warnings                      # lints
cargo fmt --all --check                                      # formatting
./scripts/verify.sh                                          # the full exit-criteria gate
```

The toolchain version is pinned in `rust-toolchain.toml`. Reproducibility
is the point of this project, and it starts with the compiler — install
the pinned version rather than whatever you already have.

## Writing rules

Every document in this repository — `README.md`, `SPEC.md`, `DESIGN.md`,
`CHANGELOG.md`, and prose inside code comments — uses Simplified Technical
English (ASD-STE100): short sentences, the active voice, one term for one
meaning. See `docs/GLOSSARY.md` for the project's registered technical
terms, and `docs/STE-DEVIATIONS.md` for the documented exceptions to this
rule (RFC 2119 keywords, mathematical notation, Rust identifiers, and
byte-layout tables).

## Rules specific to this codebase

1. `crates/swarm-core` never depends on the standard library, the network,
   a clock, or a randomness source. Time, network access, and randomness
   always enter as a parameter. This rule has no exception.
2. Write the invariant before the code that is meant to satisfy it. Do not
   write code that protects a property nobody has stated yet.
3. If you think something needs to be added beyond the current scope, ask
   first, in an issue. "Just a small addition" is how scope grows past what
   the project can actually verify.
4. Every `Body` variant added to the wire format ships together with a
   test. An unused variant is not added speculatively.
5. A change to the wire format updates the golden-vector tests in the same
   change, and states the reason in the commit message.
6. A tie-break rule, a garbage-collection policy, or a buffer-size decision
   is written into `SPEC.md` before it is written into code, not after.
7. Introduce a new technical term with a short, plain explanation on its
   first use in a document, and register it in `docs/GLOSSARY.md`. This
   project assumes a reader who is not already fluent in distributed-systems
   or cryptography jargon.

## Signing your commits

There is no contributor license agreement or developer certificate of
origin process at this time, since the project is not yet accepting
external contributions (see above).
