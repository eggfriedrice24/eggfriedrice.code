//! The user's own messages of one conversation, for the name check of an exit in `auto`.
//!
//! efr checks whether a user message names a path, a unit or an upload against every
//! message of the conversation, not against the page of newest events that a turn
//! rebuilds its history from (efr's auto spec, section 9.2). A long conversation can
//! push an early "edit my zshrc" out of that page, and the name must not get lost. So
//! this read selects only the events that carry a user message, or take one back,
//! across the whole log of the conversation. It reads the `terminal_attached` events
//! too: the naming rule reads only the newest turns, and never a turn before a pick-up
//! or a resume in another terminal (v0.0.4).

use efr_protocol::{ConversationId, Event, EventEnvelope};
use rusqlite::{Connection, Row, params};

use crate::{StoreError, sql};

const TABLE: &str = "events";

/// The kinds of the events that [`of_conversation`] reads: a prompt, a steer, the
/// events that take one back, and the moves of the conversation to a terminal.
const KINDS: [&str; 5] = [
    "prompt_queued",
    "turn_steered",
    "prompt_withdrawn",
    "steering_withdrawn",
    "terminal_attached",
];

/// Every `prompt_queued`, `turn_steered`, `prompt_withdrawn`, `steering_withdrawn` and
/// `terminal_attached` of one conversation, oldest first.
pub fn of_conversation(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<Vec<EventEnvelope>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT seq, conversation_id, created_at, payload FROM events \
         WHERE conversation_id = ?1 AND kind IN (?2, ?3, ?4, ?5, ?6) ORDER BY seq",
    )?;
    let rows = stmt
        .query_map(
            params![conversation_id.to_string(), KINDS[0], KINDS[1], KINDS[2], KINDS[3], KINDS[4]],
            Raw::from_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(Raw::decode).collect()
}

/// A row as SQLite returns it, decoded after the statement finishes so that decode
/// errors are store errors.
struct Raw {
    seq: i64,
    conversation_id: Option<String>,
    created_at: i64,
    payload: String,
}

impl Raw {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Raw {
            seq: row.get(0)?,
            conversation_id: row.get(1)?,
            created_at: row.get(2)?,
            payload: row.get(3)?,
        })
    }

    fn decode(self) -> Result<EventEnvelope, StoreError> {
        let seq = sql::to_seq(self.seq, TABLE, "seq")?;
        let conversation_id = self
            .conversation_id
            .map(|text| sql::parse(&text, TABLE, "conversation_id"))
            .transpose()?;
        let at = sql::to_timestamp(self.created_at, TABLE, "created_at")?;
        let event: Event = serde_json::from_str(&self.payload)
            .map_err(|source| StoreError::DecodeEvent { seq, source })?;
        Ok(EventEnvelope { seq, conversation_id, at, event })
    }
}

#[cfg(test)]
mod tests;
