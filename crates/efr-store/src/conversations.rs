//! The conversations and turns projections.
//!
//! The writer keeps them in step with the event log inside each batch's transaction.
//! They serve `conversations.list`, the summary in a subscription snapshot, and the
//! reconciliation after a restart; the log stays the source of truth.

use std::path::PathBuf;

use efr_protocol::{
    CommandId, ConversationId, ConversationStatus, ConversationSummary, Event, EventEnvelope,
    Scope, Seq, TurnId,
};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{StoreError, approvals, sql};

const CONVERSATIONS: &str = "conversations";
const TURNS: &str = "turns";

/// The longest title taken from a prompt, in characters.
const TITLE_CHARS: usize = 80;

/// Where a turn is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TurnStatus {
    /// Waiting behind another turn.
    Queued,
    /// Held by an earlier daemon at a restart (`prompt_held`). No daemon holds a
    /// prompt any more; the next start records a held one as not run.
    Held,
    /// Running.
    Running,
    /// Finished normally.
    Completed,
    /// Ended with an error.
    Failed,
    /// Stopped by the user.
    Interrupted,
    /// Running when the daemon stopped, and cancelled at the next start.
    Cancelled,
    /// Taken back by the user before it started (`prompt_withdrawn`); it never runs.
    Withdrawn,
}

impl TurnStatus {
    /// Every status.
    pub const ALL: [TurnStatus; 8] = [
        TurnStatus::Queued,
        TurnStatus::Held,
        TurnStatus::Running,
        TurnStatus::Completed,
        TurnStatus::Failed,
        TurnStatus::Interrupted,
        TurnStatus::Cancelled,
        TurnStatus::Withdrawn,
    ];

    /// The name stored in the `status` column, such as `running`.
    pub const fn as_str(self) -> &'static str {
        match self {
            TurnStatus::Queued => "queued",
            TurnStatus::Held => "held",
            TurnStatus::Running => "running",
            TurnStatus::Completed => "completed",
            TurnStatus::Failed => "failed",
            TurnStatus::Interrupted => "interrupted",
            TurnStatus::Cancelled => "cancelled",
            TurnStatus::Withdrawn => "withdrawn",
        }
    }

    /// True for a turn that will never run again.
    pub const fn is_finished(self) -> bool {
        matches!(
            self,
            TurnStatus::Completed
                | TurnStatus::Failed
                | TurnStatus::Interrupted
                | TurnStatus::Cancelled
                | TurnStatus::Withdrawn
        )
    }

    fn from_column(text: &str) -> Result<Self, StoreError> {
        TurnStatus::ALL
            .into_iter()
            .find(|status| status.as_str() == text)
            .ok_or_else(|| sql::decode_error(TURNS, "status", sql::UnknownName(text.to_owned())))
    }
}

/// A turn as the projection holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Turn {
    /// The turn.
    pub id: TurnId,
    /// Its conversation.
    pub conversation_id: ConversationId,
    /// The client's id for the `prompt.send` that queued it.
    pub command_id: Option<CommandId>,
    /// The prompt.
    pub prompt: String,
    /// Where it is now.
    pub status: TurnStatus,
    /// The sequence number of its `prompt_queued` event.
    pub queued_seq: Seq,
    /// When it started running.
    pub started_at: Option<Timestamp>,
    /// When it finished.
    pub ended_at: Option<Timestamp>,
    /// The sequence number of its newest event.
    pub last_seq: Seq,
}

/// Brings the conversation and its turns up to date with one event of the
/// conversation.
pub(crate) fn apply(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    let id = conversation_id.to_string();
    let seq = sql::seq(envelope.seq);
    let at = sql::micros(envelope.at);
    if let Event::ConversationCreated { origin, tty } = &envelope.event {
        create(conn, conversation_id, origin, tty.as_deref(), seq, at)?;
    } else {
        let touched = conn.execute(
            "UPDATE conversations SET updated_at = ?2, last_seq = ?3 WHERE id = ?1",
            params![id, at, seq],
        )?;
        if touched == 0 {
            return Err(StoreError::UnknownConversation { conversation_id });
        }
    }

    match &envelope.event {
        Event::PromptQueued { turn_id, command_id, text, context, .. } => {
            conn.execute(
                "INSERT INTO turns (id, conversation_id, command_id, prompt, status, queued_seq, \
                 last_seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    turn_id.to_string(),
                    id,
                    command_id.to_string(),
                    text,
                    TurnStatus::Queued.as_str(),
                    seq
                ],
            )?;
            conn.execute(
                "UPDATE conversations SET title = COALESCE(title, ?2) WHERE id = ?1",
                params![id, title_from(text)],
            )?;
            if let Some(context) = context {
                conn.execute(
                    "UPDATE conversations SET cwd = ?2 WHERE id = ?1",
                    params![id, sql::path_text(&context.pwd)],
                )?;
            }
        }
        Event::PromptHeld { turn_id } => {
            conn.execute(
                "UPDATE turns SET status = ?2 WHERE id = ?1",
                params![turn_id.to_string(), TurnStatus::Held.as_str()],
            )?;
        }
        Event::TurnStarted { turn_id, cwd, scope, .. } => {
            conn.execute(
                "UPDATE turns SET status = ?2, started_at = ?3 WHERE id = ?1",
                params![turn_id.to_string(), TurnStatus::Running.as_str(), at],
            )?;
            conn.execute(
                "UPDATE conversations SET cwd = ?2, scope = ?3 WHERE id = ?1",
                params![id, sql::path_text(cwd), sql::to_json(scope, "scope")?],
            )?;
        }
        Event::ScopeChanged { to, .. } => {
            conn.execute(
                "UPDATE conversations SET scope = ?2 WHERE id = ?1",
                params![id, sql::to_json(to, "scope")?],
            )?;
        }
        Event::TurnCompleted { turn_id, .. } => finish(conn, turn_id, TurnStatus::Completed, at)?,
        Event::TurnFailed { turn_id, .. } => finish(conn, turn_id, TurnStatus::Failed, at)?,
        Event::TurnInterrupted { turn_id } => finish(conn, turn_id, TurnStatus::Interrupted, at)?,
        Event::TurnCancelled { turn_id } => finish(conn, turn_id, TurnStatus::Cancelled, at)?,
        Event::PromptWithdrawn { turn_id, .. } => {
            finish(conn, turn_id, TurnStatus::Withdrawn, at)?;
        }
        _ => {}
    }

    if let Some(turn_id) = envelope.event.turn_id() {
        conn.execute(
            "UPDATE turns SET last_seq = ?2 WHERE id = ?1",
            params![turn_id.to_string(), seq],
        )?;
    }
    Ok(())
}

