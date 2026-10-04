//! Clocks and generators shared by the unit tests of this crate.
//!
//! `efr-test-support` provides the general `TestClock` and `TestRng`. These few fakes
//! keep the tests of this crate free of that crate and of the SQLite build it brings.

use std::future::{pending, ready};
use std::sync::Mutex;
use std::time::Duration;

use efr_stdx::rng::Rng;
use efr_stdx::time::{Clock, Sleep};
use jiff::{SignedDuration, Timestamp};

/// The instant every fake clock starts at: 2026-10-04T12:00:00Z.
pub(crate) fn start() -> Timestamp {
    Timestamp::from_second(1_791_115_200).unwrap()
}

/// A clock whose sleeps finish at once and move `now` forward by their duration.
#[derive(Debug)]
pub(crate) struct InstantClock {
    now: Mutex<Timestamp>,
    sleeps: Mutex<Vec<Duration>>,
}

impl InstantClock {
    pub(crate) fn new() -> Self {
        InstantClock { now: Mutex::new(start()), sleeps: Mutex::new(Vec::new()) }
    }

    /// Every sleep so far, in order.
    pub(crate) fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl Clock for InstantClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        self.sleeps.lock().unwrap().push(duration);
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(SignedDuration::try_from(duration).unwrap()).unwrap();
        Box::pin(ready(()))
    }
}

/// A clock whose sleeps never finish, so a timeout on it never fires.
#[derive(Debug)]
pub(crate) struct StoppedClock;

impl Clock for StoppedClock {
    fn now(&self) -> Timestamp {
        start()
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(pending())
    }
}

/// A generator that always draws the same number.
#[derive(Debug)]
pub(crate) struct FixedRng(pub(crate) u64);

impl Rng for FixedRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        for (index, byte) in dest.iter_mut().enumerate() {
            *byte = self.0.to_le_bytes()[index % 8];
        }
    }

    fn next_u64(&self) -> u64 {
        self.0
    }
}
