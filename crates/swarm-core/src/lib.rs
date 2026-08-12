//! The coordination state machine: pure, deterministic, and free of all I/O.
//!
//! Everything here is a function of its arguments. There is no network, no clock,
//! and no randomness inside this crate — time enters as the `now` parameter and
//! nothing else enters at all. `DESIGN.md` D-002 states this rule as
//! non-negotiable and records how it is enforced.
//!
//! `#![no_std]` is not decorative. It makes `std::collections::HashMap`
//! unreachable, and `HashMap`'s hasher is seeded per process, so its iteration
//! order differs between two runs of the same binary. That is exactly the class of
//! bug M0's byte-identical-trace criterion exists to catch. See `DESIGN.md` D-003.
//!
//! # Scope
//!
//! M0's placeholder (count-and-echo) and M1's foundation (`Entry`, the hash
//! chain, an empty `VersionVector`) were superseded at M2, which activated
//! causal delivery and anti-entropy (`SPEC.md` §4.3). **M3** gives the
//! entries their meaning (`SPEC.md` §6.3): a node claims tasks, folds
//! every entry it applies into the task-claim CRDT in [`state`], computes the
//! deterministic winner of each task, and publishes a [`wire::Body::Withdraw`]
//! record for any task it claimed and lost. [`log`] is unchanged from M1;
//! [`wire`] gains one `Body` variant; [`causal`] gains the derived logical
//! clock M3's winner rule needs.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(test)]
extern crate std;

pub mod bundle;
pub mod causal;
pub mod codec;
pub mod fault;
pub mod log;
pub mod policy;
pub mod state;
pub mod wire;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use ed25519_dalek::SigningKey;

use causal::VersionVector;
use fault::{verify_poe, Poe};
use state::{Claims, Escrow, TaskId};
use wire::{Body, Hash, Roster, VerifiedEntry};

/// A member of the roster.
///
/// The roster (the swarm's member list) is fixed at mission start for the whole of
/// Phase 1 — `DESIGN.md` D-005 notes that dynamic membership is where 90% of the
/// complexity comes from. `u8` is sufficient: rosters are capped at N <= 20.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct NodeId(pub u8);

/// The only notion of time in this system.
///
/// There is no wall clock at any layer. `DESIGN.md` D-002 forbids tie-breaking on
/// wall-clock time because GPS time can be spoofed, which would hand claim races to
/// an attacker. Time enters only as this parameter to [`step`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct LogicalTime(pub u64);

/// What a message carries.
///
/// Replaces M0/M1's `Payload` token: nodes now broadcast real entries and
/// exchange version vectors rather than echoing an opaque counter. Not
/// `Copy` — an `Entry` owns a signature, a `VersionVector` owns a
/// `BTreeMap` — so `Event`/`Effect` lose `Copy` along with it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Envelope {
    /// A signed, causally-dependent record (`wire::Entry`).
    Entry(wire::Entry),
    /// An anti-entropy advertisement: the sender's own version vector, so
    /// the receiver can compute what the sender is missing and push it
    /// back.
    AntiEntropy(VersionVector),
}

/// Something that happened *to* a node. The only input to [`step`].
///
/// No variant is added before a test exercises it.
/// M2 dispatches anti-entropy through [`Envelope`] rather than adding a
/// third `Event` variant — the shape here is
/// unchanged from M0/M1, only `Envelope` replaces `Payload`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    /// The clock advanced. Carries no data — `now` is a separate parameter.
    Tick,
    /// A message arrived from `from`.
    Recv { from: NodeId, payload: Envelope },
}

/// Something a node wants the outside world to do. The only output of [`step`].
///
/// The core never performs an effect; it describes one and hands it back. This is
/// what keeps the crate sans-I/O and what makes replay possible.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    Send { to: NodeId, payload: Envelope },
}

/// An entry received but not yet applied: its causal dependencies are not
/// all delivered yet (`DESIGN.md` D-013).
#[derive(Clone, PartialEq, Eq, Debug)]
struct BufferedEntry {
    /// The tick at which this node first saw the entry — used only to pick
    /// an eviction order (§5); never compared across nodes, never used as a
    /// causal or wall-clock timestamp.
    inserted_at: u64,
    entry: wire::Entry,
}