fn create(
    conn: &Connection,
    conversation_id: ConversationId,
    origin: &efr_protocol::Origin,
    tty: Option<&str>,
    seq: i64,
    at: i64,
) -> Result<(), StoreError> {
    let id = conversation_id.to_string();
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM conversations WHERE id = ?1)",
        [&id],
        |row| row.get(0),
    )?;
    if exists {
        return Err(StoreError::ConversationExists { conversation_id });
    }
    if let Some(tty) = tty {
        // A terminal has one active conversation: the newest one started from it.
        conn.execute("UPDATE conversations SET tty = NULL WHERE tty = ?1", [tty])?;
    }
    conn.execute(
        "INSERT INTO conversations (id, origin, status, tty, created_at, updated_at, last_seq) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        params![
            id,
            sql::wire_name(origin, "origin")?,
            sql::wire_name(&ConversationStatus::Idle, "conversation status")?,
            tty,
            at,
            seq
        ],
    )?;
    Ok(())
}

fn finish(
    conn: &Connection,
    turn_id: &TurnId,
    status: TurnStatus,
    at: i64,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE turns SET status = ?2, ended_at = ?3 WHERE id = ?1",
        params![turn_id.to_string(), status.as_str(), at],
    )?;
    Ok(())
}

/// Recomputes what the conversation is doing from its turns and approvals, so the
/// status never depends on the order in which events arrived.
pub(crate) fn refresh_status(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE conversations SET status = CASE \
           WHEN EXISTS (SELECT 1 FROM approvals a JOIN turns t ON t.id = a.turn_id \
                        WHERE a.conversation_id = ?1 AND a.status = ?2 AND t.status = ?3) THEN ?4 \
           WHEN EXISTS (SELECT 1 FROM turns WHERE conversation_id = ?1 AND status = ?3) THEN ?5 \
           ELSE ?6 END \
         WHERE id = ?1",
        params![
            conversation_id.to_string(),
            approvals::PENDING,
            TurnStatus::Running.as_str(),
            sql::wire_name(&ConversationStatus::AwaitingApproval, "conversation status")?,
            sql::wire_name(&ConversationStatus::Running, "conversation status")?,
            sql::wire_name(&ConversationStatus::Idle, "conversation status")?,
        ],
    )?;
    Ok(())
}

/// Empties both tables before a rebuild; turns first, for the foreign key.
pub(crate) fn clear(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("DELETE FROM turns; DELETE FROM conversations;")?;
    Ok(())
}

/// A title from a prompt: its first line that is not blank, trimmed, shortened to 80
/// characters. `None` for a blank prompt.
pub(crate) fn title_from(prompt: &str) -> Option<String> {
    let line = prompt.lines().map(str::trim).find(|line| !line.is_empty())?;
    if line.chars().count() <= TITLE_CHARS {
        return Some(line.to_owned());
    }
    let mut title: String = line.chars().take(TITLE_CHARS - 3).collect();
    title.truncate(title.trim_end().len());
    title.push_str("...");
    Some(title)
}

/// Up to `limit` conversations, the most recently updated first, that were last
/// updated before the event `before` (all of them when `before` is `None`). The
/// `last_seq` of the last summary is the `before` of the next page.
pub fn list(
    conn: &Connection,
    before: Option<Seq>,
    limit: u32,
) -> Result<Vec<ConversationSummary>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, status, created_at, updated_at, last_seq, cwd, scope, tty \
         FROM conversations WHERE last_seq < ?1 ORDER BY last_seq DESC LIMIT ?2",
    )?;
    let raw = stmt
        .query_map(params![before.map_or(i64::MAX, sql::seq), limit], RawSummary::from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawSummary::decode).collect()
}

