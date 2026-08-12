//! The policy gate (`DESIGN.md` D-005): every effect must pass through
//! [`commit`], which checks the action's consistency class. No effect exists
//! outside this path — that is invariant I6, structurally discharged.
//!
//! # Phase 1 scope
//!
//! Only [`Class::Degradable`] actions exist — no type in this crate
//! implements [`Action`] with [`Class::ExclusiveCostly`] or
//! [`Class::SafetyCritical`] (`SPEC.md` §6.5). [`commit`] cannot
//! be called on an action that does not exist; that is I5, structurally
//! discharged, and the `compile_fail` doctest on [`SafetyCriticalAction`]
//! is its concrete proof. The certificate types Phase 2 will need
//! (`QuorumCert`, `OperatorSig`, `GlobalThresholdCert`) are not named yet —
//! a variant is not added before something uses it.

use alloc::vec::Vec;

use crate::state::TaskId;
use crate::wire::Body;
use crate::{Effect, Envelope, State};

/// Consistency class of an action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// Local decision; CRDT converges. `Cert = ()` — no consensus needed.
    Degradable,
    /// Consumable resource; partition-internal quorum, leaderless, 1 RTT.
    /// Activates in Phase 2.
    ExclusiveCostly,
    /// Engagement authority; operator signature or global threshold cert.
    /// Activates in Phase 2.
    SafetyCritical,
}

/// An action that may produce effects, gated by its [`Class`] and its
/// associated `Cert` type.
pub trait Action {
    const CLASS: Class;
    type Cert;
    fn body(&self) -> Body;
}

// ---------------------------------------------------------------------------
// Phase 1 concrete actions — all Degradable, all Cert = ()
// ---------------------------------------------------------------------------

/// Claims a task for this node. Converges via the task-claim CRDT
/// (`SPEC.md` §6.3); no certificate needed.
pub struct TaskClaim {
    pub task: TaskId,
    pub priority: u8,
}

impl Action for TaskClaim {
    const CLASS: Class = Class::Degradable;
    type Cert = ();
    fn body(&self) -> Body {
        Body::TaskClaim {
            task: self.task,
            priority: self.priority,
        }
    }
}

/// Withdraws from a task this node previously claimed. Converges via the
/// task-claim CRDT; no certificate needed.
pub struct Withdraw {
    pub task: TaskId,
}

impl Action for Withdraw {
    const CLASS: Class = Class::Degradable;
    type Cert = ();
    fn body(&self) -> Body {
        Body::Withdraw { task: self.task }
    }
}

/// Spends one unit of this node's escrow budget. The per-node cap makes I4
/// structural (`SPEC.md` §6.4); no certificate needed.
pub struct Spend {
    pub amount: u64,
}