/// Everything a node knows.
///
/// Grows further at M3-M5 (CRDTs, escrow). Must stay
/// `Clone` (and, for the tests below, `PartialEq`), because [`step`] is
/// pure — see the note on the signature below.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct State {
    /// This node's own identity.
    me: NodeId,
    /// Membership, keys, mission id and epoch — fixed for Phase 1.
    roster: Roster,
    /// `roster`'s members excluding `me`, cached ascending by `NodeId`
    /// (`DESIGN.md` D-003) so `step` never has to recompute it.
    members: Vec<NodeId>,
    /// Emit a new entry when `now % entry_period == 0`. Zero disables it.
    entry_period: u64,
    /// Advertise this node's version vector when `now % anti_entropy_period
    /// == 0`. Zero disables it.
    anti_entropy_period: u64,
    /// Messages received. Observable via the `Final` trace record.
    pub recv_count: u64,
    /// Effects emitted.
    pub sent_count: u64,
    /// This node's own signed, hash-linked chain.
    log: log::Log,
    /// Entries received from other nodes, verified, ascending by seq
    /// (Vec index == seq, guaranteed by causal delivery's contiguity).
    origins: BTreeMap<NodeId, Vec<VerifiedEntry>>,
    /// This node's causal version vector: for every node, the highest seq
    /// applied — including its own (`SPEC.md` §4.3, self-inclusive).
    causal_vv: VersionVector,
    /// Entries whose causal dependencies are not yet satisfied, keyed
    /// `(origin, seq)`. Bounded; oldest dropped on overflow
    /// (`DESIGN.md` D-013).
    buffer: BTreeMap<(NodeId, u64), BufferedEntry>,
    buffer_cap: usize,
    /// The task-claim CRDT, folded from every applied entry
    /// (`SPEC.md` §6.3). No cap of its own: bounded transitively by
    /// `log_cap` across the roster, exactly as `origins` is.
    claims: Claims,
    /// Proofs of equivocation this node has independently verified, keyed by
    /// the accused node. Bounded by roster size: at most one proof is kept
    /// per node, since one is already sufficient (`DESIGN.md` D-007).
    poes: BTreeMap<NodeId, Poe>,
    /// The escrow counter (`SPEC.md` §6.4, M5): per-node spending capped
    /// by a fixed mission-start allocation. Initialised empty — call
    /// [`Self::with_budgets`] to set allocations before the first Tick.
    escrow: Escrow,
}

impl State {
    /// Creates a node's initial state.
    ///
    /// # Panics
    ///
    /// If `me` is not a member of `roster`, or if `buffer_cap` is zero. A
    /// node absent from its own mission roster is a configuration error
    /// that would otherwise stay invisible: the node would run, verify
    /// entries against a roster it isn't in, and every peer would silently
    /// reject anything it sent. A zero-capacity buffer is likewise a
    /// configuration error: every structure in this system has a stated,
    /// usable bound (`DESIGN.md` D-013).
    pub fn new(
        me: NodeId,
        roster: Roster,
        key: SigningKey,
        log_cap: usize,
        buffer_cap: usize,
        entry_period: u64,
        anti_entropy_period: u64,
    ) -> Self {
        assert!(
            roster.key(me).is_some(),
            "node must be a member of its own roster"
        );
        assert!(buffer_cap >= 1, "buffer_cap must be at least 1");
        let members: Vec<NodeId> = roster.members().filter(|&n| n != me).collect();
        #[allow(unused_mut)]
        let mut claims = Claims::new();
        #[cfg(feature = "mutant-i3")]
        claims.set_owner(me);
        State {
            me,
            log: log::Log::new(me, key, log_cap),
            roster,
            members,
            entry_period,
            anti_entropy_period,
            recv_count: 0,
            sent_count: 0,
            origins: BTreeMap::new(),
            causal_vv: VersionVector::new(),
            buffer: BTreeMap::new(),
            buffer_cap,
            claims,
            poes: BTreeMap::new(),
            escrow: Escrow::new(BTreeMap::new()),
        }
    }

    /// Sets the per-node escrow allocations. Without this call a node has
    /// zero budget and will never author a `Spend` entry — a no-op that
    /// preserves backward compatibility with M2-M4 tests.
    pub fn with_budgets(mut self, budgets: BTreeMap<NodeId, u64>) -> Self {
        self.escrow = Escrow::new(budgets);
        self
    }

    /// This node's own signed chain.
    pub fn log(&self) -> &log::Log {
        &self.log
    }

    /// This node's causal version vector.
    pub fn causal_vv(&self) -> &VersionVector {
        &self.causal_vv
    }

