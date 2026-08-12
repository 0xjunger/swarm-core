//! Two independent checkers of the same predicate (I1–I4, `SPEC.md`
//! §6.1–§6.4), kept deliberately separate: [`oracle::check_invariants`] reads
//! live `State` from inside a simulation run and is not the normative
//! verifier; [`verify::verify`] judges a `LogBundle`/`Spec` pair with no
//! access to the process that produced them and is the normative surface
//! (`SPEC.md` §7.1). See `oracle`'s module doc for why both exist.

pub mod fold;
pub mod oracle;
pub mod verdict;
pub mod verify;

pub use oracle::{check_invariants, Violation};
pub use verify::verify;
