//! The deletes of conversations (migration 10).
//!
//! A delete is crash safe because it goes in a fixed order. First the event
//! `conversation_deleted` commits with the tombstone row in `conversation_deletes` and
//! without the conversation's projection rows (`tombstone`, which the writer runs for
//! that event): from that commit on, no list, search or read finds the conversation,
//! and each later event of it fails its batch, because the conversations projection has
//! no row for it. Then [`WriterHandle::delete_step`](crate::WriterHandle::delete_step)
//! removes the rest of its rows in short transactions, each the same work again after a
//! stop: a page of its events with their search docs and receipts, and at the end its
//! saved messages, its search title, its outbox rows and the index of its recordings.
//! The daemon then removes the files of the conversation and marks the delete done
//! ([`WriterHandle::delete_done`](crate::WriterHandle::delete_done)). A start of efrd
//! finishes each delete that [`pending`] lists.
//!
//! The tombstone event stays in the log: it keeps the sequence numbers above every
//! event that went, so a number never names two events, and a subscriber that resumes
//! after it misses nothing. The trigger of the log lets only the events of a deleted
//! conversation from before its tombstone go.

use efr_protocol::{ConversationId, EventEnvelope, PtyId, Seq, UndoId};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};

use crate::{StoreError, sql};

const TABLE: &str = "conversation_deletes";

/// The most events that one step of a delete removes, with their search docs and
/// receipts, so a long conversation never holds the writer for long.
pub const STEP_EVENTS: u32 = 2_000;

/// A delete as its tombstone row holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Tombstone {
    /// The deleted conversation.
    pub conversation_id: ConversationId,
    /// The sequence number of its `conversation_deleted` event.
    pub seq: Seq,
    /// When the delete was recorded.
    pub requested_at: Timestamp,
    /// The PTYs of its hidden shells, whose recordings go.
    pub ptys: Vec<PtyId>,
    /// Its undos, whose undo trash goes.
    pub undos: Vec<UndoId>,
    /// True once its rows are gone.
    pub rows_done: bool,
}

/// What one step of a delete did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteStep {
    /// It removed this many events; more may be left.
    Removed(u32),
    /// Every row of the conversation is gone (or was gone before this step).
    RowsDone,
    /// The conversation has no tombstone.
    NotDeleted,
}

/// The parts of a conversation outside the database, as the tombstone keeps them.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Parts {
    #[serde(default)]
    ptys: Vec<PtyId>,
    #[serde(default)]
    undos: Vec<UndoId>,
}

/// The writer's part of `conversation_deleted`: the tombstone row, with the PTYs and
/// undos that the conversation's events name, and no projection row of the
/// conversation any more. A conversation that has no row (unknown, or deleted before)
/// fails the batch.
pub(crate) fn tombstone(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    let id = conversation_id.to_string();
    let known: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM conversations WHERE id = ?1)",
        [&id],
        |row| row.get(0),
    )?;
    if !known || is_deleted(conn, conversation_id)? {
        return Err(StoreError::UnknownConversation { conversation_id });
    }
    let parts = Parts {
        ptys: texts(
            conn,
            "SELECT DISTINCT json_extract(payload, '$.pty_id') FROM events \
             WHERE conversation_id = ?1 AND kind = 'shell_started'",
            &id,
        )?
        .iter()
        .map(|text| sql::parse(text, "events", "payload"))
        .collect::<Result<_, _>>()?,
        undos: texts(
            conn,
            "SELECT DISTINCT json_extract(payload, '$.undo_id') FROM events \
             WHERE conversation_id = ?1 AND kind = 'undo_applied'",
            &id,
        )?
        .iter()
        .map(|text| sql::parse(text, "events", "payload"))
        .collect::<Result<_, _>>()?,
    };
    conn.execute(
        "INSERT INTO conversation_deletes (conversation_id, seq, requested_at, parts) \
         VALUES (?1, ?2, ?3, ?4)",
        params![
            id,
            sql::seq(envelope.seq),
            sql::micros(envelope.at),
            sql::to_json(&parts, "parts")?
        ],
    )?;
    // NOTE: children first, for the foreign keys of turns, approvals and shells.
    conn.execute("DELETE FROM job_notices WHERE conversation_id = ?1", [&id])?;
    conn.execute("DELETE FROM jobs WHERE conversation_id = ?1", [&id])?;
    conn.execute("DELETE FROM compactions WHERE conversation_id = ?1", [&id])?;
    conn.execute("DELETE FROM approvals WHERE conversation_id = ?1", [&id])?;
    conn.execute("DELETE FROM shells WHERE conversation_id = ?1", [&id])?;
    conn.execute("DELETE FROM turns WHERE conversation_id = ?1", [&id])?;
    conn.execute("DELETE FROM conversations WHERE id = ?1", [&id])?;
    Ok(())
}

/// The non-null text values of the one column of `query` for the conversation `id`.
fn texts(conn: &Connection, query: &str, id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(query)?;
    let rows = stmt.query_map([id], |row| row.get::<_, Option<String>>(0))?;
    let mut found = Vec::new();
    for row in rows {
        if let Some(text) = row? {
            found.push(text);
        }
    }
    Ok(found)
}

/// The query of [`is_deleted`], for a caller that asks for many conversations with one
/// prepared statement.
pub(crate) const IS_DELETED: &str =
    "SELECT EXISTS (SELECT 1 FROM conversation_deletes WHERE conversation_id = ?1)";

/// True when `conversation_id` has a tombstone.
pub fn is_deleted(conn: &Connection, conversation_id: ConversationId) -> Result<bool, StoreError> {
    Ok(conn.prepare(IS_DELETED)?.query_row([conversation_id.to_string()], |row| row.get(0))?)
}

