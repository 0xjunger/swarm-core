//! Causal delivery and anti-entropy mechanics (`docs/spec.md` §9.3-9.5).
//!
//! `src/lib.rs`'s own inline tests cover the basic shape of each mechanism
//! (an unsatisfied entry buffers, a satisfied one applies, a duplicate is a
//! no-op, an anti-entropy reply carries the gap). This file goes further:
//! multi-entry, multi-origin scenarios that only make sense once several
//! nodes and several `step` calls are in play — including invariant **I2**
//! directly (`docs/spec.md` §13).

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use swarm_core::causal::VersionVector;
use swarm_core::wire::{Body, Entry, Hash, Roster, UnsignedEntry, PHASE1_EPOCH, PHASE1_MISSION_ID};
use swarm_core::{step, Envelope, Event, LogicalTime, NodeId, State};

fn key(seed: u8) -> SigningKey {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    SigningKey::from_bytes(&bytes)
}

/// A 4-node roster: `W=0, X=1, Y=2, Z=3`.
fn roster4() -> (Roster, [SigningKey; 4]) {
    let keys = [key(1), key(2), key(3), key(4)];
    let mut m = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        m.insert(NodeId(i as u8), k.verifying_key());
    }
    (Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, m), keys)
}

fn state(me: NodeId, roster: &Roster, k: &SigningKey) -> State {
    // Periods disabled: these tests drive delivery by hand, one `Recv` at a
    // time, and must not have a node also autonomously authoring entries.
    State::new(me, roster.clone(), k.clone(), 64, 8, 0, 0)
}

fn claim(task: u64) -> Body {
    Body::TaskClaim { task, priority: 1 }
}

/// Hand-builds an entry with an arbitrary `deps`, bypassing `log::Log` so a
/// scenario can construct exactly the causal shape it needs (e.g. an entry
/// that depends on seqs from two other origins at once) without first
/// growing real chains for all of them.
fn raw_entry(node: NodeId, key: &SigningKey, seq: u64, prev: Hash, deps: VersionVector) -> Entry {
    UnsignedEntry {
        mission_id: PHASE1_MISSION_ID,
        epoch: PHASE1_EPOCH,
        node,
        seq,
        prev,
        deps,
        body: claim(seq),
    }
    .sign(key)
}

fn vv(pairs: &[(NodeId, u64)]) -> VersionVector {
    let mut v = VersionVector::new();
    for &(n, s) in pairs {
        v.bump(n, s);
    }
    v
}

fn recv(state: &State, from: NodeId, entry: Entry, at: u64) -> State {
    let (next, fx) = step(
        state,
        Event::Recv {
            from,
            payload: Envelope::Entry(entry),
        },
        LogicalTime(at),
    );
    assert!(fx.is_empty(), "delivering an entry never itself replies");
    next
}

// ---------------------------------------------------------------------------
// Same-origin gap: fixed-point drain
// ---------------------------------------------------------------------------

#[test]
fn a_same_origin_gap_fills_and_drains_to_a_fixed_point_in_one_step() {
    let (roster, keys) = roster4();
    let w = NodeId(0);
    let z = NodeId(3);

    let w0 = raw_entry(w, &keys[0], 0, Hash::ZERO, VersionVector::new());
    let w1 = raw_entry(w, &keys[0], 1, w0.chain_hash(), vv(&[(w, 0)]));

    let observer = state(z, &roster, &keys[3]);

    // seq 1 arrives first: its deps name (W, 0), which hasn't been seen.
    let after_gap = recv(&observer, w, w1.clone(), 10);
    assert_eq!(after_gap.causal_vv().highest(w), None, "not applied");
    assert_eq!(after_gap.buffer_keys().count(), 1);

    // seq 0 arrives: applying it satisfies seq 1's deps in the same `step`
    // call, so both entries are visible immediately — no second delivery
    // needed (`docs/spec.md` §9.3, "drained to a fixed point").
    let after_fill = recv(&after_gap, w, w0.clone(), 11);
    assert_eq!(after_fill.causal_vv().highest(w), Some(1));
    assert_eq!(after_fill.entries(), vec![&w0, &w1]);
    assert_eq!(after_fill.buffer_keys().count(), 0, "buffer drained");
}

