//! The jobs projection (phase C): one row per background job, from its `job_started`,
//! `job_ended` and `job_watch_matched` events, and one row per event that a job notice
//! tells about, with the turn that took the notice.
//!
//! efrd seeds the ids of new jobs from it ([`next_number`]), finds the jobs that ran
//! when it stopped ([`running`]), with the launcher's process id and start time from
//! the event and never from a file, and hands each actor the notices that no turn took
//! ([`undelivered_notices`]). The turns that a notice started are rows of `turns` with
//! `wake = 1` (`conversations.rs`); [`wake_starts_since`] counts them for the limit of
//! such turns per hour, so a restart does not reset it.

use efr_protocol::{ConversationId, Event, EventEnvelope, JobInfo, JobState, Seq};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{StoreError, sql};

const JOBS: &str = "jobs";
const NOTICES: &str = "job_notices";

/// The columns of a [`JobRow`], in the order that [`RawJob::from_row`] reads them.
const COLUMNS: &str = "info, start_seq, launcher_pid, launcher_start";

/// One job as the projection holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct JobRow {
    /// The job, as its events tell it now.
    pub info: JobInfo,
    /// The sequence number of its `job_started` event.
    pub start_seq: Seq,
    /// The process id of its launcher.
    pub launcher_pid: u32,
    /// The start time of the launcher's process, in clock ticks after boot.
    pub launcher_start: u64,
}

/// A notice that no turn took yet: the event that it tells about, with its job.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UndeliveredNotice {
    /// The conversation of the job.
    pub conversation_id: ConversationId,
    /// The job, as its events tell it now.
    pub job: JobInfo,
    /// The sequence number of the event.
    pub source_seq: Seq,
    /// The `job_ended` or `job_watch_matched` event.
    pub event: Event,
}

/// Brings the jobs projection up to date with one event of the conversation; other
/// events change nothing.
pub(crate) fn apply(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    let id = conversation_id.to_string();
    let seq = sql::seq(envelope.seq);
    match &envelope.event {
        Event::JobStarted { job, launcher_pid, launcher_start, .. } => {
            let info = JobInfo { conversation_id, ..job.clone() };
            conn.execute(
                "INSERT INTO jobs (conversation_id, number, turn_id, call_id, command, launch, \
                 state, started_at, ends_by, ended_at, exit_code, stopped_by, output_bytes, \
                 launcher_pid, launcher_start, start_seq, info) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    id,
                    number(&info),
                    info.turn_id.to_string(),
                    info.call_id.to_string(),
                    info.command,
                    sql::to_json(&info.launch, "launch")?,
                    sql::wire_name(&info.state, "job state")?,
                    sql::micros(info.started_at),
                    sql::micros(info.ends_by),
                    info.ended_at.map(sql::micros),
                    info.exit_code,
                    info.stopped_by.map(|by| sql::wire_name(&by, "job stop")).transpose()?,
                    bytes(info.output_bytes),
                    i64::from(*launcher_pid),
                    i64::try_from(*launcher_start).unwrap_or(i64::MAX),
                    seq,
                    sql::to_json(&info, "job")?,
                ],
            )?;
        }
        Event::JobEnded { job_id, state, exit_code, stopped_by, output_bytes, .. } => {
            let key = (id.clone(), job_number(job_id));
            notice_source(conn, &id, key.1, envelope)?;
            update(conn, &key, |info| {
                info.state = *state;
                info.exit_code = *exit_code;
                info.stopped_by = *stopped_by;
                info.output_bytes = *output_bytes;
                info.ended_at = Some(sql::truncate_to_micros(envelope.at));
            })?;
        }
        Event::JobWatchMatched { job_id, .. } => {
            let key = (id.clone(), job_number(job_id));
            notice_source(conn, &id, key.1, envelope)?;
            update(conn, &key, |info| info.wakes = info.wakes.saturating_add(1))?;
        }
        Event::JobNotice { turn_id, source_seq, .. } => {
            conn.execute(
                "UPDATE job_notices SET taken_by = ?2 WHERE source_seq = ?1",
                params![sql::seq(*source_seq), turn_id.to_string()],
            )?;
        }
        _ => {}
    }
    Ok(())
}

