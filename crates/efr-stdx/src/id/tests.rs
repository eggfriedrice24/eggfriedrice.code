use std::time::Duration;

use jiff::Timestamp;
use pretty_assertions::{assert_eq, assert_ne};
use proptest::prelude::{any, prop_assert, prop_assert_eq, proptest};
use uuid::{Uuid, Variant};

use super::{uuid_v7, uuid_v7_from};
use crate::rng::Rng;
use crate::time::{Clock, Sleep};

/// The latest millisecond that `Timestamp` can hold: 9999-12-30T22:00:00.999Z. jiff
/// stops short of the end of the year so that every offset still gives a valid date.
const MAX_MILLIS: i64 = 253_402_207_200_999;

/// The largest gap between two ids in the ordering property.
const MAX_GAP: i64 = 1_000_000;

#[derive(Debug)]
struct FixedClock(Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(std::future::pending())
    }
}

/// Fills every buffer with one byte value.
#[derive(Debug)]
struct ByteRng(u8);

impl Rng for ByteRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        dest.fill(self.0);
    }
}

fn millis_of(id: Uuid) -> u64 {
    let b = id.as_bytes();
    u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]])
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millisecond(millis).unwrap()
}

#[test]
fn id_is_version_7_with_the_rfc_variant() {
    let id = uuid_v7(&FixedClock(at(1_759_500_000_000)), &ByteRng(0xff));
    assert_eq!(id.get_version_num(), 7);
    assert_eq!(id.get_variant(), Variant::RFC4122);
}

#[test]
fn id_starts_with_the_clock_millisecond() {
    let id = uuid_v7(&FixedClock(at(1_759_500_000_123)), &ByteRng(0));
    assert_eq!(millis_of(id), 1_759_500_000_123);
}

#[test]
fn same_clock_and_rng_give_the_same_id() {
    let clock = FixedClock(at(1_759_500_000_000));
    assert_eq!(uuid_v7(&clock, &ByteRng(7)), uuid_v7(&clock, &ByteRng(7)));
}

#[test]
fn random_bits_come_from_the_rng() {
    let clock = FixedClock(at(1_759_500_000_000));
    assert_ne!(uuid_v7(&clock, &ByteRng(1)), uuid_v7(&clock, &ByteRng(2)));
}

#[test]
fn time_before_the_epoch_becomes_the_epoch() {
    let id = uuid_v7(&FixedClock(at(-5_000)), &ByteRng(0));
    assert_eq!(millis_of(id), 0);
}

#[test]
fn latest_timestamp_fits() {
    assert_eq!(Timestamp::MAX.as_millisecond(), MAX_MILLIS);
    let id = uuid_v7(&FixedClock(Timestamp::MAX), &ByteRng(0));
    assert_eq!(millis_of(id), u64::try_from(MAX_MILLIS).unwrap());
}

proptest! {
    #[test]
    fn ids_sort_by_millisecond(
        first in 0..MAX_MILLIS - MAX_GAP,
        gap in 1..=MAX_GAP,
        early in any::<[u8; 10]>(),
        late in any::<[u8; 10]>(),
    ) {
        let earlier = uuid_v7_from(at(first), &early);
        let later = uuid_v7_from(at(first + gap), &late);
        prop_assert!(earlier < later, "{earlier} sorts after {later}");
    }

    #[test]
    fn millisecond_round_trips(millis in 0..=MAX_MILLIS, random in any::<[u8; 10]>()) {
        let id = uuid_v7_from(at(millis), &random);
        prop_assert_eq!(millis_of(id), u64::try_from(millis).unwrap());
        prop_assert_eq!(id.get_version_num(), 7);
    }
}
