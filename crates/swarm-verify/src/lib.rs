//! Two independent checkers of the same predicate (I1–I4, `docs/spec.md`
//! §15), kept deliberately separate: [`oracle::check_invariants`] reads
//! live `State` from inside a simulation run and is not the normative
//! verifier; [`verify::verify`] judges a `LogBundle`/`Spec` pair with no
//! access to the process that produced them and is the normative surface
//! (`docs/spec.md` §20). See `oracle`'s module doc for why both exist.

pub mod fold;
pub mod oracle;
pub mod verdict;
pub mod verify;

pub use oracle::{check_invariants, Violation};
pub use verify::verify;