/// One conversation, as a list shows it.
pub fn get(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<Option<ConversationSummary>, StoreError> {
    conn.query_row(
        "SELECT id, title, status, created_at, updated_at, last_seq, cwd, scope, tty \
         FROM conversations WHERE id = ?1",
        [conversation_id.to_string()],
        RawSummary::from_row,
    )
    .optional()?
    .map(RawSummary::decode)
    .transpose()
}

/// Every turn of a conversation, oldest first.
pub fn turns(conn: &Connection, conversation_id: ConversationId) -> Result<Vec<Turn>, StoreError> {
    query_turns(
        conn,
        "SELECT id, conversation_id, command_id, prompt, status, queued_seq, started_at, ended_at, \
         last_seq FROM turns WHERE conversation_id = ?1 ORDER BY queued_seq",
        params![conversation_id.to_string()],
    )
}

/// One turn, with the conversation it belongs to.
pub fn turn(conn: &Connection, turn_id: TurnId) -> Result<Option<Turn>, StoreError> {
    let mut found = query_turns(
        conn,
        "SELECT id, conversation_id, command_id, prompt, status, queued_seq, started_at, ended_at, \
         last_seq FROM turns WHERE id = ?1",
        params![turn_id.to_string()],
    )?;
    Ok(found.pop())
}

/// Every turn that has not finished (queued, held or running), across conversations,
/// oldest first: what the reconciliation after a restart looks at.
pub fn unfinished_turns(conn: &Connection) -> Result<Vec<Turn>, StoreError> {
    query_turns(
        conn,
        "SELECT id, conversation_id, command_id, prompt, status, queued_seq, started_at, ended_at, \
         last_seq FROM turns WHERE status IN (?1, ?2, ?3) ORDER BY queued_seq",
        params![
            TurnStatus::Queued.as_str(),
            TurnStatus::Held.as_str(),
            TurnStatus::Running.as_str()
        ],
    )
}

fn query_turns(
    conn: &Connection,
    statement: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Turn>, StoreError> {
    let mut stmt = conn.prepare(statement)?;
    let raw = stmt.query_map(params, RawTurn::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawTurn::decode).collect()
}

struct RawSummary {
    id: String,
    title: Option<String>,
    status: String,
    created_at: i64,
    updated_at: i64,
    last_seq: i64,
    cwd: Option<String>,
    scope: Option<String>,
    tty: Option<String>,
}

impl RawSummary {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            status: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
            last_seq: row.get(5)?,
            cwd: row.get(6)?,
            scope: row.get(7)?,
            tty: row.get(8)?,
        })
    }

    fn decode(self) -> Result<ConversationSummary, StoreError> {
        Ok(ConversationSummary {
            id: sql::parse(&self.id, CONVERSATIONS, "id")?,
            title: self.title,
            status: sql::from_wire(&self.status, CONVERSATIONS, "status")?,
            created_at: sql::to_timestamp(self.created_at, CONVERSATIONS, "created_at")?,
            updated_at: sql::to_timestamp(self.updated_at, CONVERSATIONS, "updated_at")?,
            last_seq: sql::to_seq(self.last_seq, CONVERSATIONS, "last_seq")?,
            cwd: self.cwd.map(PathBuf::from),
            scope: self
                .scope
                .map(|text| sql::from_json::<Scope>(&text, CONVERSATIONS, "scope"))
                .transpose()?,
            tty: self.tty,
        })
    }
}

struct RawTurn {
    id: String,
    conversation_id: String,
    command_id: Option<String>,
    prompt: String,
    status: String,
    queued_seq: i64,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    last_seq: i64,
}

impl RawTurn {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawTurn {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            command_id: row.get(2)?,
            prompt: row.get(3)?,
            status: row.get(4)?,
            queued_seq: row.get(5)?,
            started_at: row.get(6)?,
            ended_at: row.get(7)?,
            last_seq: row.get(8)?,
        })
    }

    fn decode(self) -> Result<Turn, StoreError> {
        Ok(Turn {
            id: sql::parse(&self.id, TURNS, "id")?,
            conversation_id: sql::parse(&self.conversation_id, TURNS, "conversation_id")?,
            command_id: self
                .command_id
                .map(|text| sql::parse(&text, TURNS, "command_id"))
                .transpose()?,
            prompt: self.prompt,
            status: TurnStatus::from_column(&self.status)?,
            queued_seq: sql::to_seq(self.queued_seq, TURNS, "queued_seq")?,
            started_at: self
                .started_at
                .map(|value| sql::to_timestamp(value, TURNS, "started_at"))
                .transpose()?,
            ended_at: self
                .ended_at
                .map(|value| sql::to_timestamp(value, TURNS, "ended_at"))
                .transpose()?,
            last_seq: sql::to_seq(self.last_seq, TURNS, "last_seq")?,
        })
    }
}

#[cfg(test)]
mod tests;
