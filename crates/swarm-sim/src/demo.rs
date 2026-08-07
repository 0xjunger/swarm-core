//! Formatting shared by the terminal demos in `examples/`.
//!
//! **Deliberate exception to "examples add no code to the library."** That
//! rule held while there was one example (`watch.rs`) and made sense: a demo
//! reading nothing but the public API is proof the API is sufficient. It
//! stopped holding once a third demo (`claim.rs`) copied the same
//! `pretty_groups`/`envelope_label`/`flag` verbatim — at that point the rule
//! was producing duplication, not preventing coupling. This module is CLI
//! presentation only (ANSI color codes, an arg parser, a couple of one-line
//! renderers); it contains no protocol logic and nothing here is on the path
//! `swarm_core::step` runs through, so it does not weaken the sans-I/O
//! argument the original rule was protecting. See `docs/spec.md` §3.

use swarm_core::wire::Body;
use swarm_core::Envelope;

pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GRN: &str = "\x1b[32m";
pub const YEL: &str = "\x1b[33m";
pub const CYN: &str = "\x1b[36m";
pub const BLD: &str = "\x1b[1m";
pub const RST: &str = "\x1b[0m";

/// `--name value` from a raw argv slice. Returns `None` if absent or
/// unparsable, so callers supply the default with `.unwrap_or(...)`.
pub fn flag(args: &[String], name: &str) -> Option<u64> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1)?.parse().ok()
}

/// A short human label for what an envelope carries.
pub fn envelope_label(e: &Envelope) -> String {
    match e {
        Envelope::Entry(entry) => match entry.body {
            Body::TaskClaim { task, .. } => {
                format!("entry n{}#{} claim t{task}", entry.node.0, entry.seq)
            }
            Body::Withdraw { task } => {
                format!("entry n{}#{} withdraw t{task}", entry.node.0, entry.seq)
            }
            Body::Spend { amount } => {
                format!("entry n{}#{} spend {amount}", entry.node.0, entry.seq)
            }
        },
        Envelope::AntiEntropy(vv) => format!("vv sync ({} known)", vv.iter().count()),
    }
}

/// Turns `Partition::render`'s `"000:000,001:000,002:001"` into
/// `"{n0 n1} {n2}"`.
pub fn pretty_groups(s: &str) -> String {
    let mut out: Vec<Vec<String>> = Vec::new();
    for pair in s.split(',') {
        let (node, group) = pair.split_once(':').unwrap_or(("0", "0"));
        let g: usize = group.parse().unwrap_or(0);
        while out.len() <= g {
            out.push(Vec::new());
        }
        out[g].push(format!("n{}", node.parse::<u8>().unwrap_or(0)));
    }
    out.iter()
        .map(|g| format!("{{{}}}", g.join(" ")))
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_parses_a_present_value() {
        let args: Vec<String> = ["prog", "--seed", "7"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(flag(&args, "--seed"), Some(7));
    }

    #[test]
    fn flag_is_none_when_absent() {
        let args: Vec<String> = ["prog"].iter().map(|s| s.to_string()).collect();
        assert_eq!(flag(&args, "--seed"), None);
    }

    #[test]
    fn pretty_groups_renders_multiple_partitions() {
        assert_eq!(pretty_groups("000:000,001:000,002:001"), "{n0 n1}  {n2}");
    }
}