// ---------------------------------------------------------------------------
// I2 — cross-node deps: buffered until every dependency, from every origin,
// is separately delivered.
// ---------------------------------------------------------------------------

#[test]
fn i2_an_entry_is_not_applied_before_all_its_cross_node_deps_are_delivered() {
    let (roster, keys) = roster4();
    let (w, x, y, z) = (NodeId(0), NodeId(1), NodeId(2), NodeId(3));

    // W's chain: w0, w1, w2.
    let w0 = raw_entry(w, &keys[0], 0, Hash::ZERO, VersionVector::new());
    let w1 = raw_entry(w, &keys[0], 1, w0.chain_hash(), vv(&[(w, 0)]));
    let w2 = raw_entry(w, &keys[0], 2, w1.chain_hash(), vv(&[(w, 1)]));

    // X's chain: x0, x1.
    let x0 = raw_entry(x, &keys[1], 0, Hash::ZERO, VersionVector::new());
    let x1 = raw_entry(x, &keys[1], 1, x0.chain_hash(), vv(&[(x, 0)]));

    // Y's genesis entry claims it has already seen W up to seq 2 and X up
    // to seq 1 — a real Y would only be able to author this after actually
    // receiving those, but for this test the fabricated claim is exactly
    // what makes the scenario a direct I2 check: the *receiver* below must
    // honour that claim rather than trust it implicitly.
    let y0 = raw_entry(y, &keys[2], 0, Hash::ZERO, vv(&[(w, 2), (x, 1)]));

    let observer = state(z, &roster, &keys[3]);

    // Y's entry arrives first: neither dependency is met yet.
    let s = recv(&observer, y, y0.clone(), 1);
    assert_eq!(s.entries(), Vec::<&Entry>::new());
    assert_eq!(s.buffer_keys().count(), 1);

    // All of W's chain arrives. Y's entry still needs X.
    let s = recv(&s, w, w0.clone(), 2);
    let s = recv(&s, w, w1.clone(), 3);
    let s = recv(&s, w, w2.clone(), 4);
    assert_eq!(s.causal_vv().highest(w), Some(2));
    assert!(
        s.entries().iter().all(|e| e.node != y),
        "I2 violated: Y's entry applied before X's deps were met"
    );
    assert_eq!(s.buffer_keys().count(), 1, "Y's entry is still buffered");

    // X's chain arrives. Now both of Y's deps are met, in the same step
    // that delivers the last one.
    let s = recv(&s, x, x0.clone(), 5);
    let s = recv(&s, x, x1.clone(), 6);
    assert!(
        s.entries().contains(&&y0),
        "Y's entry applies once both deps land"
    );
    assert_eq!(s.buffer_keys().count(), 0);
}

// ---------------------------------------------------------------------------
// The causal buffer's bound
// ---------------------------------------------------------------------------

#[test]
fn the_causal_buffer_evicts_the_oldest_entry_when_full() {
    let (roster, keys) = roster4();
    let (w, x, y, z) = (NodeId(0), NodeId(1), NodeId(2), NodeId(3));

    // Each entry depends on a predecessor from its own origin that this
    // test never delivers, so all three stay buffered forever.
    let unsatisfiable =
        |node: NodeId, key: &SigningKey| raw_entry(node, key, 5, Hash::ZERO, vv(&[(node, 4)]));

    let mut observer = State::new(z, roster.clone(), keys[3].clone(), 64, 2, 0, 0);

    observer = recv(&observer, w, unsatisfiable(w, &keys[0]), 1);
    observer = recv(&observer, x, unsatisfiable(x, &keys[1]), 2);
    assert_eq!(observer.buffer_keys().count(), 2, "buffer at its cap");

    // A third, at a later tick: the buffer is full, so the oldest —
    // smallest (inserted_at, origin, seq), i.e. W's, inserted at tick 1 —
    // is evicted to make room (`docs/spec.md` §9.4).
    observer = recv(&observer, y, unsatisfiable(y, &keys[2]), 3);

    let keys_held: Vec<(NodeId, u64)> = observer.buffer_keys().collect();
    assert_eq!(keys_held, [(x, 5), (y, 5)], "W's entry was the oldest");
}

