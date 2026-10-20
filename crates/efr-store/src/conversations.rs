//! The conversations and turns projections.
//!
//! The writer keeps them in step with the event log inside each batch's transaction.
//! They serve `conversations.list`, the summary in a subscription snapshot, and the
//! reconciliation after a restart; the log stays the source of truth.

use std::path::PathBuf;

use efr_protocol::{
    CommandId, ConversationId, ConversationStatus, ConversationSummary, Event, EventEnvelope,
    RouteScope, Scope, Seq, TurnId,
};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{StoreError, approvals, sql};

const CONVERSATIONS: &str = "conversations";
const TURNS: &str = "turns";

/// The start of the prompt column of a turn that a job notice started, before the ids
/// of its jobs.
pub const WAKE_PROMPT: &str = "job notice: ";

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
    /// True for a turn that a job notice started, not a prompt. Its `command_id` is
    /// `None`, and its `prompt` names the jobs, such as `job notice: j3, j4`.
    pub wake: bool,
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
    if let Event::ConversationCreated { origin, tty, route } = &envelope.event {
        create(conn, conversation_id, origin, tty.as_deref(), route.as_ref(), seq, at)?;
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
                // NOTE: only a prompt from the row's own terminal makes its slot the
                // current one; the first pid that it names stays, so a nested shell
                // never takes the slot from the outer one (DV1).
                if let Some(tty) = &context.tty {
                    conn.execute(
                        "UPDATE conversations SET routed_seq = ?3, \
                         shell_pid = COALESCE(shell_pid, ?4) WHERE id = ?1 AND tty = ?2",
                        params![id, tty, seq, context.shell_pid],
                    )?;
                }
            }
        }
        Event::TerminalAttached { tty, route, shell_pid, .. } => {
            fill_routes(conn, tty)?;
            let route = sql::to_json(route, "route")?;
            conn.execute(
                "UPDATE conversations SET tty = NULL WHERE tty = ?1 AND route = ?2 AND id <> ?3",
                params![tty, route, id],
            )?;
            conn.execute(
                "UPDATE conversations SET tty = ?2, route = ?3, shell_pid = ?4, routed_seq = ?5 \
                 WHERE id = ?1",
                params![id, tty, route, shell_pid, seq],
            )?;
        }
        Event::JobNotice { turn_id, job_id, woke: true, .. } => {
            // NOTE: the first notice that starts a turn makes its row, as a prompt does
            // for its turn; the turn's `turn_started` follows in the same batch.
            conn.execute(
                "INSERT INTO turns (id, conversation_id, command_id, prompt, status, queued_seq, \
                 last_seq, wake) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?5, 1) \
                 ON CONFLICT (id) DO UPDATE SET prompt = turns.prompt || ', ' || ?6 \
                 WHERE turns.wake = 1",
                params![
                    turn_id.to_string(),
                    id,
                    format!("{WAKE_PROMPT}{job_id}"),
                    TurnStatus::Queued.as_str(),
                    seq,
                    job_id.as_str(),
                ],
            )?;
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
                "UPDATE conversations SET cwd = ?2, scope = ?3, route = COALESCE(route, ?4) \
                 WHERE id = ?1",
                params![
                    id,
                    sql::path_text(cwd),
                    sql::to_json(scope, "scope")?,
                    sql::to_json(&route_of_scope(Some(scope)), "route")?
                ],
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
        Event::TurnInterrupted { turn_id, .. } => {
            finish(conn, turn_id, TurnStatus::Interrupted, at)?
        }
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
    route: Option<&RouteScope>,
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
    let route = route.map(|route| sql::to_json(route, "route")).transpose()?;
    match (tty, &route) {
        (Some(tty), Some(route)) => {
            // A terminal has one conversation per scope key: the newest one of the key.
            fill_routes(conn, tty)?;
            conn.execute(
                "UPDATE conversations SET tty = NULL WHERE tty = ?1 AND route = ?2",
                params![tty, route],
            )?;
        }
        (Some(tty), None) => {
            // NOTE: an event from before routing by scope: the daemon of then kept one
            // conversation per terminal, so a rebuild of an old log gives its rows.
            conn.execute("UPDATE conversations SET tty = NULL WHERE tty = ?1", [tty])?;
        }
        (None, _) => {}
    }
    conn.execute(
        "INSERT INTO conversations (id, origin, status, tty, created_at, updated_at, last_seq, \
         route, routed_seq) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8)",
        params![
            id,
            sql::wire_name(origin, "origin")?,
            sql::wire_name(&ConversationStatus::Idle, "conversation status")?,
            tty,
            at,
            seq,
            route,
            tty.map(|_| seq)
        ],
    )?;
    Ok(())
}

