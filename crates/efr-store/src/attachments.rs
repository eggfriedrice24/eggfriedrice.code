//! The table `attachments`: the attachments of each conversation, a projection of the
//! events that record them (`0015_attachments.sql`).
//!
//! `prompt_queued` and `tool_call_completed` add one row for each attachment, with the
//! sequence number of their event. `attachments_withheld` marks the rows that efrd sends
//! as a label from then on. `prompt_withdrawn` drops the rows of its prompt, which no
//! request reads. The bytes are never here: they are in efrd's blob dir.
//!
//! A turn reads the [`Summary`] of its conversation when it starts: the highest number,
//! for the id of the next attachment, and the withheld ones.

use efr_protocol::{AttachmentId, ConversationId, Event, EventEnvelope};
use rusqlite::{Connection, params};

use crate::{StoreError, sql};

/// What a turn needs to know of the attachments of its conversation before it starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Summary {
    /// The highest number of an attachment of the conversation; 0 when it has none.
    pub last: u32,
    /// The attachments that efrd sends as a label from now on, in the order of the
    /// conversation.
    pub withheld: Vec<AttachmentId>,
}

impl Summary {
    /// A summary of a conversation whose highest number is `last`.
    pub fn new(last: u32, withheld: Vec<AttachmentId>) -> Self {
        Summary { last, withheld }
    }
}

/// Brings the table up to date with one event of the conversation; other events change
/// nothing.
pub(crate) fn apply(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    let id = conversation_id.to_string();
    match &envelope.event {
        Event::PromptQueued { attachments, .. } | Event::ToolCallCompleted { attachments, .. } => {
            let mut insert = conn.prepare(
                "INSERT INTO attachments (conversation_id, number, sha256, attachment, seq) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for attachment in attachments {
                insert.execute(params![
                    id,
                    number(&attachment.id),
                    attachment.sha256,
                    sql::to_json(attachment, "attachment")?,
                    sql::seq(envelope.seq),
                ])?;
            }
        }
        Event::AttachmentsWithheld { attachments, .. } => {
            let mut update = conn.prepare(
                "UPDATE attachments SET withheld = 1 WHERE conversation_id = ?1 AND number = ?2",
            )?;
            for attachment in attachments {
                update.execute(params![id, number(attachment)])?;
            }
        }
        Event::PromptWithdrawn { turn_id, .. } => {
            conn.execute(
                "DELETE FROM attachments WHERE conversation_id = ?1 AND seq IN \
                 (SELECT seq FROM events WHERE conversation_id = ?1 AND turn_id = ?2 \
                  AND kind = 'prompt_queued')",
                params![id, turn_id.to_string()],
            )?;
        }
        _ => {}
    }
    Ok(())
}

/// Empties the table, before a rebuild replays the log.
pub(crate) fn clear(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("DELETE FROM attachments", [])?;
    Ok(())
}

/// The [`Summary`] of the attachments of `conversation_id`.
pub fn summary(conn: &Connection, conversation_id: ConversationId) -> Result<Summary, StoreError> {
    let id = conversation_id.to_string();
    let last: Option<i64> = conn.query_row(
        "SELECT MAX(number) FROM attachments WHERE conversation_id = ?1",
        [&id],
        |row| row.get(0),
    )?;
    let mut withheld = conn.prepare(
        "SELECT number FROM attachments WHERE conversation_id = ?1 AND withheld = 1 \
         ORDER BY number",
    )?;
    let withheld = withheld
        .query_map([&id], |row| row.get::<_, i64>(0))?
        .map(|number| Ok(AttachmentId::nth(to_number(number?))))
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(Summary { last: last.map_or(0, to_number), withheld })
}

/// True when a row of `conversation_id` names the blob of the bytes with the SHA-256
/// `sha256`: the blob must stay.
pub fn names_blob(
    conn: &Connection,
    conversation_id: ConversationId,
    sha256: &str,
) -> Result<bool, StoreError> {
    let named = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM attachments WHERE conversation_id = ?1 AND sha256 = ?2)",
        params![conversation_id.to_string(), sha256],
        |row| row.get(0),
    )?;
    Ok(named)
}

/// The number of `id` as the table keeps it.
fn number(id: &AttachmentId) -> i64 {
    id.number().map_or(0, i64::from)
}

/// A number of the table as an id's number.
fn to_number(number: i64) -> u32 {
    u32::try_from(number).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
