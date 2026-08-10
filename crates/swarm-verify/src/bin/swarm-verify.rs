//! `swarm-verify --bundle <path> --spec <path> [--json]` (`docs/spec.md`
//! §20.6) — the binary a stranger runs. It reads exactly two files and
//! prints a [`Verdict`]; it has no access to, and no notion of, the process
//! that produced them.
//!
//! Exit codes: `0` every invariant `Satisfied` and no chain finding; `1` at
//! least one `Violated` invariant or chain finding; `2` a decode, format, or
//! usage error. `Undetermined` alone does not change the exit code, but is
//! always printed.

use std::env;
use std::fs;
use std::process::ExitCode;

use swarm_core::bundle::{LogBundle, Spec};
use swarm_verify::verdict::{ChainFinding, ChainProblem, InvariantResult, Verdict, Witness};
use swarm_verify::verify;

struct Args {
    bundle: String,
    spec: String,
    json: bool,
}

const USAGE: &str = "usage: swarm-verify --bundle <path> --spec <path> [--json]";

fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Args, String> {
    let mut bundle = None;
    let mut spec = None;
    let mut json = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bundle" => {
                bundle = Some(args.next().ok_or("--bundle requires a path argument")?);
            }
            "--spec" => {
                spec = Some(args.next().ok_or("--spec requires a path argument")?);
            }
            "--json" => json = true,
            other => return Err(format!("unrecognised argument: {other}")),
        }
    }

    Ok(Args {
        bundle: bundle.ok_or("--bundle <path> is required")?,
        spec: spec.ok_or("--spec <path> is required")?,
        json,
    })
}

fn main() -> ExitCode {
    let args = match parse_args(env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let bundle_bytes = match fs::read(&args.bundle) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: could not read bundle file '{}': {e}", args.bundle);
            return ExitCode::from(2);
        }
    };
    let spec_bytes = match fs::read(&args.spec) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: could not read spec file '{}': {e}", args.spec);
            return ExitCode::from(2);
        }
    };

    let bundle = match LogBundle::decode(&bundle_bytes) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: could not decode bundle '{}': {e:?}", args.bundle);
            return ExitCode::from(2);
        }
    };
    let spec = match Spec::decode(&spec_bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not decode spec '{}': {e:?}", args.spec);
            return ExitCode::from(2);
        }
    };

    let verdict = verify(&bundle, &spec);

    if args.json {
        println!("{}", render_json(&verdict));
    } else {
        render_human(&verdict);
    }

    if verdict.any_violated() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// Human-readable rendering
// ---------------------------------------------------------------------------

fn render_human(verdict: &Verdict) {
    if !verdict.chains.is_empty() {
        println!("chains:");
        for finding in &verdict.chains {
            println!("  {}", describe_chain_finding(finding));
        }
    }
    println!("I1: {}", describe_result(&verdict.i1));
    println!("I2: {}", describe_result(&verdict.i2));
    println!("I3: {}", describe_result(&verdict.i3));
    println!("I4: {}", describe_result(&verdict.i4));
    println!("structural: {}", verdict.structural_note);
    println!(
        "input_attestable: {} (Phase 1 — no input attestation)",
        verdict.input_attestable
    );
}

fn describe_result(result: &InvariantResult) -> String {
    match result {
        InvariantResult::Satisfied => "Satisfied".to_string(),
        InvariantResult::Violated(w) => format!("Violated ({})", describe_witness(w)),
        InvariantResult::Undetermined(reason) => format!("Undetermined ({reason})"),
    }
}

fn describe_witness(witness: &Witness) -> String {
    match witness {
        Witness::Equivocation(poe) => format!("Equivocation by node {}", poe.node().0),
        Witness::UnmetDependency {
            observer,
            entry,
            missing,
        } => format!(
            "observer {} holds entry (author {}, seq {}) with unmet dependency (author {}, seq {})",
            observer.0, entry.node.0, entry.seq, missing.0.0, missing.1
        ),
        Witness::Divergence { a, b, task, .. } => format!(
            "observers {} and {} disagree on the winner of task {}",
            a.0, b.0, task
        ),
        Witness::Overspend {
            node,
            budget,
            entries,
        } => format!(
            "node {} exceeded budget {} across {} Spend entries",
            node.0,
            budget,
            entries.len()
        ),
    }
}

fn describe_chain_finding(finding: &ChainFinding) -> String {
    let problem = match &finding.error {
        ChainProblem::Chain(e) => format!("{e:?}"),
        ChainProblem::TooLong { cap, actual } => {
            format!("chain length {actual} exceeds spec.log_cap {cap}")
        }
    };
    format!(
        "observer {} author {}: {problem}",
        finding.observer.0, finding.author.0
    )
}