/// Empties both tables before a rebuild.
pub(crate) fn clear(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("DELETE FROM jobs; DELETE FROM job_notices;")?;
    Ok(())
}

/// The number of `job`'s id; a valid id always has one.
fn number(job: &JobInfo) -> i64 {
    job_number(&job.id)
}

fn job_number(id: &efr_protocol::JobId) -> i64 {
    id.number().map_or(0, i64::from)
}

/// A byte count as a column value.
fn bytes(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Records the event of `envelope`, a job's end or watch match, as one that a notice
/// tells about and that no turn took yet.
fn notice_source(
    conn: &Connection,
    conversation: &str,
    number: i64,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO job_notices (source_seq, conversation_id, number, at) VALUES (?1, ?2, ?3, ?4)",
        params![sql::seq(envelope.seq), conversation, number, sql::micros(envelope.at)],
    )?;
    Ok(())
}

/// Changes the stored `JobInfo` of the job `key` with `change`, and its columns with
/// it. An event of a job that the projection does not hold changes nothing.
fn update(
    conn: &Connection,
    key: &(String, i64),
    change: impl FnOnce(&mut JobInfo),
) -> Result<(), StoreError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT info FROM jobs WHERE conversation_id = ?1 AND number = ?2",
            params![key.0, key.1],
            |row| row.get(0),
        )
        .optional()?;
    let Some(stored) = stored else {
        return Ok(());
    };
    let mut info: JobInfo = sql::from_json(&stored, JOBS, "info")?;
    change(&mut info);
    conn.execute(
        "UPDATE jobs SET state = ?3, ended_at = ?4, exit_code = ?5, stopped_by = ?6, \
         output_bytes = ?7, info = ?8 WHERE conversation_id = ?1 AND number = ?2",
        params![
            key.0,
            key.1,
            sql::wire_name(&info.state, "job state")?,
            info.ended_at.map(sql::micros),
            info.exit_code,
            info.stopped_by.map(|by| sql::wire_name(&by, "job stop")).transpose()?,
            bytes(info.output_bytes),
            sql::to_json(&info, "job")?,
        ],
    )?;
    Ok(())
}

/// Up to `limit` jobs of `conversation`, newest first.
pub fn of_conversation(
    conn: &Connection,
    conversation: ConversationId,
    limit: u32,
) -> Result<Vec<JobRow>, StoreError> {
    query(
        conn,
        &format!(
            "SELECT {COLUMNS} FROM jobs WHERE conversation_id = ?1 ORDER BY start_seq DESC \
             LIMIT ?2"
        ),
        params![conversation.to_string(), limit],
    )
}

/// Up to `limit` jobs of every conversation, newest first.
pub fn all(conn: &Connection, limit: u32) -> Result<Vec<JobRow>, StoreError> {
    query(
        conn,
        &format!("SELECT {COLUMNS} FROM jobs ORDER BY start_seq DESC LIMIT ?1"),
        params![limit],
    )
}

/// Every job of every conversation whose state is still `running`, oldest first: what
/// efrd finds at its start, after it stopped while they ran.
pub fn running(conn: &Connection) -> Result<Vec<JobRow>, StoreError> {
    query(
        conn,
        &format!("SELECT {COLUMNS} FROM jobs WHERE state = ?1 ORDER BY start_seq"),
        params![sql::wire_name(&JobState::Running, "job state")?],
    )
}

