use std::sync::Arc;

use efr_stdx::id::uuid_v7;
use efr_stdx::rng::Rng;
use pretty_assertions::{assert_eq, assert_ne};

use super::TestRng;
use crate::TestClock;

#[test]
fn the_sequence_is_frozen_splitmix64() {
    // The published SplitMix64 outputs for seed 0. A change here breaks every fixture
    // that holds a generated id.
    let rng = TestRng::new(0);
    let values: Vec<u64> = (0..4).map(|_| rng.next_u64()).collect();
    assert_eq!(
        values,
        [
            0xE220_A839_7B1D_CDAF,
            0x6E78_9E6A_A1B9_65F4,
            0x06C4_5D18_8009_454F,
            0xF88B_B8A8_724C_81EC
        ]
    );
}

#[test]
fn the_same_seed_gives_the_same_bytes() {
    let one = TestRng::new(42);
    let two = TestRng::new(42);
    let mut first = [0; 24];
    let mut second = [0; 24];
    one.fill_bytes(&mut first);
    two.fill_bytes(&mut second);
    assert_eq!(first, second);
}

#[test]
fn different_seeds_give_different_bytes() {
    assert_ne!(TestRng::new(1).next_u64(), TestRng::new(2).next_u64());
}

#[test]
fn fill_bytes_writes_whole_values_in_little_endian_order() {
    let rng = TestRng::new(42);
    let mut bytes = [0; 10];
    rng.fill_bytes(&mut bytes);
    let first = 0xBDD7_3226_2FEB_6E95_u64.to_le_bytes();
    let second = 0x28EF_E333_B266_F103_u64.to_le_bytes();
    assert_eq!(bytes[..8], first);
    assert_eq!(bytes[8..], second[..2]);
    // The rest of the second value is dropped, so the next draw is the third value.
    assert_eq!(rng.next_u64(), {
        let again = TestRng::new(42);
        again.next_u64();
        again.next_u64();
        again.next_u64()
    });
}

#[test]
fn an_empty_buffer_draws_nothing() {
    let rng = TestRng::new(7);
    rng.fill_bytes(&mut []);
    assert_eq!(rng.next_u64(), TestRng::new(7).next_u64());
}

#[test]
fn a_shared_generator_gives_ids_that_repeat_across_runs() {
    let clock = TestClock::new();
    let rng: Arc<dyn Rng> = Arc::new(TestRng::new(1));
    let first = uuid_v7(&clock, &*rng);
    let second = uuid_v7(&clock, &*rng);
    assert_eq!(first.to_string(), "01a106c9-0600-715c-8289-ec2d0a9167ec");
    assert_ne!(first, second);
    let again = TestRng::new(1);
    assert_eq!(uuid_v7(&clock, &again), first);
}

#[test]
fn debug_shows_the_state() {
    assert_eq!(format!("{:?}", TestRng::new(5)), "TestRng { state: 5 }");
}
