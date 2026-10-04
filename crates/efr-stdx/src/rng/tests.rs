use std::sync::Arc;

use pretty_assertions::{assert_eq, assert_ne};

use super::{Rng, SystemRng};

/// Fills every buffer with 1, 2, 3, and so on.
#[derive(Debug)]
struct CountingRng;

impl Rng for CountingRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        for (byte, value) in dest.iter_mut().zip(1..) {
            *byte = value;
        }
    }
}

#[test]
fn next_u64_reads_eight_little_endian_bytes() {
    assert_eq!(CountingRng.next_u64(), u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
}

#[test]
fn forwarding_rngs_use_the_inner_rng() {
    fn draw<R: Rng>(rng: R) -> u64 {
        rng.next_u64()
    }
    let shared: Arc<dyn Rng> = Arc::new(CountingRng);
    let expected = CountingRng.next_u64();
    assert_eq!(draw(&CountingRng), expected);
    assert_eq!(draw(shared), expected);
}

#[test]
fn system_rng_draws_differ() {
    let rng = SystemRng::new().unwrap();
    let mut first = [0; 32];
    let mut second = [0; 32];
    rng.fill_bytes(&mut first);
    rng.fill_bytes(&mut second);
    assert_ne!(first, second);
    assert_ne!(first, [0; 32]);
}

#[test]
fn system_rngs_are_seeded_independently() {
    let one = SystemRng::new().unwrap();
    let two = SystemRng::new().unwrap();
    let mut first = [0; 32];
    let mut second = [0; 32];
    one.fill_bytes(&mut first);
    two.fill_bytes(&mut second);
    assert_ne!(first, second);
}

#[test]
fn system_rng_fills_an_empty_buffer() {
    SystemRng::new().unwrap().fill_bytes(&mut []);
}

#[test]
fn system_rng_debug_hides_the_state() {
    assert_eq!(format!("{:?}", SystemRng::new().unwrap()), "SystemRng { .. }");
}