impl Action for Spend {
    const CLASS: Class = Class::Degradable;
    type Cert = ();
    fn body(&self) -> Body {
        Body::Spend {
            amount: self.amount,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 2 stub — no certificate type exists yet, nothing implements it
// ---------------------------------------------------------------------------

/// An action that requires operator or threshold certification.
/// Stub only — not implemented in Phase 1.
///
/// I5, made structural rather than executable
/// (`SPEC.md` §6.5): `SafetyCriticalAction` does not implement
/// [`Action`], so [`commit`] cannot be called on it — there is no `Cert`
/// type to supply. A safety-critical effect without a valid certificate is
/// therefore not a runtime state this program can reach; it is a program
/// `rustc` refuses to compile.
///
/// ```compile_fail
/// use std::collections::BTreeMap;
/// use ed25519_dalek::SigningKey;
/// use swarm_core::policy::{commit, SafetyCriticalAction};
/// use swarm_core::wire::{Body, Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
/// use swarm_core::{NodeId, State};
///
/// let key = SigningKey::from_bytes(&[1u8; 32]);
/// let mut keys = BTreeMap::new();
/// let a = NodeId(0);
/// keys.insert(a, key.verifying_key());
/// let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);
/// let state = State::new(a, roster, key, 64, 8, 10, 0);
///
/// let action = SafetyCriticalAction { body: Body::Spend { amount: 1 } };
/// // Does not compile: `SafetyCriticalAction` has no `Action` impl, so
/// // there is no `Cert` type and therefore no way to call `commit`.
/// let _ = commit(&state, &action, &());
/// ```
pub struct SafetyCriticalAction {
    pub body: Body,
}

// Intentionally NOT implemented for Phase 1:
// impl Action for SafetyCriticalAction { ... Cert = OperatorSig | GlobalThresholdCert ... }

// ---------------------------------------------------------------------------
// The gate — the single path through which entries produce effects (I6)
// ---------------------------------------------------------------------------

/// Authorises an action for effect emission: `true` if effects should be
/// emitted, `false` if the action is refused.
///
/// For [`Class::Degradable`] actions, always passes — the CRDT converges
/// without a certificate. For [`Class::ExclusiveCostly`] and
/// [`Class::SafetyCritical`], the certificate would be checked here (Phase 2);
/// neither arm is reachable in Phase 1, since no `Action` impl has that
/// `CLASS` (I5).
///
/// This is the **only** function that authorises effect emission. Every
/// `Effect::Send` traces back through this call to a signed entry — that is
/// I6.
pub fn commit<A: Action>(_state: &State, _action: &A, _cert: &A::Cert) -> bool {
    match A::CLASS {
        Class::Degradable => true,
        Class::ExclusiveCostly => {
            // Phase 2: verify QuorumCert. Unreachable in Phase 1 — no
            // ExclusiveCostly-class `Action` impl exists.
            true
        }
        Class::SafetyCritical => {
            // Unreachable in Phase 1 — no SafetyCritical-class `Action`
            // impl exists (I5; see `SafetyCriticalAction`'s doctest).
            false
        }
    }
}

/// Authors an entry and, if the policy gate passes, broadcasts it.
///
/// The entry is always appended to the log and folded into local state. The
/// gate only controls whether effects are emitted — the record always stays
/// in the chain (traceability, I6).
pub fn author_and_commit<A: Action>(
    state: &mut State,
    action: &A,
    cert: &A::Cert,
    effects: &mut Vec<Effect>,
) {
    let deps = state.causal_vv.clone();
    let Ok(appended) = state.log.append(action.body(), deps) else {
        return;
    };
    let entry = appended.clone();
    state.causal_vv.bump(state.me, entry.seq);

    state
        .claims
        .observe(&crate::wire::VerifiedEntry::from_verified(entry.clone()));
    state
        .escrow
        .observe(&crate::wire::VerifiedEntry::from_verified(entry.clone()));

    if commit(state, action, cert) {
        for &peer in &state.members {
            effects.push(Effect::Send {
                to: peer,
                payload: Envelope::Entry(entry.clone()),
            });
        }
    }
    // If `commit` refuses, the entry still stays in the log (traceability) —
    // just no effect is emitted; the action is recorded as refused
    // (`DESIGN.md` D-005).
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I6: `commit` is the only function in this module that gates effect
    /// emission. `author_and_commit` produces effects ONLY through `commit`.
    #[test]
    fn i6_commit_is_the_single_effect_gate() {
        use crate::wire::{Roster, PHASE1_EPOCH, PHASE1_MISSION_ID};
        use crate::NodeId;
        use crate::State;
        use alloc::collections::BTreeMap;
        use ed25519_dalek::SigningKey;

        let key = SigningKey::from_bytes(&[1u8; 32]);
        let mut keys = BTreeMap::new();
        let a = NodeId(0);
        let b = NodeId(1);
        keys.insert(a, key.verifying_key());
        keys.insert(b, SigningKey::from_bytes(&[2u8; 32]).verifying_key());
        let roster = Roster::new(PHASE1_MISSION_ID, PHASE1_EPOCH, keys);

        let mut s = State::new(a, roster, key.clone(), 64, 8, 10, 0);
        let action = TaskClaim {
            task: 0,
            priority: 1,
        };
        let mut fx = Vec::new();
        author_and_commit(&mut s, &action, &(), &mut fx);

        // Degradable actions always pass the gate.
        assert_eq!(fx.len(), 1);
        assert_eq!(s.log.len(), 1);
        assert_eq!(s.causal_vv.highest(a), Some(0));
    }
}