    /// The task-claim CRDT as this node has derived it (`SPEC.md` §6.3). Two nodes holding the same entry set hold an identical one — that
    /// is invariant I3.
    pub fn claims(&self) -> &Claims {
        &self.claims
    }

    /// Keys currently held in the causal buffer, in no particular order
    /// beyond `BTreeMap`'s own (ascending by `(origin, seq)`).
    pub fn buffer_keys(&self) -> impl Iterator<Item = (NodeId, u64)> + '_ {
        self.buffer.keys().copied()
    }

    /// Every equivocation this node has independently verified proof of,
    /// ascending by the accused `NodeId` (`DESIGN.md` D-007).
    pub fn poes(&self) -> impl Iterator<Item = &Poe> + '_ {
        self.poes.values()
    }

    /// Whether this node holds a verified proof accusing `node`.
    pub fn is_proven_faulty(&self, node: NodeId) -> bool {
        self.poes.contains_key(&node)
    }

    /// The escrow counter as this node has derived it (`SPEC.md` §6.3, §6.4,
    /// M5). Two nodes holding the same entry set hold an identical one — that
    /// is invariant I3 and I4 together.
    pub fn escrow(&self) -> &Escrow {
        &self.escrow
    }

    /// Every entry this node has applied — its own log plus everything
    /// received — ascending by author then by seq.
    pub fn entries(&self) -> Vec<&wire::Entry> {
        let mut out = Vec::new();
        for node in self.roster.members() {
            if node == self.me {
                out.extend(self.log.entries().iter());
            } else if let Some(v) = self.origins.get(&node) {
                out.extend(v.iter().map(VerifiedEntry::entry));
            }
        }
        out
    }

    /// This node's own view of the mission, as a [`bundle::LogBundle`]
    /// (`SPEC.md` §4.4): a single-observer bundle keyed at `self.me`,
    /// reading only `log` and `origins` — nothing derived (`claims`,
    /// `escrow`, `causal_vv`) is exported, because the verifier rebuilds all
    /// of it from the raw entries itself.
    pub fn export_bundle(&self) -> bundle::LogBundle {
        let mut chains: BTreeMap<NodeId, Vec<wire::Entry>> = BTreeMap::new();
        for node in self.roster.members() {
            let author_entries: Vec<wire::Entry> = if node == self.me {
                self.log.entries().to_vec()
            } else if let Some(v) = self.origins.get(&node) {
                v.iter().map(|e| e.entry().clone()).collect()
            } else {
                Vec::new()
            };
            if !author_entries.is_empty() {
                chains.insert(node, author_entries);
            }
        }
        let mut views = BTreeMap::new();
        views.insert(self.me, chains);
        bundle::LogBundle {
            mission_id: self.roster.mission_id,
            epoch: self.roster.epoch,
            views,
        }
    }
}

/// The chain-hash a node would expect as `prev` on the next entry it applies
/// from `origin`: the last one it already holds, or `Hash::ZERO` if none.
fn expected_prev(state: &State, origin: NodeId) -> Hash {
    state
        .origins
        .get(&origin)
        .and_then(|v| v.last())
        .map_or(Hash::ZERO, |v| v.entry().chain_hash())
}

/// `true` if `state` has already applied `(node, seq)` or something newer
/// from `node` — the duplicate/already-known branch of causal delivery
/// (`SPEC.md` §4.3).
fn already_known(state: &State, node: NodeId, seq: u64) -> bool {
    state.causal_vv.highest(node).is_some_and(|k| k >= seq)
}

/// The entry this node holds at `(node, seq)`, wherever it currently lives:
/// its own log, an already-applied origin, or the not-yet-satisfied causal
/// buffer (`DESIGN.md` D-007). Equivocation detection must see all
/// three, since a conflicting entry can arrive while the first copy is still
/// sitting unapplied in the buffer.
fn held_at(state: &State, node: NodeId, seq: u64) -> Option<wire::Entry> {
    if node == state.me {
        return state.log.entries().get(seq as usize).cloned();
    }
    if let Some(e) = state
        .origins
        .get(&node)
        .and_then(|v| v.get(seq as usize))
        .map(VerifiedEntry::entry)
    {
        return Some(e.clone());
    }
    state.buffer.get(&(node, seq)).map(|b| b.entry.clone())
}

