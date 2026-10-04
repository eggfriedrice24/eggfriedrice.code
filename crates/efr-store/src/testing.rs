//! Clocks, ids and stores shared by the unit tests of this crate.
//!
//! `efr-test-support` provides the general `TestClock`, but it depends on this crate,
//! so these few helpers stay local.

use std::fmt::Debug;
use std::future::ready;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_protocol::{CommandId, ConversationId, Event, Origin, TurnId};
use efr_stdx::time::{Clock, Sleep};
use jiff::{SignedDuration, Timestamp};

use crate::writer::DEFAULT_BROADCAST_CAPACITY;
use crate::{Migrations, StoreWriter, WriterHandle, db};

/// The instant every test clock starts at: 2026-10-04T12:00:00Z, plus a nanosecond
/// part that the store must drop.
pub(crate) fn start() -> Timestamp {
    Timestamp::new(1_791_115_200, 123_456_789).unwrap()
}

/// A clock that moves only when a test moves it. Its sleeps finish at once and move
/// it forward by their duration.
#[derive(Debug)]
pub(crate) struct TestClock {
    now: Mutex<Timestamp>,
}

impl TestClock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(TestClock { now: Mutex::new(start()) })
    }

    pub(crate) fn advance(&self, by: Duration) {
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(SignedDuration::try_from(by).unwrap()).unwrap();
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        self.advance(duration);
        Box::pin(ready(()))
    }
}

fn id<T>(kind: u32, n: u64) -> T
where
    T: FromStr,
    T::Err: Debug,
{
    format!("{kind:08x}-0000-7000-8000-{n:012x}").parse().unwrap()
}

pub(crate) fn conversation(n: u64) -> ConversationId {
    id(1, n)
}

pub(crate) fn turn(n: u64) -> TurnId {
    id(2, n)
}

pub(crate) fn command(n: u64) -> CommandId {
    id(3, n)
}

pub(crate) fn created(tty: Option<&str>) -> Event {
    Event::ConversationCreated { origin: Origin::Shell, tty: tty.map(str::to_owned) }
}

/// A writer over a migrated in-memory database.
pub(crate) fn memory_writer(clock: Arc<TestClock>) -> (WriterHandle, StoreWriter) {
    let mut conn = db::open_in_memory().unwrap();
    Migrations::new().migrate(&mut conn, None).unwrap();
    StoreWriter::spawn(conn, clock, DEFAULT_BROADCAST_CAPACITY).unwrap()
}
