//! Clocks, ids and stores shared by the unit tests of this crate.
//!
//! `efr-test-support` provides the general `TestClock`, but it depends on this crate,
//! so these few helpers stay local.

use std::fmt::Debug;
use std::future::ready;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_protocol::{
    CallId, CommandId, ConversationId, Event, Origin, PtyId, Scope, ShellContext, TurnId,
    TurnSettings,
};
use efr_stdx::time::{Clock, Sleep};
use jiff::{SignedDuration, Timestamp};
use rusqlite::Connection;

use crate::writer::DEFAULT_BROADCAST_CAPACITY;
use crate::{Migrations, StoreError, StoreWriter, WriterHandle, db};

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

pub(crate) fn call(n: u64) -> CallId {
    id(4, n)
}

pub(crate) fn pty(n: u64) -> PtyId {
    id(5, n)
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

pub(crate) fn path(text: &str) -> PathBuf {
    PathBuf::from(text)
}

/// A prompt for turn `turn`, sent from `/etc/nixos`.
pub(crate) fn queued(turn: u64, text: &str) -> Event {
    Event::PromptQueued {
        turn_id: self::turn(turn),
        command_id: command(turn),
        text: text.to_owned(),
        origin: Origin::Shell,
        context: Some(ShellContext::new("/etc/nixos")),
        settings: TurnSettings::default(),
        steers: Vec::new(),
    }
}

/// Turn `turn` starts in `cwd`, scoped to that path.
pub(crate) fn started(turn: u64, cwd: &str) -> Event {
    Event::TurnStarted {
        turn_id: self::turn(turn),
        cwd: cwd.into(),
        scope: Scope::Path(cwd.into()),
        settings: None,
    }
}

/// The compaction `n` of a conversation, whose cut is after the whole turn `through`
/// (inside it, before message `message`, when given), with a summary when `summary`.
pub(crate) fn compacted(n: u64, through: u64, message: Option<u32>, summary: bool) -> Event {
    Event::ConversationCompacted(efr_protocol::Compaction {
        compaction_id: id(6, n),
        turn_id: None,
        trigger: efr_protocol::CompactionTrigger::Manual,
        focus: None,
        model: "test-model".to_owned(),
        window: 272_000,
        limit: 206_720,
        tokens_before: 230_000,
        tokens_after: 20_000,
        through_turn: turn(through),
        through_message: message,
        kept_turns: 1,
        pruned_outputs: if summary { 0 } else { 3 },
        pruned_tokens: if summary { 0 } else { 30_000 },
        omitted_turns: 0,
        omitted_messages: 0,
        summary: summary.then(|| format!("## Task and state\nsummary {n}")),
        usage: None,
    })
}

/// Runs `f` on the writer's connection.
pub(crate) async fn on_writer<T: Send + 'static>(
    writer: &WriterHandle,
    f: impl FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    writer.run(move |state| f(&state.conn)).await
}

/// Every row of `table` as text, ordered by the first column.
pub(crate) fn dump(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY 1")).unwrap();
    let columns = stmt.column_count();
    stmt.query_map([], |row| {
        let values: Vec<String> = (0..columns)
            .map(|index| row.get_ref(index).map(|value| format!("{value:?}")))
            .collect::<Result<_, _>>()?;
        Ok(values.join(" | "))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}
