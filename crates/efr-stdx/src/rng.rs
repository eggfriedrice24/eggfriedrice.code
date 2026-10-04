//! Injected randomness.
//!
//! Anything that draws random bytes takes an [`Rng`], so a test can use a seeded
//! generator and get the same ids, PKCE verifiers and scratch names on every run.
//! [`SystemRng`] is the production generator.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use rand::rngs::{StdRng, SysRng};
use rand::{Rng as _, SeedableRng as _};

use crate::StdxError;

/// A source of random bytes.
///
/// The methods take `&self`, so one generator can be shared as `Arc<dyn Rng>`; an
/// implementation keeps its state behind a lock.
pub trait Rng: Send + Sync + fmt::Debug {
    /// Fills `dest` with random bytes.
    fn fill_bytes(&self, dest: &mut [u8]);

    /// A random `u64`: eight bytes from [`Rng::fill_bytes`] in little-endian order.
    fn next_u64(&self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }
}

impl<R: Rng + ?Sized> Rng for Arc<R> {
    fn fill_bytes(&self, dest: &mut [u8]) {
        (**self).fill_bytes(dest);
    }

    fn next_u64(&self) -> u64 {
        (**self).next_u64()
    }
}

impl<R: Rng + ?Sized> Rng for &R {
    fn fill_bytes(&self, dest: &mut [u8]) {
        (**self).fill_bytes(dest);
    }

    fn next_u64(&self) -> u64 {
        (**self).next_u64()
    }
}

/// The production generator: ChaCha12 (`rand`'s `StdRng`), seeded once from the
/// operating system.
///
/// ChaCha12 is a cryptographic generator, so the output is fit for PKCE verifiers and
/// OAuth `state` values. Seeding once makes every later draw infallible, where a read
/// of the operating system's source can fail on each call.
pub struct SystemRng {
    inner: Mutex<StdRng>,
}

impl SystemRng {
    /// A new generator, seeded from the operating system's random source.
    pub fn new() -> Result<Self, StdxError> {
        let rng = StdRng::try_from_rng(&mut SysRng)
            .map_err(|source| StdxError::SeedRng { source: source.into() })?;
        Ok(SystemRng { inner: Mutex::new(rng) })
    }
}

impl Rng for SystemRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        // A panic in another thread while it held the lock cannot leave the ChaCha
        // state unusable, so a poisoned lock still guards a good generator.
        let mut rng = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        rng.fill_bytes(dest);
    }
}

impl fmt::Debug for SystemRng {
    // The generator state is a secret: a reader who sees it can predict every later
    // output, including PKCE verifiers.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemRng").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
