//! The shells projection: the hidden shell of each conversation.
//!
//! The daemon reads it to reattach or reap shells and to answer `pty.attach` for a
//! conversation's PTY.

use std::path::PathBuf;

use efr_protocol::{ConversationId, Event, EventEnvelope, PtyId, Seq};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{StoreError, sql};

const TABLE: &str = "shells";

/// Whether a shell still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellStatus {
    /// Started and not exited.
    Running,
    /// Exited.
    Exited,
}

impl ShellStatus {
    /// The name stored in the `status` column.
    pub const fn as_str(self) -> &'static str {
        match self {
            ShellStatus::Running => "running",
            ShellStatus::Exited => "exited",
        }
    }

    fn from_column(text: &str) -> Result<Self, StoreError> {
        [ShellStatus::Running, ShellStatus::Exited]
            .into_iter()
            .find(|status| status.as_str() == text)
            .ok_or_else(|| sql::decode_error(TABLE, "status", sql::UnknownName(text.to_owned())))
    }
}

/// A hidden shell as the projection holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Shell {
    /// The shell's PTY.
    pub pty_id: PtyId,
    /// The conversation it belongs to.
    pub conversation_id: ConversationId,
    /// Whether it still runs.
    pub status: ShellStatus,
    /// Its working directory: where it started, or its latest OSC 7 report.
    pub cwd: PathBuf,
    /// The host in the latest OSC 7 report, when it named one.
    pub host: Option<String>,
    /// Its process id.
    pub pid: Option<u32>,
    /// Its exit code; absent while it runs or when a signal ended it.
    pub exit_code: Option<i32>,
    /// The sequence number of its `shell_started` event.
    pub started_seq: Seq,
    /// When it started.
    pub started_at: Timestamp,
    /// The sequence number of its `shell_exited` event.
    pub exited_seq: Option<Seq>,
    /// When it exited.
    pub exited_at: Option<Timestamp>,
}

/// Brings the shells up to date with one event of a conversation.
pub(crate) fn apply(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    let seq = sql::seq(envelope.seq);
    let at = sql::micros(envelope.at);
    match &envelope.event {
        Event::ShellStarted { pty_id, cwd, pid } => {
            conn.execute(
                "INSERT INTO shells (pty_id, conversation_id, status, cwd, pid, started_seq, \
                 started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    pty_id.to_string(),
                    conversation_id.to_string(),
                    ShellStatus::Running.as_str(),
                    sql::path_text(cwd),
                    pid,
                    seq,
                    at
                ],
            )?;
        }
        Event::ShellExited { pty_id, exit_code } => {
            conn.execute(
                "UPDATE shells SET status = ?2, exit_code = ?3, exited_seq = ?4, exited_at = ?5 \
                 WHERE pty_id = ?1",
                params![pty_id.to_string(), ShellStatus::Exited.as_str(), exit_code, seq, at],
            )?;
        }
        Event::CwdChanged { pty_id, cwd, host } => {
            conn.execute(
                "UPDATE shells SET cwd = ?2, host = ?3 WHERE pty_id = ?1",
                params![pty_id.to_string(), sql::path_text(cwd), host],
            )?;
        }
        _ => {}
    }
    Ok(())
}

/// Empties the table before a rebuild.
pub(crate) fn clear(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("DELETE FROM shells", [])?;
    Ok(())
}

/// The shells of one conversation, oldest first.
pub fn for_conversation(
    conn: &Connection,
    conversation_id: ConversationId,
) -> Result<Vec<Shell>, StoreError> {
    query(
        conn,
        "SELECT pty_id, conversation_id, status, cwd, host, pid, exit_code, started_seq, \
         started_at, exited_seq, exited_at FROM shells WHERE conversation_id = ?1 \
         ORDER BY started_seq",
        params![conversation_id.to_string()],
    )
}

/// Every shell that has not exited, oldest first.
pub fn running(conn: &Connection) -> Result<Vec<Shell>, StoreError> {
    query(
        conn,
        "SELECT pty_id, conversation_id, status, cwd, host, pid, exit_code, started_seq, \
         started_at, exited_seq, exited_at FROM shells WHERE status = ?1 ORDER BY started_seq",
        params![ShellStatus::Running.as_str()],
    )
}

/// One shell by its PTY.
pub fn get(conn: &Connection, pty_id: PtyId) -> Result<Option<Shell>, StoreError> {
    conn.query_row(
        "SELECT pty_id, conversation_id, status, cwd, host, pid, exit_code, started_seq, \
         started_at, exited_seq, exited_at FROM shells WHERE pty_id = ?1",
        [pty_id.to_string()],
        RawShell::from_row,
    )
    .optional()?
    .map(RawShell::decode)
    .transpose()
}

fn query(
    conn: &Connection,
    statement: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Shell>, StoreError> {
    let mut stmt = conn.prepare(statement)?;
    let raw = stmt.query_map(params, RawShell::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawShell::decode).collect()
}

struct RawShell {
    pty_id: String,
    conversation_id: String,
    status: String,
    cwd: String,
    host: Option<String>,
    pid: Option<i64>,
    exit_code: Option<i32>,
    started_seq: i64,
    started_at: i64,
    exited_seq: Option<i64>,
    exited_at: Option<i64>,
}

impl RawShell {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawShell {
            pty_id: row.get(0)?,
            conversation_id: row.get(1)?,
            status: row.get(2)?,
            cwd: row.get(3)?,
            host: row.get(4)?,
            pid: row.get(5)?,
            exit_code: row.get(6)?,
            started_seq: row.get(7)?,
            started_at: row.get(8)?,
            exited_seq: row.get(9)?,
            exited_at: row.get(10)?,
        })
    }

    fn decode(self) -> Result<Shell, StoreError> {
        Ok(Shell {
            pty_id: sql::parse(&self.pty_id, TABLE, "pty_id")?,
            conversation_id: sql::parse(&self.conversation_id, TABLE, "conversation_id")?,
            status: ShellStatus::from_column(&self.status)?,
            cwd: PathBuf::from(self.cwd),
            host: self.host,
            pid: self
                .pid
                .map(|pid| {
                    u32::try_from(pid).map_err(|source| sql::decode_error(TABLE, "pid", source))
                })
                .transpose()?,
            exit_code: self.exit_code,
            started_seq: sql::to_seq(self.started_seq, TABLE, "started_seq")?,
            started_at: sql::to_timestamp(self.started_at, TABLE, "started_at")?,
            exited_seq: self
                .exited_seq
                .map(|value| sql::to_seq(value, TABLE, "exited_seq"))
                .transpose()?,
            exited_at: self
                .exited_at
                .map(|value| sql::to_timestamp(value, TABLE, "exited_at"))
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests;
