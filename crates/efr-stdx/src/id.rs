//! UUIDv7 identifiers from the injected clock and generator.
//!
//! A version 7 UUID starts with the Unix time in milliseconds, so ids sort by creation
//! time across milliseconds and an index on them grows at one end. The remaining bits
//! come from the [`Rng`], so a manual clock and a seeded generator give the same ids
//! on every test run.

use jiff::Timestamp;
use uuid::Uuid;

use crate::rng::Rng;
use crate::time::Clock;

/// A new version 7 UUID for the current time on `clock`, with random bits from `rng`.
///
/// Two ids made in the same millisecond have no defined order between them. Code that
/// needs a total order uses a sequence number, as the event log does with `seq`.
pub fn uuid_v7(clock: &dyn Clock, rng: &dyn Rng) -> Uuid {
    let mut random = [0; 10];
    rng.fill_bytes(&mut random);
    uuid_v7_from(clock.now(), &random)
}

/// The id for `at` with the given random bytes. The time field is an unsigned count of
/// milliseconds, so a time before the Unix epoch becomes the epoch. The latest
/// `Timestamp`, late in the year 9999, fits in the 48-bit field.
pub(crate) fn uuid_v7_from(at: Timestamp, random: &[u8; 10]) -> Uuid {
    let millis = u64::try_from(at.as_millisecond()).unwrap_or(0);
    uuid::Builder::from_unix_timestamp_millis(millis, random).into_uuid()
}

#[cfg(test)]
mod tests;
