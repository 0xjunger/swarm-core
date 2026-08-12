//! Seeded, reproducible randomness. The only source of randomness in the project.
//!
//! Note where this lives: in the simulator, never in `swarm-core`. `DESIGN.md`
//! D-002 requires randomness to be injected, and M0 goes further — the core does
//! not receive any.

use rand_chacha::ChaCha8Rng;
use rand_core::{RngCore, SeedableRng};

/// A deterministic random number generator.
///
/// Wraps `ChaCha8Rng`, whose output is guaranteed reproducible across releases of
/// the crate. `rand::rngs::StdRng` would not do: its documentation explicitly
/// disclaims value stability, so upgrading it could silently change every trace
/// this project has ever recorded without breaking the build.
///
/// Exposes only integer operations. No floating point appears anywhere in the
/// simulation model.
pub struct SimRng {
    inner: ChaCha8Rng,
}

impl SimRng {
    pub fn new(seed: u64) -> Self {
        Self {
            inner: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Draws the next value. Every draw advances the stream, which is why the
    /// determinism contract fixes *how many* draws happen per effect and in what
    /// order.
    pub fn next_u32(&mut self) -> u32 {
        self.inner.next_u32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let (mut a, mut b) = (SimRng::new(42), SimRng::new(42));
        let xs: Vec<u32> = (0..64).map(|_| a.next_u32()).collect();
        let ys: Vec<u32> = (0..64).map(|_| b.next_u32()).collect();
        assert_eq!(xs, ys);
    }

    #[test]
    fn different_seed_different_stream() {
        let (mut a, mut b) = (SimRng::new(42), SimRng::new(43));
        let xs: Vec<u32> = (0..64).map(|_| a.next_u32()).collect();
        let ys: Vec<u32> = (0..64).map(|_| b.next_u32()).collect();
        assert_ne!(xs, ys);
    }
}