/// Checks an incoming entry against whatever this node already holds at the
/// same `(node, seq)`. If the two conflict and both are validly signed under
/// the roster, records a proof — self-verifying, needing nothing beyond the
/// roster (`DESIGN.md` D-007).
///
/// Only the first proof per accused node is kept: one is already sufficient,
/// and keeping more would grow `poes` without bound over a long run.
fn detect_equivocation(state: &mut State, incoming: &wire::Entry) {
    if state.poes.contains_key(&incoming.node) {
        return;
    }
    let Some(existing) = held_at(state, incoming.node, incoming.seq) else {
        return;
    };
    let Some(poe) = Poe::new(existing, incoming.clone()) else {
        return;
    };
    if verify_poe(&state.roster, &poe).is_ok() {
        state.poes.insert(poe.node(), poe);
    }
}

/// Verifies and applies an entry whose `deps` are already satisfied.
/// Returns whether it was applied. A verification failure is dropped
/// silently — defensive only; the honest M2 simulator never triggers it.
fn attempt_apply(state: &mut State, entry: wire::Entry) -> bool {
    let prev = expected_prev(state, entry.node);
    let expected_seq = state.causal_vv.highest(entry.node).map_or(0, |s| s + 1);
    match log::verify_next(&state.roster, 0, expected_seq, prev, &entry) {
        Ok(verified) => {
            state.causal_vv.bump(entry.node, entry.seq);
            state.claims.observe(&verified);
            state.escrow.observe(&verified);
            state.origins.entry(entry.node).or_default().push(verified);
            true
        }
        Err(_) => false,
    }
}

/// The task this node claims next: the number of claims already in its own
/// log, so every node walks `0, 1, 2, …` and every task is therefore
/// contested by every node (`SPEC.md` §6.3).
///
/// Derived rather than counted in a field on purpose: a separate counter
/// could drift out of step with the log after a refused append, and a derived
/// value cannot. The scan is over a `log_cap`-bounded vector once per
/// `entry_period` — not optimised at Phase 1 scale.
fn next_task(state: &State) -> TaskId {
    state
        .log
        .entries()
        .iter()
        .filter(|e| matches!(e.body, Body::TaskClaim { .. }))
        .count() as TaskId
}

/// Authors a `Withdraw` for every task this node claimed, is not winning, and
/// has not already withdrawn from — ascending by task id (`SPEC.md`
/// §6.3). By the same section, losing is monotone, so each fires at most once per task.
///
/// "Have I already withdrawn?" is asked of the derived state rather than of
/// the raw log: the only way `(task, me)` enters `withdrawn` is an entry
/// authored by `me`, and [`policy::author_and_commit`] folds those in as it
/// writes them, so the two readings are the same and this one does not rescan
/// the chain.
fn author_withdrawals(state: &mut State, effects: &mut Vec<Effect>) {
    let me = state.me;
    let pending: Vec<TaskId> = state
        .claims
        .tasks()
        .filter(|&task| {
            state.claims.has_claimed(task, me)
                && !state.claims.has_withdrawn(task, me)
                && state.claims.winner(task).is_some_and(|w| w.node != me)
        })
        .collect();

    for task in pending {
        policy::author_and_commit(state, &policy::Withdraw { task }, &(), effects);
    }
}

/// Rescans the causal buffer to a fixed point: repeatedly applies any
/// buffered entry whose `deps` are now satisfied, restarting after each
/// success (an apply can unblock others), until one full pass finds nothing
/// more (`SPEC.md` §4.3).
fn drain_buffer(state: &mut State) {
    loop {
        let ready = state
            .buffer
            .iter()
            .find(|(_, b)| b.entry.deps.le(&state.causal_vv))
            .map(|(&k, _)| k);
        let Some(key) = ready else { break };
        let buffered = state.buffer.remove(&key).expect("key found above");
        attempt_apply(state, buffered.entry);
    }
}

/// Inserts an unsatisfied entry into the bounded causal buffer, evicting the
/// oldest — smallest `(inserted_at, origin, seq)` — entry if full
/// (`DESIGN.md` D-013). A re-arriving entry for a key already buffered is
/// a no-op: the existing copy (and its `inserted_at`) is kept.
fn buffer_insert(state: &mut State, key: (NodeId, u64), now: u64, entry: wire::Entry) {
    if state.buffer.contains_key(&key) {
        return;
    }
    if state.buffer.len() == state.buffer_cap {
        let evict = state
            .buffer
            .iter()
            .map(|(&(origin, seq), b)| (b.inserted_at, origin, seq))
            .min()
            .map(|(_, origin, seq)| (origin, seq))
            .expect("buffer_cap >= 1 and len == cap implies non-empty");
        state.buffer.remove(&evict);
    }
    state.buffer.insert(
        key,
        BufferedEntry {
            inserted_at: now,
            entry,
        },
    );
}

