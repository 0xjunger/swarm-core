//! X2a: the process-boundary test. Runs the actual `swarm-verify` binary
//! against the committed fixture files via `std::process::Command` — no
//! simulator involved, so this exercises exactly what a stranger holding
//! nothing but two files on disk would run.
//!
//! Asserts the exit-code contract: `0` every invariant `Satisfied` and no
//! chain finding, `1` at least one `Violated` invariant or chain finding
//! (`Undetermined` alone does not count), `2` a decode, format, or usage
//! error.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_path(name: &str, ext: &str) -> String {
    fixtures_dir()
        .join(format!("{name}.{ext}"))
        .to_str()
        .expect("fixture path is valid UTF-8")
        .to_string()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_swarm-verify"))
        .args(args)
        .output()
        .expect("failed to run swarm-verify binary")
}

fn run_fixture(name: &str) -> Output {
    run(&[
        "--bundle",
        &fixture_path(name, "bundle"),
        "--spec",
        &fixture_path(name, "spec"),
    ])
}

fn assert_exit(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clean_exits_zero() {
    assert_exit(&run_fixture("clean"), 0);
}

#[test]
fn equivocation_exits_one() {
    assert_exit(&run_fixture("equivocation"), 1);
}

#[test]
fn overspend_exits_one() {
    assert_exit(&run_fixture("overspend"), 1);
}

#[test]
fn broken_chain_exits_one() {
    assert_exit(&run_fixture("broken_chain"), 1);
}

#[test]
fn misfiled_chain_exits_one() {
    assert_exit(&run_fixture("misfiled_chain"), 1);
}

#[test]
fn missing_node_exits_zero_since_undetermined_is_not_a_violation() {
    let output = run_fixture("missing_node");
    assert_exit(&output, 0);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Undetermined"));
}

#[test]
fn truncated_exits_two_on_decode_failure() {
    assert_exit(&run_fixture("truncated"), 2);
}

#[test]
fn missing_spec_argument_exits_two() {
    let output = run(&["--bundle", &fixture_path("clean", "bundle")]);
    assert_exit(&output, 2);
}

#[test]
fn nonexistent_bundle_path_exits_two() {
    let output = run(&[
        "--bundle",
        "/nonexistent/path/does-not-exist.bundle",
        "--spec",
        &fixture_path("clean", "spec"),
    ]);
    assert_exit(&output, 2);
}

#[test]
fn json_on_clean_writes_to_stdout_and_still_exits_zero() {
    let output = run(&[
        "--bundle",
        &fixture_path("clean", "bundle"),
        "--spec",
        &fixture_path("clean", "spec"),
        "--json",
    ]);
    assert_exit(&output, 0);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"i1\""));
    assert!(stdout.contains("\"Satisfied\""));
}
