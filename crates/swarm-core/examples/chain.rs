//! The M1 demo: build a signed hash chain, verify it, tamper with it, and
//! watch verification fail.
//!
//!   cargo run -q -p swarm-core --example chain
//!   cargo run -q -p swarm-core --example chain -- --len 5000
//!
//! This is the visible form of the acceptance tests in `tests/chain.rs`:
//! the tamper-resistance claim of `DESIGN.md` §M1, demonstrated rather than
//! asserted. Terminal output only — GUIs and visualisation are out of scope
//! for all of Phase 1.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use swarm_core::causal::VersionVector;
use swarm_core::log::{verify_chain, Log};
use swarm_core::wire::{Body, Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::NodeId;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let len = flag(&args, "--len").unwrap_or(1000) as usize;

    // A deterministic key, injected by the caller: the core never generates
    // randomness (DESIGN.md §11.1).
    let mut seed = [0u8; 32];
    seed[0] = 1;
    let key = SigningKey::from_bytes(&seed);
    let node = NodeId(0);

    let mut keys = BTreeMap::new();
    keys.insert(node, key.verifying_key());
    let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

    println!("M1 — one node, a signed hash chain (docs/spec-m1.md)");
    println!();

    // 1. Build.
    let mut log = Log::new(node, key.clone(), len.max(1));
    for i in 0..len {
        log.append(
            Body::TaskClaim {
                task: i as u64,
                priority: 1,
            },
            VersionVector::new(),
        )
        .unwrap();
    }
    println!(
        "build     {len} entries: Ed25519-signed, BLAKE3-linked, seq 0..={}",
        len - 1
    );
    for i in [0usize, 1, len - 1] {
        let e = &log.entries()[i];
        println!(
            "          #{i:04}  prev={}…  seq={}  {:?}",
            hex(&e.prev.0[..4]),
            e.seq,
            e.body
        );
    }
    let head = log.entries()[len - 1].chain_hash();
    println!(
        "          head  {}…  identical on every run — nothing random enters the core",
        hex(&head.0[..8])
    );
    println!();

    // 2. Verify.
    print!("verify    all {len} entries end to end ... ");
    match verify_chain(&roster, log.entries()) {
        Ok(v) => println!("OK: {} entries became VerifiedEntry", v.len()),
        Err(e) => println!("FAILED: {e:?}"),
    }
    println!();

    // 3. Tamper with one byte in the middle, and verify again.
    let mut entries = log.entries().to_vec();
    let original = entries[len / 2].clone();
    // Claims only in this chain, so the M3 `Withdraw` variant cannot occur.
    let Body::TaskClaim { task, priority } = entries[len / 2].body else {
        unreachable!("this demo appends TaskClaim entries only");
    };
    entries[len / 2].body = Body::TaskClaim {
        task,
        priority: priority + 1, // exactly one byte of the canonical encoding
    };
    println!(
        "tamper    one byte of entry #{}: priority {} -> {}",
        len / 2,
        priority,
        priority + 1
    );
    print!("verify    again ... ");
    match verify_chain(&roster, &entries) {
        Ok(_) => println!("OK (this must never happen)"),
        Err(e) => println!("FAILED at: {e:?}"),
    }
    println!("          one altered byte broke the chain — the tamper-resistance");
    println!("          claim of DESIGN.md §M1, demonstrated rather than asserted");
    println!();

    // 4. Restore, then attack invariant I1 instead: the same (node, seq) twice.
    entries[len / 2] = original;
    entries.insert(4, entries[3].clone());
    println!(
        "restore   entry #{}, then duplicate entry #3 — same (node, seq) twice",
        len / 2
    );
    print!("verify    again ... ");
    match verify_chain(&roster, &entries) {
        Ok(_) => println!("OK (this must never happen)"),
        Err(e) => println!("FAILED at: {e:?}"),
    }
    println!("          invariant I1: at most one signed entry per (node, seq)");
}

fn flag(args: &[String], name: &str) -> Option<u64> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1)?.parse().ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