// ---------------------------------------------------------------------------
// Machine-readable (JSON) rendering
// ---------------------------------------------------------------------------
//
// Hand-written, no serde: consistent with `swarm-core`'s own rule against
// letting a general-purpose serializer decide the shape of anything this
// project claims is canonical (`docs/spec.md` §8.2). Every raw `Entry`
// referenced by a witness is emitted as its hex-encoded full canonical
// encoding (`docs/spec.md` §8.2) — not summarised — so a reader can decode
// and check it independently, the same discipline `Witness` itself follows
// (`docs/spec.md` §20.4).

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_entry(entry: &swarm_core::wire::Entry) -> String {
    format!(
        "{{\"author\":{},\"seq\":{},\"hex\":{}}}",
        entry.node.0,
        entry.seq,
        json_string(&hex(&entry.encoded()))
    )
}

fn render_json(verdict: &Verdict) -> String {
    let chains: Vec<String> = verdict.chains.iter().map(json_chain_finding).collect();
    format!(
        "{{\"chains\":[{}],\"i1\":{},\"i2\":{},\"i3\":{},\"i4\":{},\"structural_note\":{},\"input_attestable\":{}}}",
        chains.join(","),
        json_result(&verdict.i1),
        json_result(&verdict.i2),
        json_result(&verdict.i3),
        json_result(&verdict.i4),
        json_string(verdict.structural_note),
        verdict.input_attestable,
    )
}

fn json_result(result: &InvariantResult) -> String {
    match result {
        InvariantResult::Satisfied => "{\"status\":\"Satisfied\"}".to_string(),
        InvariantResult::Violated(w) => {
            format!("{{\"status\":\"Violated\",\"witness\":{}}}", json_witness(w))
        }
        InvariantResult::Undetermined(reason) => format!(
            "{{\"status\":\"Undetermined\",\"reason\":{}}}",
            json_string(reason)
        ),
    }
}

fn json_witness(witness: &Witness) -> String {
    match witness {
        Witness::Equivocation(poe) => format!(
            "{{\"kind\":\"Equivocation\",\"node\":{},\"a\":{},\"b\":{}}}",
            poe.node().0,
            json_entry(poe.a()),
            json_entry(poe.b())
        ),
        Witness::UnmetDependency {
            observer,
            entry,
            missing,
        } => format!(
            "{{\"kind\":\"UnmetDependency\",\"observer\":{},\"entry\":{},\"missing_author\":{},\"missing_seq\":{}}}",
            observer.0,
            json_entry(entry),
            missing.0 .0,
            missing.1
        ),
        Witness::Divergence {
            a,
            b,
            task,
            winner_a,
            winner_b,
        } => format!(
            "{{\"kind\":\"Divergence\",\"observer_a\":{},\"observer_b\":{},\"task\":{},\"winner_a\":{},\"winner_b\":{}}}",
            a.0,
            b.0,
            task,
            json_claim(winner_a.as_ref()),
            json_claim(winner_b.as_ref())
        ),
        Witness::Overspend {
            node,
            budget,
            entries,
        } => format!(
            "{{\"kind\":\"Overspend\",\"node\":{},\"budget\":{},\"entries\":[{}]}}",
            node.0,
            budget,
            entries.iter().map(json_entry).collect::<Vec<_>>().join(",")
        ),
    }
}

fn json_claim(claim: Option<&swarm_core::state::Claim>) -> String {
    match claim {
        None => "null".to_string(),
        Some(c) => format!(
            "{{\"priority\":{},\"lc\":{},\"node\":{},\"seq\":{}}}",
            c.priority, c.lc, c.node.0, c.seq
        ),
    }
}

fn json_chain_finding(finding: &ChainFinding) -> String {
    let problem = match &finding.error {
        ChainProblem::Chain(e) => format!("{{\"kind\":\"ChainError\",\"detail\":{}}}", json_string(&format!("{e:?}"))),
        ChainProblem::TooLong { cap, actual } => format!(
            "{{\"kind\":\"TooLong\",\"cap\":{cap},\"actual\":{actual}}}"
        ),
    };
    format!(
        "{{\"observer\":{},\"author\":{},\"error\":{problem},\"entries\":[{}]}}",
        finding.observer.0,
        finding.author.0,
        finding.entries.iter().map(json_entry).collect::<Vec<_>>().join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_accepts_bundle_spec_and_json() {
        let args = parse_args(
            ["--bundle", "a.bundle", "--spec", "b.spec", "--json"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert_eq!(args.bundle, "a.bundle");
        assert_eq!(args.spec, "b.spec");
        assert!(args.json);
    }

    #[test]
    fn parse_args_defaults_json_to_false() {
        let args = parse_args(["--bundle", "a.bundle", "--spec", "b.spec"].into_iter().map(String::from))
            .unwrap();
        assert!(!args.json);
    }

    #[test]
    fn parse_args_requires_bundle_and_spec() {
        assert!(parse_args(["--spec", "b.spec"].into_iter().map(String::from)).is_err());
        assert!(parse_args(["--bundle", "a.bundle"].into_iter().map(String::from)).is_err());
        assert!(parse_args(std::iter::empty()).is_err());
    }

    #[test]
    fn parse_args_rejects_unknown_flags() {
        assert!(parse_args(["--nonsense"].into_iter().map(String::from)).is_err());
    }

    #[test]
    fn json_string_escapes_quotes_and_backslashes() {
        assert_eq!(json_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