/// The entry this node holds at `(origin, seq)`, if any — used to answer an
/// anti-entropy request.
fn entry_at(state: &State, origin: NodeId, seq: u64) -> Option<&wire::Entry> {
    if origin == state.me {
        state.log.entries().get(seq as usize)
    } else {
        state
            .origins
            .get(&origin)
            .and_then(|v| v.get(seq as usize))
            .map(VerifiedEntry::entry)
    }
}

/// The one function. Verbatim from `DESIGN.md` D-002.
///
/// Takes `&State` and returns a new `State` rather than mutating in place. That
/// costs a clone per event, and the cost is accepted on purpose: this is the shape
/// a folding scheme's step function has (`z_{i+1} = F(z_i, w_i)`), and Phase 4's
/// claim that `swarm-verify`'s replay becomes a proof without rewriting the circuit
/// depends on the signature already being this. `DESIGN.md` D-002, D-011 records
/// the cost and the conditions for revisiting it.
///
/// Determinism: the returned effects are in a fixed order, and the function reads
/// nothing outside its arguments.
pub fn step(state: &State, ev: Event, now: LogicalTime) -> (State, Vec<Effect>) {
    let mut next = state.clone();
    let mut effects = Vec::new();

    match ev {
        Event::Recv { from, payload } => {
            next.recv_count += 1;
            match payload {
                Envelope::Entry(entry) => {
                    // Equivocation detection runs before delivery decides what
                    // to do with the entry: a conflicting copy of an
                    // already-applied, already-buffered, or brand-new
                    // `(node, seq)` must be caught in all three cases
                    // (`DESIGN.md` D-007).
                    detect_equivocation(&mut next, &entry);
                    if already_known(&next, entry.node, entry.seq) {
                        // Duplicate or already superseded — honest re-delivery
                        // (e.g. via anti-entropy) is expected traffic, not an
                        // error (`SPEC.md` §6.1).
                    } else if entry.deps.le(&next.causal_vv) {
                        if attempt_apply(&mut next, entry) {
                            drain_buffer(&mut next);
                        }
                    } else {
                        let key = (entry.node, entry.seq);
                        buffer_insert(&mut next, key, now.0, entry);
                    }
                }
                Envelope::AntiEntropy(their_vv) => {
                    // Ascending by origin, then ascending by seq within
                    // each origin — advertise-then-push-reply, one round
                    // trip, no separate request envelope.
                    //
                    // `start` overlaps by one with what the peer already
                    // claims to have (`their_vv.highest`, not `+ 1`): a
                    // version vector counts entries, it does not identify
                    // them, so a peer's claimed head is re-sent every round
                    // rather than only the entries strictly past it. This is
                    // what lets equivocation be detected across a partition
                    // heal even when neither side's vector shows a gap
                    // (`DESIGN.md` D-007).
                    //
                    // Clamped to `mine`: if the peer's claimed highest for
                    // `origin` is already past what this node itself holds
                    // (this node is the one behind, not ahead — exactly a
                    // victim of equivocation stuck at the fork point while
                    // the peer has kept advancing past it), `start` must not
                    // exceed `mine`, or the range is empty and this node's
                    // own highest entry — the one that might conflict with
                    // what the peer holds — is never re-offered at all.
                    for (origin, mine) in next.causal_vv.iter() {
                        let start = their_vv.highest(origin).unwrap_or(0).min(mine);
                        for seq in start..=mine {
                            if let Some(e) = entry_at(&next, origin, seq) {
                                effects.push(Effect::Send {
                                    to: from,
                                    payload: Envelope::Entry(e.clone()),
                                });
                            }
                        }
                    }
                }
            }
        }
        Event::Tick => {
            // All entry authorship happens here and never in `Recv`: a node
            // that authored while draining its causal buffer would
            // interleave authorship with the fixed-point drain (`SPEC.md`
            // §6.2). The order within the tick is
            // normative — claim, then withdrawals, then the advertisement.
            if next.entry_period != 0 && now.0.is_multiple_of(next.entry_period) {
                let claim = policy::TaskClaim {
                    task: next_task(&next),
                    // Fixed for autonomously authored claims: nothing in
                    // Phase 1 produces real priorities, and an unused
                    // configuration knob is avoided on principle.
                    priority: 1,
                };
                policy::author_and_commit(&mut next, &claim, &(), &mut effects);
                author_withdrawals(&mut next, &mut effects);
                // M5: spend 1 unit per authoring tick while budget remains.
                // can_spend is a local check — no consensus, no handshake.
                // The per-node cap is what makes I4 hold even in a partition
                // (`SPEC.md` §6.4).
                if next.escrow.can_spend(next.me, 1) {
                    policy::author_and_commit(
                        &mut next,
                        &policy::Spend { amount: 1 },
                        &(),
                        &mut effects,
                    );
                }
            }
            if next.anti_entropy_period != 0 && now.0.is_multiple_of(next.anti_entropy_period) {
                for &peer in &next.members {
                    effects.push(Effect::Send {
                        to: peer,
                        payload: Envelope::AntiEntropy(next.causal_vv.clone()),
                    });
                }
            }
        }
    }

    next.sent_count += effects.len() as u64;
    (next, effects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        let mut bytes = [0u8; 32];
        bytes[0] = seed;
        SigningKey::from_bytes(&bytes)
    }

    /// A 3-node roster (`A=0, B=1, C=2`) and the keys behind it.
    fn roster3() -> (Roster, [SigningKey; 3]) {
        let keys = [key(1), key(2), key(3)];
        let mut m = alloc::collections::BTreeMap::new();
        for (i, k) in keys.iter().enumerate() {
            m.insert(NodeId(i as u8), k.verifying_key());
        }
        (
            Roster::new(wire::PHASE1_MISSION_ID, wire::PHASE1_EPOCH, m),
            keys,
        )
    }

    fn state(me: NodeId, roster: &Roster, k: &SigningKey, entry_period: u64) -> State {
        State::new(me, roster.clone(), k.clone(), 64, 8, entry_period, 0)
    }

    #[test]
    #[should_panic(expected = "own roster")]
    fn node_missing_from_its_own_roster_is_rejected() {
        let (roster, keys) = roster3();
        let mut m = alloc::collections::BTreeMap::new();
        m.insert(NodeId(0), keys[0].verifying_key());
        m.insert(NodeId(1), keys[1].verifying_key());
        let partial = Roster::new(roster.mission_id, roster.epoch, m);
        State::new(NodeId(9), partial, key(9), 64, 8, 10, 0);
    }

    #[test]
    #[should_panic(expected = "buffer_cap must be at least 1")]
    fn zero_buffer_cap_is_rejected() {
        let (roster, keys) = roster3();
        State::new(NodeId(0), roster, keys[0].clone(), 64, 0, 10, 0);
    }

    #[test]
    fn entry_created_and_broadcast_on_period() {
        let (roster, keys) = roster3();
        let s = state(NodeId(0), &roster, &keys[0], 10);
        let (s2, fx) = step(&s, Event::Tick, LogicalTime(10));

        assert_eq!(s2.log().len(), 1);
        assert_eq!(s2.causal_vv().highest(NodeId(0)), Some(0));
        let dests: Vec<NodeId> = fx.iter().map(|Effect::Send { to, .. }| *to).collect();
        // Never to itself, ascending NodeId (rule R4).
        assert_eq!(dests, [NodeId(1), NodeId(2)]);
        assert!(fx
            .iter()
            .all(|Effect::Send { payload, .. }| matches!(payload, Envelope::Entry(_))));
    }

    #[test]
    fn entry_creation_is_silent_off_period() {
        let (roster, keys) = roster3();
        let s = state(NodeId(0), &roster, &keys[0], 10);
        let (s2, fx) = step(&s, Event::Tick, LogicalTime(11));
        assert!(fx.is_empty());
        assert_eq!(s2.log().len(), 0);
    }

    #[test]
    fn an_entry_with_unsatisfied_deps_is_buffered_not_applied() {
        let (roster, keys) = roster3();
        let a = state(NodeId(0), &roster, &keys[0], 10);
        let (a1, fx) = step(&a, Event::Tick, LogicalTime(10));
        assert_eq!(fx.len(), 2);
        let Effect::Send { payload, .. } = fx[0].clone();
        let Envelope::Entry(first) = payload else {
            panic!("expected an entry")
        };

        // C never saw A's genesis entry; build A's *second* entry directly so
        // its `deps` names A's seq 0, which C does not have.
        let mut a2_log = a1.log().clone();
        let second = a2_log
            .append(
                Body::TaskClaim {
                    task: 1,
                    priority: 1,
                },
                {
                    let mut vv = VersionVector::new();
                    vv.bump(NodeId(0), 0);
                    vv
                },
            )
            .unwrap()
            .clone();
        let _ = first; // the genesis entry itself is not delivered to C below

        let c = state(NodeId(2), &roster, &keys[2], 0);
        let (c1, fx) = step(
            &c,
            Event::Recv {
                from: NodeId(0),
                payload: Envelope::Entry(second),
            },
            LogicalTime(11),
        );
        assert!(fx.is_empty());
        assert_eq!(c1.causal_vv().highest(NodeId(0)), None, "not applied");
        assert_eq!(c1.buffer_keys().count(), 1, "buffered instead");
    }

    #[test]
    fn a_satisfied_entry_applies_and_advances_causal_vv() {
        let (roster, keys) = roster3();
        let a = state(NodeId(0), &roster, &keys[0], 10);
        let (a1, fx) = step(&a, Event::Tick, LogicalTime(10));
        let Effect::Send { payload, .. } = fx[0].clone();
        let Envelope::Entry(first) = payload else {
            panic!("expected an entry")
        };
        let _ = a1;

        let c = state(NodeId(2), &roster, &keys[2], 0);
        let (c1, fx) = step(
            &c,
            Event::Recv {
                from: NodeId(0),
                payload: Envelope::Entry(first),
            },
            LogicalTime(11),
        );
        assert!(fx.is_empty());
        assert_eq!(c1.causal_vv().highest(NodeId(0)), Some(0));
        assert_eq!(c1.entries().len(), 1);
        assert!(c1.buffer_keys().count() == 0);
    }

    #[test]
    fn anti_entropy_reply_carries_exactly_the_missing_entries() {
        let (roster, keys) = roster3();
        let a = state(NodeId(0), &roster, &keys[0], 10);
        let (a1, _) = step(&a, Event::Tick, LogicalTime(10));
        let (a2, _) = step(&a1, Event::Tick, LogicalTime(20));
        assert_eq!(a2.log().len(), 2);

        // B advertises an empty VV: it has nothing from A.
        let b = state(NodeId(1), &roster, &keys[1], 0);
        let (_, fx) = step(
            &a2,
            Event::Recv {
                from: NodeId(1),
                payload: Envelope::AntiEntropy(b.causal_vv().clone()),
            },
            LogicalTime(21),
        );

        let seqs: Vec<u64> = fx
            .iter()
            .map(|Effect::Send { payload, .. }| match payload {
                Envelope::Entry(e) => e.seq,
                Envelope::AntiEntropy(_) => panic!("expected entries only"),
            })
            .collect();
        assert_eq!(seqs, [0, 1]);
        assert!(fx.iter().all(|Effect::Send { to, .. }| *to == NodeId(1)));
    }

    #[test]
    fn a_duplicate_delivery_is_a_no_op() {
        let (roster, keys) = roster3();
        let a = state(NodeId(0), &roster, &keys[0], 10);
        let (a1, fx) = step(&a, Event::Tick, LogicalTime(10));
        let Effect::Send { payload, .. } = fx[0].clone();
        let Envelope::Entry(first) = payload else {
            panic!("expected an entry")
        };
        let _ = a1;

        let c = state(NodeId(2), &roster, &keys[2], 0);
        let (c1, _) = step(
            &c,
            Event::Recv {
                from: NodeId(0),
                payload: Envelope::Entry(first.clone()),
            },
            LogicalTime(11),
        );
        let (c2, fx) = step(
            &c1,
            Event::Recv {
                from: NodeId(0),
                payload: Envelope::Entry(first),
            },
            LogicalTime(12),
        );
        assert!(fx.is_empty());
        // `recv_count` legitimately differs (a duplicate is still received);
        // everything the entry could have changed must not.
        assert_eq!(c1.causal_vv(), c2.causal_vv());
        assert_eq!(c1.entries(), c2.entries());
        assert_eq!(c1.buffer_keys().count(), c2.buffer_keys().count());
    }

    // -----------------------------------------------------------------
    // M3 authoring rules (`SPEC.md` §6.3)
    // -----------------------------------------------------------------

    /// Extracts the entries a `step` broadcast, deduplicated: the same entry
    /// goes to every peer, and what matters here is *which* entries were
    /// authored, not how many peers heard them.
    fn authored(fx: &[Effect]) -> Vec<wire::Entry> {
        let mut out: Vec<wire::Entry> = Vec::new();
        for Effect::Send { payload, .. } in fx {
            if let Envelope::Entry(e) = payload {
                if !out.iter().any(|k| k.seq == e.seq && k.node == e.node) {
                    out.push(e.clone());
                }
            }
        }
        out
    }

    #[test]
    fn a_node_claims_tasks_zero_one_two_in_order() {
        // Every node walks the same task numbering, which is what makes every
        // task contested (`SPEC.md` §6.3) — M2 numbered tasks by the
        // author's own seq, so nothing ever collided.
        let (roster, keys) = roster3();
        let mut s = state(NodeId(0), &roster, &keys[0], 10);
        for expected in 0..3u64 {
            let (next, fx) = step(&s, Event::Tick, LogicalTime((expected + 1) * 10));
            let bodies: Vec<Body> = authored(&fx).iter().map(|e| e.body).collect();
            assert_eq!(
                bodies,
                [Body::TaskClaim {
                    task: expected,
                    priority: 1
                }]
            );
            s = next;
        }
        assert_eq!(s.claims().tasks().collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn the_sole_claimant_wins_and_never_withdraws() {
        let (roster, keys) = roster3();
        let s = state(NodeId(0), &roster, &keys[0], 10);
        let (s, _) = step(&s, Event::Tick, LogicalTime(10));
        assert_eq!(s.claims().winner(0).map(|w| w.node), Some(NodeId(0)));

        let (s2, fx) = step(&s, Event::Tick, LogicalTime(20));
        let bodies: Vec<Body> = authored(&fx).iter().map(|e| e.body).collect();
        assert_eq!(
            bodies,
            [Body::TaskClaim {
                task: 1,
                priority: 1
            }],
            "the winner claims the next task and writes no withdrawal"
        );
        assert!(!s2.claims().has_withdrawn(0, NodeId(0)));
    }

    #[test]
    fn a_losing_node_withdraws_exactly_once_on_the_next_period() {
        let (roster, keys) = roster3();

        // A claims task 0 first, with an empty `deps` — lc 0, the strongest
        // possible clock (`SPEC.md` §6.3).
        let a = state(NodeId(0), &roster, &keys[0], 10);
        let (_, a_fx) = step(&a, Event::Tick, LogicalTime(10));
        let a0 = authored(&a_fx).remove(0);

        // C claims task 0 too, then hears A's claim and loses: A's lc is 0
        // and its NodeId is lower.
        let c = state(NodeId(2), &roster, &keys[2], 10);
        let (c, _) = step(&c, Event::Tick, LogicalTime(10));
        assert_eq!(c.claims().winner(0).map(|w| w.node), Some(NodeId(2)));

        let (c, fx) = step(
            &c,
            Event::Recv {
                from: NodeId(0),
                payload: Envelope::Entry(a0),
            },
            LogicalTime(11),
        );
        assert_eq!(c.claims().winner(0).map(|w| w.node), Some(NodeId(0)));
        assert!(
            fx.is_empty(),
            "authorship never happens in Recv, only in Tick"
        );
        assert!(!c.claims().has_withdrawn(0, NodeId(2)), "not yet — on Tick");

        // The next entry_period: C claims task 1 *and* withdraws from task 0,
        // in that order.
        let (c, fx) = step(&c, Event::Tick, LogicalTime(20));
        let bodies: Vec<Body> = authored(&fx).iter().map(|e| e.body).collect();
        assert_eq!(
            bodies,
            [
                Body::TaskClaim {
                    task: 1,
                    priority: 1
                },
                Body::Withdraw { task: 0 },
            ]
        );
        assert!(c.claims().has_withdrawn(0, NodeId(2)));

        // And never again: losing is monotone, so the withdrawal is final
        // (`SPEC.md` §6.3).
        let (_, fx) = step(&c, Event::Tick, LogicalTime(30));
        let bodies: Vec<Body> = authored(&fx).iter().map(|e| e.body).collect();
        assert_eq!(
            bodies,
            [Body::TaskClaim {
                task: 2,
                priority: 1
            }],
            "task 0 must not be withdrawn from twice"
        );
    }

    #[test]
    fn step_is_pure_and_reproducible() {
        let (roster, keys) = roster3();
        let s = state(NodeId(0), &roster, &keys[0], 10);
        let (a, fx_a) = step(&s, Event::Tick, LogicalTime(10));
        let (b, fx_b) = step(&s, Event::Tick, LogicalTime(10));

        // Same input, same output — and the input is untouched.
        assert_eq!(a, b);
        assert_eq!(fx_a, fx_b);
        assert_eq!(s.sent_count, 0);
    }
}
