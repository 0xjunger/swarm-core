//! Regenerates `crates/swarm-verify/tests/fixtures/*` (`SPEC.md`
//! §8, E7a). The fixture corpus is committed to the repo like the golden
//! vectors (`swarm-core/tests/golden_vector.rs`); this binary exists so a
//! stranger can reproduce every byte of it, and so
//! `tests/fixtures.rs::regenerated_fixtures_match_committed_bytes` has
//! something independent to compare the committed files against.
//!
//!   cargo run -p swarm-verify --example gen_fixtures
//!
//! Deterministic: fixed seeds throughout (`fixture_data.rs`), so running
//! this twice writes byte-identical files.

#[path = "../tests/support/fixture_data.rs"]
mod fixture_data;

use std::fs;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&dir).expect("create tests/fixtures");

    write(&dir, "clean", fixture_data::clean());
    write(&dir, "equivocation", fixture_data::equivocation());
    write(&dir, "overspend", fixture_data::overspend());
    write(&dir, "broken_chain", fixture_data::broken_chain());
    write(&dir, "misfiled_chain", fixture_data::misfiled_chain());
    write(&dir, "missing_node", fixture_data::missing_node());

    fs::write(
        dir.join("truncated.bundle"),
        fixture_data::truncated_bytes(),
    )
    .expect("write truncated.bundle");
    fs::write(
        dir.join("truncated.spec"),
        fixture_data::truncated_spec().encode(),
    )
    .expect("write truncated.spec");

    println!("wrote fixtures to {}", dir.display());
}

fn write(
    dir: &Path,
    name: &str,
    (bundle, spec): (swarm_core::bundle::LogBundle, swarm_core::bundle::Spec),
) {
    fs::write(dir.join(format!("{name}.bundle")), bundle.encode())
        .unwrap_or_else(|e| panic!("write {name}.bundle: {e}"));
    fs::write(dir.join(format!("{name}.spec")), spec.encode())
        .unwrap_or_else(|e| panic!("write {name}.spec: {e}"));
}
