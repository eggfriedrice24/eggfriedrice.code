//! The outbox: durable side effects that outlive the request that caused them.
//!
//! A handler enqueues an item in the batch whose events record the decision, so the
//! effect is queued exactly when the decision commits. A worker claims items in id
//! order through `WriterHandle::outbox_claim`, performs them, and marks them done.
//!
//! Items are replay-safe or process-bound. After a restart, `cancel_process_bound`
//! cancels every unfinished process-bound item (a provider stream, a reply on a
//! connection that no longer exists) and returns unfinished replay-safe items to the
//! queue: nothing continues on its own after a restart except what is safe to repeat.

use std::fmt;

use jiff::Timestamp;
use rusqlite::{Connection, Row, params};
use serde_json::Value;

use crate::{StoreError, sql};

const TABLE: &str = "outbox";

/// The id of an outbox item. Ids are never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutboxId(i64);

impl OutboxId {
    /// The number inside.
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for OutboxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// An item to enqueue with a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct NewOutboxItem {
    kind: String,
    payload: Value,
    replay_safe: bool,
}

impl NewOutboxItem {
    /// An item that may run again after a restart, such as a notification.
    pub fn replay_safe(kind: impl Into<String>, payload: Value) -> Self {
        NewOutboxItem { kind: kind.into(), payload, replay_safe: true }
    }

    /// An item that only makes sense in the process that enqueued it, such as a
    /// provider turn. A restart cancels it.
    pub fn process_bound(kind: impl Into<String>, payload: Value) -> Self {
        NewOutboxItem { kind: kind.into(), payload, replay_safe: false }
    }
}

/// An unfinished outbox item.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OutboxItem {
    /// The item.
    pub id: OutboxId,
    /// What to do, such as `notify.tty`; the worker dispatches on it.
    pub kind: String,
    /// The details the worker needs.
    pub payload: Value,
    /// True when the item may run again after a restart.
    pub replay_safe: bool,
    /// When it was enqueued.
    pub created_at: Timestamp,
    /// When a worker claimed it; `None` while it waits.
    pub claimed_at: Option<Timestamp>,
}

/// What [`crate::WriterHandle::outbox_cancel_process_bound`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutboxReconciled {
    /// Unfinished process-bound items that were cancelled.
    pub cancelled: u64,
    /// Claimed but unfinished replay-safe items that went back to the queue.
    pub requeued: u64,
}

/// Enqueues `item` inside the writer's transaction.
pub(crate) fn enqueue(
    conn: &Connection,
    item: &NewOutboxItem,
    at: Timestamp,
) -> Result<OutboxId, StoreError> {
    conn.execute(
        "INSERT INTO outbox (kind, payload, replay_safe, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            item.kind,
            sql::to_json(&item.payload, "outbox payload")?,
            item.replay_safe,
            sql::micros(at)
        ],
    )?;
    Ok(OutboxId(conn.last_insert_rowid()))
}

/// Claims up to `limit` waiting items, oldest first, and returns them.
pub(crate) fn claim(
    conn: &Connection,
    at: Timestamp,
    limit: u32,
) -> Result<Vec<OutboxItem>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, payload, replay_safe, created_at, claimed_at FROM outbox \
         WHERE claimed_at IS NULL AND done_at IS NULL AND cancelled_at IS NULL \
         ORDER BY id LIMIT ?1",
    )?;
    let mut raw = stmt.query_map([limit], RawItem::from_row)?.collect::<Result<Vec<_>, _>>()?;
    let claimed_at = sql::micros(at);
    for item in &mut raw {
        conn.execute(
            "UPDATE outbox SET claimed_at = ?2 WHERE id = ?1",
            params![item.id, claimed_at],
        )?;
        item.claimed_at = Some(claimed_at);
    }
    raw.into_iter().map(RawItem::decode).collect()
}

/// Marks a claimed item done.
pub(crate) fn done(conn: &Connection, id: OutboxId, at: Timestamp) -> Result<(), StoreError> {
    let changed = conn.execute(
        "UPDATE outbox SET done_at = ?2 WHERE id = ?1 AND claimed_at IS NOT NULL \
         AND done_at IS NULL AND cancelled_at IS NULL",
        params![id.0, sql::micros(at)],
    )?;
    if changed == 0 {
        return Err(StoreError::OutboxItemNotClaimed { id });
    }
    Ok(())
}

/// The startup step: cancels every unfinished process-bound item and returns every
/// claimed, unfinished replay-safe item to the queue.
pub(crate) fn cancel_process_bound(
    conn: &Connection,
    at: Timestamp,
) -> Result<OutboxReconciled, StoreError> {
    let cancelled = conn.execute(
        "UPDATE outbox SET cancelled_at = ?1 \
         WHERE replay_safe = 0 AND done_at IS NULL AND cancelled_at IS NULL",
        [sql::micros(at)],
    )?;
    let requeued = conn.execute(
        "UPDATE outbox SET claimed_at = NULL \
         WHERE replay_safe = 1 AND claimed_at IS NOT NULL AND done_at IS NULL \
         AND cancelled_at IS NULL",
        [],
    )?;
    Ok(OutboxReconciled { cancelled: cancelled as u64, requeued: requeued as u64 })
}

/// Every unfinished item, claimed or waiting, oldest first.
pub fn open_items(conn: &Connection) -> Result<Vec<OutboxItem>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, payload, replay_safe, created_at, claimed_at FROM outbox \
         WHERE done_at IS NULL AND cancelled_at IS NULL ORDER BY id",
    )?;
    let raw = stmt.query_map([], RawItem::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawItem::decode).collect()
}

struct RawItem {
    id: i64,
    kind: String,
    payload: String,
    replay_safe: bool,
    created_at: i64,
    claimed_at: Option<i64>,
}

impl RawItem {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawItem {
            id: row.get(0)?,
            kind: row.get(1)?,
            payload: row.get(2)?,
            replay_safe: row.get(3)?,
            created_at: row.get(4)?,
            claimed_at: row.get(5)?,
        })
    }

    fn decode(self) -> Result<OutboxItem, StoreError> {
        Ok(OutboxItem {
            id: OutboxId(self.id),
            kind: self.kind,
            payload: sql::from_json(&self.payload, TABLE, "payload")?,
            replay_safe: self.replay_safe,
            created_at: sql::to_timestamp(self.created_at, TABLE, "created_at")?,
            claimed_at: self
                .claimed_at
                .map(|value| sql::to_timestamp(value, TABLE, "claimed_at"))
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests;