/// Every conversation with a tombstone, for a rebuild of the projections, which skips
/// their events.
pub(crate) fn deleted_ids(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT conversation_id FROM conversation_deletes")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The tombstone of `conversation_id`, if it has one.
pub fn get(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<Option<Tombstone>, StoreError> {
    conn.query_row(
        "SELECT conversation_id, seq, requested_at, parts, rows_done_at FROM conversation_deletes \
         WHERE conversation_id = ?1",
        [conversation_id.to_string()],
        RawTombstone::from_row,
    )
    .optional()?
    .map(RawTombstone::decode)
    .transpose()
}

/// The deletes that are not done, oldest first: a start of efrd finishes them.
pub fn pending(conn: &Connection) -> Result<Vec<Tombstone>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT conversation_id, seq, requested_at, parts, rows_done_at FROM conversation_deletes \
         WHERE done_at IS NULL ORDER BY seq",
    )?;
    let rows = stmt.query_map([], RawTombstone::from_row)?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?.decode()?);
    }
    Ok(found)
}

/// One step of the delete of `conversation_id` in the writer's transaction: at most
/// `limit` of its events before the tombstone with their search docs and receipts, or,
/// when none is left, the rest of its rows.
pub(crate) fn step(
    conn: &Connection,
    conversation_id: ConversationId,
    limit: u32,
    at: Timestamp,
) -> Result<DeleteStep, StoreError> {
    let Some(tombstone) = get(conn, conversation_id)? else {
        return Ok(DeleteStep::NotDeleted);
    };
    if tombstone.rows_done {
        return Ok(DeleteStep::RowsDone);
    }
    let id = conversation_id.to_string();
    let before = sql::seq(tombstone.seq);
    let page: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT seq FROM events WHERE conversation_id = ?1 AND seq < ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![id, before, limit], |row| row.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    if let (Some(&first), Some(&last)) = (page.first(), page.last()) {
        // NOTE: `receipts.seq` has no index; one scan per page, with the page's numbers
        // from the index of the events.
        conn.execute(
            "DELETE FROM receipts WHERE seq BETWEEN ?2 AND ?3 AND seq IN \
             (SELECT seq FROM events WHERE conversation_id = ?1 AND seq BETWEEN ?2 AND ?3)",
            params![id, first, last],
        )?;
        let mut read =
            conn.prepare("SELECT conversation_id, kind, text FROM search_docs WHERE seq = ?1")?;
        let mut delete_fts = conn.prepare(
            "INSERT INTO search_fts (search_fts, rowid, text, conversation_id, kind) \
             VALUES ('delete', ?1, ?2, ?3, ?4)",
        )?;
        let mut delete_doc = conn.prepare("DELETE FROM search_docs WHERE seq = ?1")?;
        for &seq in &page {
            let doc: Option<(String, i64, String)> = read
                .query_row([seq], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()?;
            if let Some((conversation, kind, text)) = doc {
                delete_fts.execute(params![seq, text, conversation, kind])?;
                delete_doc.execute([seq])?;
            }
        }
        conn.execute(
            "DELETE FROM events WHERE conversation_id = ?1 AND seq BETWEEN ?2 AND ?3",
            params![id, first, last],
        )?;
        let removed = u32::try_from(page.len()).unwrap_or(u32::MAX);
        return Ok(DeleteStep::Removed(removed));
    }
    conn.execute("DELETE FROM turn_messages WHERE conversation_id = ?1", [&id])?;
    conn.execute("DELETE FROM search_titles WHERE conversation_id = ?1", [&id])?;
    conn.execute(
        "DELETE FROM outbox WHERE json_extract(payload, '$.conversation_id') = ?1",
        [&id],
    )?;
    let mut segments = conn.prepare("DELETE FROM recording_segments WHERE pty_id = ?1")?;
    for pty in &tombstone.ptys {
        segments.execute([pty.to_string()])?;
    }
    conn.execute(
        "UPDATE conversation_deletes SET rows_done_at = ?2 WHERE conversation_id = ?1",
        params![id, sql::micros(at)],
    )?;
    Ok(DeleteStep::RowsDone)
}

/// Marks the delete of `conversation_id` done: its files are gone too. True when it
/// had a tombstone whose rows were gone.
pub(crate) fn done(
    conn: &Connection,
    conversation_id: ConversationId,
    at: Timestamp,
) -> Result<bool, StoreError> {
    let changed = conn.execute(
        "UPDATE conversation_deletes SET done_at = ?2 \
         WHERE conversation_id = ?1 AND rows_done_at IS NOT NULL",
        params![conversation_id.to_string(), sql::micros(at)],
    )?;
    Ok(changed > 0)
}

/// A tombstone row as SQLite returns it.
struct RawTombstone {
    conversation_id: String,
    seq: i64,
    requested_at: i64,
    parts: String,
    rows_done_at: Option<i64>,
}

impl RawTombstone {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawTombstone {
            conversation_id: row.get(0)?,
            seq: row.get(1)?,
            requested_at: row.get(2)?,
            parts: row.get(3)?,
            rows_done_at: row.get(4)?,
        })
    }

    fn decode(self) -> Result<Tombstone, StoreError> {
        let parts: Parts = sql::from_json(&self.parts, TABLE, "parts")?;
        Ok(Tombstone {
            conversation_id: sql::parse(&self.conversation_id, TABLE, "conversation_id")?,
            seq: sql::to_seq(self.seq, TABLE, "seq")?,
            requested_at: sql::to_timestamp(self.requested_at, TABLE, "requested_at")?,
            ptys: parts.ptys,
            undos: parts.undos,
            rows_done: self.rows_done_at.is_some(),
        })
    }
}

#[cfg(test)]
mod tests;