/// The number of the next job of `conversation`: one more than the highest number that
/// it used, or 1 for its first job. efrd seeds its ids from it, so an id never repeats
/// after a restart.
pub fn next_number(conn: &Connection, conversation: ConversationId) -> Result<u32, StoreError> {
    let highest: Option<i64> = conn.query_row(
        "SELECT MAX(number) FROM jobs WHERE conversation_id = ?1",
        [conversation.to_string()],
        |row| row.get(0),
    )?;
    let highest = highest.map_or(0, |highest| u32::try_from(highest).unwrap_or(u32::MAX));
    Ok(highest.saturating_add(1))
}

/// When each turn that a job notice started began, at or after `since`, oldest first:
/// of `conversation`, or of every conversation for `None`. The limits of such turns per
/// hour count them.
pub fn wake_starts_since(
    conn: &Connection,
    conversation: Option<ConversationId>,
    since: Timestamp,
) -> Result<Vec<Timestamp>, StoreError> {
    let since = sql::micros(since);
    let mut stmt;
    let rows = match conversation {
        Some(conversation) => {
            stmt = conn.prepare(
                "SELECT started_at FROM turns WHERE wake = 1 AND started_at >= ?1 \
                 AND conversation_id = ?2 ORDER BY started_at",
            )?;
            stmt.query_map(params![since, conversation.to_string()], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?
        }
        None => {
            stmt = conn.prepare(
                "SELECT started_at FROM turns WHERE wake = 1 AND started_at >= ?1 \
                 ORDER BY started_at",
            )?;
            stmt.query_map(params![since], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    rows.into_iter().map(|at| sql::to_timestamp(at, "turns", "started_at")).collect()
}

/// Every event that a job notice tells about, recorded at or after `since`, that no
/// turn took yet, oldest first, with its job. Each needs a `job_notice` still.
pub fn undelivered_notices(
    conn: &Connection,
    since: Timestamp,
) -> Result<Vec<UndeliveredNotice>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT n.source_seq, n.conversation_id, j.info, e.payload FROM job_notices n \
         JOIN jobs j ON j.conversation_id = n.conversation_id AND j.number = n.number \
         JOIN events e ON e.seq = n.source_seq \
         WHERE n.taken_by IS NULL AND n.at >= ?1 ORDER BY n.source_seq",
    )?;
    let rows = stmt
        .query_map([sql::micros(since)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(seq, conversation, info, payload)| {
            let source_seq = sql::to_seq(seq, NOTICES, "source_seq")?;
            let event: Event = serde_json::from_str(&payload)
                .map_err(|source| StoreError::DecodeEvent { seq: source_seq, source })?;
            Ok(UndeliveredNotice {
                conversation_id: sql::parse(&conversation, NOTICES, "conversation_id")?,
                job: sql::from_json(&info, JOBS, "info")?,
                source_seq,
                event,
            })
        })
        .collect()
}

fn query(
    conn: &Connection,
    statement: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<JobRow>, StoreError> {
    let mut stmt = conn.prepare(statement)?;
    let raw = stmt.query_map(params, RawJob::from_row)?.collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawJob::decode).collect()
}

/// A row as SQLite returns it, decoded after the statement finishes so that decode
/// errors are store errors.
struct RawJob {
    info: String,
    start_seq: i64,
    launcher_pid: i64,
    launcher_start: i64,
}

impl RawJob {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawJob {
            info: row.get(0)?,
            start_seq: row.get(1)?,
            launcher_pid: row.get(2)?,
            launcher_start: row.get(3)?,
        })
    }

    fn decode(self) -> Result<JobRow, StoreError> {
        Ok(JobRow {
            info: sql::from_json(&self.info, JOBS, "info")?,
            start_seq: sql::to_seq(self.start_seq, JOBS, "start_seq")?,
            launcher_pid: u32::try_from(self.launcher_pid)
                .map_err(|source| sql::decode_error(JOBS, "launcher_pid", source))?,
            launcher_start: u64::try_from(self.launcher_start)
                .map_err(|source| sql::decode_error(JOBS, "launcher_start", source))?,
        })
    }
}

#[cfg(test)]
mod tests;
