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

echo "== no_std: swarm-core must cross-compile for a bare-metal target =="
cargo build -p swarm-core --target thumbv7em-none-eabihf

echo "== lint: workspace must be clippy-clean =="
cargo clippy --workspace -- -D warnings

echo "OK: all checks passed."
