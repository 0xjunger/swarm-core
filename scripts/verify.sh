#!/usr/bin/env bash
# The Phase 1 exit gate. Green here means the criteria are met — nothing
# else does (`docs/spec.md` §1).
set -euo pipefail

echo "== clean build: every invariant must hold =="
cargo test --workspace --release

echo "== negative control: the I3 mutant MUST fail =="
if cargo test --release -p swarm-core --features mutant-i3 \
     -p swarm-sim --test m6_property 2>/dev/null; then
  echo "FAIL: the checker did not catch the deliberately broken build." >&2
  exit 1
fi
echo "OK: the checker caught the mutant."

echo "== external verification: two commands, no shared memory =="
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

cargo run --release -p swarm-sim --example phase1 -- --equivocation \
    --export-bundle "$workdir/run.bundle" --export-spec "$workdir/mission.spec"

if cargo run --release -p swarm-verify -- \
     --bundle "$workdir/run.bundle" --spec "$workdir/mission.spec"; then
  echo "FAIL: the verifier passed a run containing a known equivocation." >&2
  exit 1
fi
echo "OK: the equivocating run was caught from the files alone."

cargo run --release -p swarm-sim --example phase1 -- \
    --export-bundle "$workdir/clean.bundle" --export-spec "$workdir/clean.spec"
cargo run --release -p swarm-verify -- \
    --bundle "$workdir/clean.bundle" --spec "$workdir/clean.spec"
echo "OK: the honest run verified from the files alone."

echo "== no_std: swarm-core must cross-compile for a bare-metal target =="
cargo build -p swarm-core --target thumbv7em-none-eabihf

echo "== lint: workspace must be clippy-clean =="
cargo clippy --workspace -- -D warnings

echo "OK: all checks passed."
