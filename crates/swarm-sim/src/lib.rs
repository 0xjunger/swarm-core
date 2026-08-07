//! A deterministic network simulator for `swarm-core`.
//!
//! No sockets, no threads, no async runtime, no wall clock. Time advances because
//! the loop in [`sim::run`] increments a counter, and randomness comes from a
//! single seeded stream. Two runs with the same configuration produce byte-identical
//! traces — which is M0's entire acceptance criterion (`DESIGN.md` §M0).
//!
//! `turmoil` and `madsim` were considered and rejected: both are built on tokio and
//! would force async through the whole project for no benefit at this scale.

#![forbid(unsafe_code)]

pub mod demo;
pub mod net;
pub mod partition;
pub mod rng;
pub mod sim;
pub mod trace;

pub use partition::Partition;
pub use sim::{run, run_with_states, SimConfig};
pub use trace::{Trace, TraceRecord};