/// Gives every row of `tty` without a route the key of its scope, as a start of the
/// daemon reads it ([`slots`]), before a rule compares routes.
fn fill_routes(conn: &Connection, tty: &str) -> Result<(), StoreError> {
    let mut stmt =
        conn.prepare("SELECT id, scope FROM conversations WHERE tty = ?1 AND route IS NULL")?;
    let rows = stmt
        .query_map([tty], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, scope) in rows {
        let route = route_of_column(None, scope)?;
        conn.execute(
            "UPDATE conversations SET route = ?2 WHERE id = ?1",
            params![id, sql::to_json(&route, "route")?],
        )?;
    }
    Ok(())
}

/// The scope key of a turn's scope: its project, or `outside` for every other place.
pub fn route_of_scope(scope: Option<&Scope>) -> RouteScope {
    match scope {
        Some(Scope::Project(id)) => RouteScope::Project(*id),
        _ => RouteScope::Outside,
    }
}

/// The scope key of a row from its `route` and `scope` columns: the route, else the
/// key of the scope. A route that this build cannot read counts as `outside`, the key
/// that never picks a conversation up.
fn route_of_column(route: Option<String>, scope: Option<String>) -> Result<RouteScope, StoreError> {
    if let Some(route) = route {
        return Ok(match serde_json::from_str(&route) {
            Ok(RouteScope::Unknown) | Err(_) => RouteScope::Outside,
            Ok(route) => route,
        });
    }
    let scope =
        scope.map(|text| sql::from_json::<Scope>(&text, CONVERSATIONS, "scope")).transpose()?;
    Ok(route_of_scope(scope.as_ref()))
}

/// One slot of a terminal as the projection holds it: the conversation that the
/// terminal `tty` keeps for the scope key `route`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SlotRow {
    /// The terminal.
    pub tty: String,
    /// The scope key.
    pub route: RouteScope,
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The pid of the shell that holds the slot, when it said.
    pub shell_pid: Option<u32>,
    /// The sequence number of the slot's newest routed event.
    pub routed_seq: Seq,
}

/// Every slot of every terminal, in one query: the rows with a tty, with no window of
/// events. A row without a route gets the key of its scope; a row without a routed
/// sequence number counts as the oldest.
pub fn slots(conn: &Connection) -> Result<Vec<SlotRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, tty, route, scope, shell_pid, COALESCE(routed_seq, 0) \
         FROM conversations WHERE tty IS NOT NULL ORDER BY last_seq",
    )?;
    let raw = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(|(id, tty, route, scope, shell_pid, routed_seq)| {
            Ok(SlotRow {
                tty,
                route: route_of_column(route, scope)?,
                conversation_id: sql::parse(&id, CONVERSATIONS, "id")?,
                shell_pid: shell_pid.and_then(|pid| u32::try_from(pid).ok()),
                routed_seq: sql::to_seq(routed_seq, CONVERSATIONS, "routed_seq")?,
            })
        })
        .collect()
}

/// The scope key of a conversation: its `route` column, else the key of its scope.
/// `None` for a conversation that does not exist.
pub fn route(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<Option<RouteScope>, StoreError> {
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT route, scope FROM conversations WHERE id = ?1",
            [conversation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(route, scope)| route_of_column(route, scope)).transpose()
}

/// The newest conversation (by its last activity) whose scope key is `route`, in any
/// status: the only candidate of a pick-up (F4).
pub fn newest_of_route(
    conn: &Connection,
    route: &RouteScope,
) -> Result<Option<ConversationSummary>, StoreError> {
    conn.query_row(
        "SELECT id, title, status, created_at, updated_at, last_seq, cwd, scope, tty \
         FROM conversations WHERE route = ?1 ORDER BY updated_at DESC, last_seq DESC LIMIT 1",
        [sql::to_json(route, "route")?],
        RawSummary::from_row,
    )
    .optional()?
    .map(RawSummary::decode)
    .transpose()
}

/// Every conversation that the terminal `tty` holds, one per scope key, its current
/// one first: the slot with the newest routed event.
pub fn of_tty(conn: &Connection, tty: &str) -> Result<Vec<ConversationSummary>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, status, created_at, updated_at, last_seq, cwd, scope, tty \
         FROM conversations WHERE tty = ?1 \
         ORDER BY COALESCE(routed_seq, 0) DESC, last_seq DESC",
    )?;
    let raw = stmt.query_map([tty], RawSummary::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawSummary::decode).collect()
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
         last_seq, wake FROM turns WHERE conversation_id = ?1 ORDER BY queued_seq",
        params![conversation_id.to_string()],
    )
}

/// One turn, with the conversation it belongs to.
pub fn turn(conn: &Connection, turn_id: TurnId) -> Result<Option<Turn>, StoreError> {
    let mut found = query_turns(
        conn,
        "SELECT id, conversation_id, command_id, prompt, status, queued_seq, started_at, ended_at, \
         last_seq, wake FROM turns WHERE id = ?1",
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
         last_seq, wake FROM turns WHERE status IN (?1, ?2, ?3) ORDER BY queued_seq",
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
    wake: i64,
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
            wake: row.get(9)?,
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
            wake: self.wake != 0,
        })
    }
}

#[cfg(test)]
mod tests;
