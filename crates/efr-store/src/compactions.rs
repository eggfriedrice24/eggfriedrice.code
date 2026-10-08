//! The compactions projection: each `conversation_compacted` event of a conversation.
//!
//! A turn builds its history after a compaction from the newest compaction with a
//! summary and the newest compaction of any kind. [`latest`] reads both with a query of
//! their own, so the page of newest events that a turn also reads never decides what
//! the model sees: a long conversation pages past the event, never past this table.

use efr_protocol::{Compaction, ConversationId, Event, EventEnvelope, Seq};
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::{StoreError, sql};

const TABLE: &str = "compactions";

/// One recorded compaction, with the sequence number of its event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoredCompaction {
    /// The sequence number of the `conversation_compacted` event.
    pub seq: Seq,
    /// The compaction as the event records it.
    pub compaction: Compaction,
}

/// The compactions that decide a conversation's history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LatestCompactions {
    /// The newest compaction that wrote a summary: the history starts with its summary
    /// and its verbatim tail.
    pub summary: Option<StoredCompaction>,
    /// The newest compaction of any kind. When it is newer than
    /// [`summary`](Self::summary) and has no summary of its own, its pruning holds for
    /// the tool results before its cut.
    pub newest: Option<StoredCompaction>,
}

/// Records a `conversation_compacted` event in the projection; other events change
/// nothing.
pub(crate) fn apply(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    let Event::ConversationCompacted(compaction) = &envelope.event else {
        return Ok(());
    };
    conn.execute(
        "INSERT INTO compactions (seq, conversation_id, compaction_id, has_summary, compaction) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            sql::seq(envelope.seq),
            conversation_id.to_string(),
            compaction.compaction_id.to_string(),
            i64::from(compaction.summary.is_some()),
            sql::to_json(compaction, "compaction")?,
        ],
    )?;
    Ok(())
}

/// Empties the table before a rebuild.
pub(crate) fn clear(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("DELETE FROM compactions", [])?;
    Ok(())
}

/// The newest compaction of `conversation_id` with a summary, and the newest of any
/// kind.
pub fn latest(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<LatestCompactions, StoreError> {
    let id = conversation_id.to_string();
    let summary = newest(
        conn,
        "SELECT seq, compaction FROM compactions WHERE conversation_id = ?1 AND has_summary = 1 \
         ORDER BY seq DESC LIMIT 1",
        &id,
    )?;
    let newest = newest(
        conn,
        "SELECT seq, compaction FROM compactions WHERE conversation_id = ?1 \
         ORDER BY seq DESC LIMIT 1",
        &id,
    )?;
    Ok(LatestCompactions { summary, newest })
}

/// Every compaction of `conversation_id`, oldest first.
pub fn of_conversation(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<Vec<StoredCompaction>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT seq, compaction FROM compactions WHERE conversation_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt
        .query_map([conversation_id.to_string()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(|(seq, compaction)| decode(seq, &compaction)).collect()
}

fn newest(
    conn: &Connection,
    query: &str,
    id: &str,
) -> Result<Option<StoredCompaction>, StoreError> {
    conn.query_row(query, [id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
        .optional()?
        .map(|(seq, compaction)| decode(seq, &compaction))
        .transpose()
}

fn decode(seq: i64, compaction: &str) -> Result<StoredCompaction, StoreError> {
    Ok(StoredCompaction {
        seq: sql::to_seq(seq, TABLE, "seq")?,
        compaction: sql::from_json(compaction, TABLE, "compaction")?,
    })
}

#[cfg(test)]
mod tests;