// ---------------------------------------------------------------------------
// Anti-entropy: the gap computation spans every origin the sender is behind
// on, ascending by origin then seq.
// ---------------------------------------------------------------------------

#[test]
fn anti_entropy_reply_spans_every_behind_origin_ascending() {
    let (roster, keys) = roster4();
    let (w, x, y) = (NodeId(0), NodeId(1), NodeId(2));

    let w0 = raw_entry(w, &keys[0], 0, Hash::ZERO, VersionVector::new());
    let w1 = raw_entry(w, &keys[0], 1, w0.chain_hash(), vv(&[(w, 0)]));

    // Y receives both of W's entries, then authors its own via a real `Tick`
    // (entry_period fires every tick here) — this is the only path that
    // correctly appends to Y's own log and advances its own causal_vv
    // component, exactly as `docs/spec.md` §9.2 specifies.
    //
    // Y authors *two* entries in that one tick, and that is M3 working as
    // specified: `raw_entry` gives w0 the body `TaskClaim { task: 0 }`, Y's
    // own first claim is also task 0 (`docs/spec.md` §10.6 numbers claims by
    // the author's own claim count), and W wins it — W's claim has lc 0
    // against Y's lc 2, and lower lc wins (§5). So Y claims, immediately
    // observes that it lost, and withdraws in the same tick.
    let y_state = State::new(y, roster.clone(), keys[2].clone(), 64, 8, 1, 0);
    let y_state = recv(&y_state, w, w0, 1);
    let y_state = recv(&y_state, w, w1, 2);
    let (y_state, y0_fx) = step(&y_state, Event::Tick, LogicalTime(3));
    assert_eq!(
        y0_fx.len(),
        6,
        "a claim and a withdrawal, each broadcast to W, X and Z, never to itself"
    );

    // X advertises a VV that only knows W's seq 0 — behind on W by one, and
    // has never heard of Y at all.
    let x_vv = vv(&[(w, 0)]);
    let (_, fx) = step(
        &y_state,
        Event::Recv {
            from: x,
            payload: Envelope::AntiEntropy(x_vv),
        },
        LogicalTime(5),
    );

    let sent: Vec<(NodeId, u64)> = fx
        .iter()
        .map(|swarm_core::Effect::Send { payload, .. }| match payload {
            Envelope::Entry(e) => (e.node, e.seq),
            Envelope::AntiEntropy(_) => panic!("expected entries only"),
        })
        .collect();
    // Ascending by origin (W=0 before Y=2), then by seq within an origin —
    // both of Y's entries, since X has never heard of Y at all.
    assert_eq!(sent, [(w, 1), (y, 0), (y, 1)]);
    assert!(fx
        .iter()
        .all(|swarm_core::Effect::Send { to, .. }| *to == x));
}

// ---------------------------------------------------------------------------
// Idempotency: re-delivering an entry that is already buffered (not yet
// applied) does not disturb the buffer.
// ---------------------------------------------------------------------------

#[test]
fn re_delivering_an_already_buffered_entry_is_a_no_op() {
    let (roster, keys) = roster4();
    let w = NodeId(0);
    let z = NodeId(3);

    let w1 = raw_entry(w, &keys[0], 1, Hash::ZERO, vv(&[(w, 0)]));

    let observer = state(z, &roster, &keys[3]);
    let once = recv(&observer, w, w1.clone(), 1);
    let twice = recv(&once, w, w1, 2);

    assert_eq!(once.buffer_keys().count(), 1);
    assert_eq!(
        once.buffer_keys().collect::<Vec<_>>(),
        twice.buffer_keys().collect::<Vec<_>>()
    );
    assert_eq!(once.causal_vv(), twice.causal_vv());
}
