//! A seeded generator for tests.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use efr_stdx::rng::Rng;

/// The SplitMix64 increment, the fractional part of the golden ratio.
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// A deterministic [`Rng`]: the same seed gives the same bytes on every run and every
/// machine, so ids, PKCE verifiers and scratch names in a test are the same each time
/// and transcripts can name them.
///
/// The algorithm is SplitMix64, written out here instead of taken from `rand`, because
/// `rand` does not promise that a seeded generator keeps its output across versions,
/// and a changed sequence would break every fixture that holds a generated id. This
/// sequence is frozen: a test pins its first values.
///
/// [`Rng::fill_bytes`] draws whole 64-bit values and writes each in little-endian
/// order; the unused bytes of the last value are dropped, so a call of 10 bytes uses
/// two values. Clones are not offered; share one generator as `Arc<dyn Rng>`.
pub struct TestRng {
    state: AtomicU64,
}

impl TestRng {
    /// A generator with `seed`.
    pub fn new(seed: u64) -> Self {
        TestRng { state: AtomicU64::new(seed) }
    }
}

impl Rng for TestRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }

    fn next_u64(&self) -> u64 {
        // The SplitMix64 state only ever grows by GAMMA, so one atomic add advances it
        // without a lock, and concurrent draws still each get a distinct value.
        let state = self.state.fetch_add(GAMMA, Ordering::Relaxed).wrapping_add(GAMMA);
        mix(state)
    }
}

impl fmt::Debug for TestRng {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestRng").field("state", &self.state.load(Ordering::Relaxed)).finish()
    }
}

/// The SplitMix64 output function.
fn mix(state: u64) -> u64 {
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests;
