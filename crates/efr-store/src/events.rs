//! The event log: one row per event, in sequence order.
//!
//! The writer inserts rows; the functions here read them back as `EventEnvelope`s. A
//! row whose kind this build does not know, written by a newer daemon, reads back as
//! `Event::Unknown { kind, payload }`, so a reader keeps advancing past it. A row of a
//! known kind whose body does not match fails with [`StoreError::DecodeEvent`].
//!
//! The read functions take a connection, so they run inside `Readers::with`.

use std::collections::HashSet;

use efr_protocol::{ConversationId, Event, EventEnvelope, Seq, TurnId};
use rusqlite::{Connection, Row, params};

use crate::{StoreError, sql};

const TABLE: &str = "events";

/// Writes one event. The caller assigns `seq` and runs inside the writer's
/// transaction.
pub(crate) fn insert(conn: &Connection, envelope: &EventEnvelope) -> Result<(), StoreError> {
    let event = &envelope.event;
    let payload = sql::to_json(event, "event")?;
    conn.execute(
        "INSERT INTO events (seq, conversation_id, turn_id, kind, payload, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            sql::seq(envelope.seq),
            envelope.conversation_id.map(|id| id.to_string()),
            event.turn_id().map(|id| id.to_string()),
            event.kind(),
            payload,
            sql::micros(envelope.at),
        ],
    )?;
    Ok(())
}

/// Up to `limit` events with a sequence number above `after`, in order, across every
/// conversation.
pub fn read_after(
    conn: &Connection,
    after: Seq,
    limit: u32,
) -> Result<Vec<EventEnvelope>, StoreError> {
    query(
        conn,
        "SELECT seq, conversation_id, created_at, payload FROM events \
         WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        params![sql::seq(after), limit],
    )
}

/// Up to `limit` events of one conversation with a sequence number above `after`, in
/// order: the replay of a subscription that resumes.
pub fn read_conversation_after(
    conn: &Connection,
    conversation_id: ConversationId,
    after: Seq,
    limit: u32,
) -> Result<Vec<EventEnvelope>, StoreError> {
    query(
        conn,
        "SELECT seq, conversation_id, created_at, payload FROM events \
         WHERE conversation_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        params![conversation_id.to_string(), sql::seq(after), limit],
    )
}

/// The `limit` most recent events of one conversation below `before` (or the most
/// recent ones when `before` is `None`), oldest first: one page of
/// `conversation.history`, paging backwards.
pub fn read_conversation_before(
    conn: &Connection,
    conversation_id: ConversationId,
    before: Option<Seq>,
    limit: u32,
) -> Result<Vec<EventEnvelope>, StoreError> {
    let before = before.map_or(i64::MAX, sql::seq);
    let mut page = query(
        conn,
        "SELECT seq, conversation_id, created_at, payload FROM events \
         WHERE conversation_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3",
        params![conversation_id.to_string(), before, limit],
    )?;
    page.reverse();
    Ok(page)
}

/// The `limit` most recent events of one conversation without its
/// `tool_call_output_updated` events, oldest first: what a turn rebuilds the earlier
/// turns from. A command's output is in its `tool_call_completed`; the updates only
/// showed it growing, and a long command writes thousands of them, which would push
/// earlier turns out of the page.
pub fn read_turn_history(
    conn: &Connection,
    conversation_id: ConversationId,
    limit: u32,
) -> Result<Vec<EventEnvelope>, StoreError> {
    read_turn_history_without(conn, conversation_id, limit, &HashSet::new()).map(|page| page.events)
}

/// The kinds of the events that hold the body of a turn: the assistant's text, as it
/// streamed and when it completed, and each tool call with its result. A turn that is
/// rebuilt from its events reads them, and no other reader of the page does.
pub const BODY_KINDS: [&str; 4] = [
    "assistant_message_updated",
    "assistant_message_completed",
    "tool_call_started",
    "tool_call_completed",
];

/// One page of [`read_turn_history_without`].
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct TurnHistory {
    /// The events of the page, oldest first, without the bodies that the reader does
    /// not need.
    pub events: Vec<EventEnvelope>,
    /// The sequence number of the oldest event of the page, also when the page leaves
    /// that event out; `None` for an empty page.
    pub first: Option<Seq>,
    /// The events that the page left out.
    pub left_out: u64,
    /// The bytes of event JSON that the read decoded.
    pub bytes: u64,
}

/// The page of [`read_turn_history`] without the events of [`BODY_KINDS`] of the turns
/// `bodies_of` names: the reader has those turns' messages from another source, so the
/// read never copies those events out of SQLite or decodes them. A left-out event still
/// counts towards `limit`, so the page reaches back exactly as far as without the
/// filter, and every event that it holds is the event of the log. A turn id that
/// `bodies_of` does not name keeps every event.
pub fn read_turn_history_without(
    conn: &Connection,
    conversation_id: ConversationId,
    limit: u32,
    bodies_of: &HashSet<TurnId>,
) -> Result<TurnHistory, StoreError> {
    let turns: Vec<String> = bodies_of.iter().map(ToString::to_string).collect();
    let turns = sql::to_json(&turns, "turn ids")?;
    let kinds = BODY_KINDS.map(|kind| format!("'{kind}'")).join(", ");
    // NOTE: SQLite reads a column inside a CASE only when that branch runs, so the
    // payload of a left-out event, often a tool output of 32 KiB, is never copied.
    let statement = format!(
        "SELECT seq, conversation_id, created_at, CASE WHEN kind IN ({kinds}) AND turn_id IN \
         (SELECT value FROM json_each(?3)) THEN NULL ELSE payload END FROM events \
         WHERE conversation_id = ?1 AND kind <> 'tool_call_output_updated' \
         ORDER BY seq DESC LIMIT ?2"
    );
    let mut stmt = conn.prepare(&statement)?;
    let rows = stmt
        .query_map(params![conversation_id.to_string(), limit, turns], |row| {
            // NOTE: `payload` is NOT NULL in the table, so only the CASE gives NULL.
            let Some(payload) = row.get::<_, Option<String>>(3)? else {
                return Ok(Paged::LeftOut(row.get(0)?));
            };
            Ok(Paged::Kept(RawEvent {
                seq: row.get(0)?,
                conversation_id: row.get(1)?,
                created_at: row.get(2)?,
                payload,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut page = TurnHistory::default();
    if let Some(oldest) = rows.last() {
        let seq = match oldest {
            Paged::Kept(raw) => raw.seq,
            Paged::LeftOut(seq) => *seq,
        };
        page.first = Some(sql::to_seq(seq, TABLE, "seq")?);
    }
    page.events.reserve(rows.len());
    for row in rows.into_iter().rev() {
        match row {
            Paged::Kept(raw) => {
                page.bytes += raw.payload.len() as u64;
                page.events.push(raw.decode()?);
            }
            Paged::LeftOut(_) => page.left_out += 1,
        }
    }
    Ok(page)
}

/// A row of [`read_turn_history_without`]: an event, or the sequence number of an
/// event that the page left out.
enum Paged {
    Kept(RawEvent),
    LeftOut(i64),
}

/// The events of one conversation from `from` through `through`, without its
/// `tool_call_output_updated` events, oldest first: the events of an older turn, for a
/// turn that saved no messages and whose start is older than the page of
/// [`read_turn_history`].
pub fn read_turn_range(
    conn: &Connection,
    conversation_id: ConversationId,
    from: Seq,
    through: Seq,
) -> Result<Vec<EventEnvelope>, StoreError> {
    query(
        conn,
        "SELECT seq, conversation_id, created_at, payload FROM events \
         WHERE conversation_id = ?1 AND seq >= ?2 AND seq <= ?3 \
         AND kind <> 'tool_call_output_updated' ORDER BY seq",
        params![conversation_id.to_string(), sql::seq(from), sql::seq(through)],
    )
}

/// The sequence number of the newest event, or [`Seq::ZERO`] for an empty log: the
/// high-water mark a subscriber reads after it subscribes.
pub fn last_seq(conn: &Connection) -> Result<Seq, StoreError> {
    let seq: i64 =
        conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| row.get(0))?;
    sql::to_seq(seq, TABLE, "seq")
}

/// A row as SQLite returns it, decoded after the statement finishes so that decode
/// errors are store errors.
struct RawEvent {
    seq: i64,
    conversation_id: Option<String>,
    created_at: i64,
    payload: String,
}

impl RawEvent {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawEvent {
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

fn query(
    conn: &Connection,
    statement: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<EventEnvelope>, StoreError> {
    let mut stmt = conn.prepare(statement)?;
    let raw = stmt.query_map(params, RawEvent::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawEvent::decode).collect()
}

#[cfg(test)]
mod tests;
