//! The approvals projection: which tool calls wait for the user's answer.
//!
//! `approval.respond` checks it before it records an answer, and the reconciliation
//! after a restart expires what it lists.

use efr_protocol::{CallId, ConversationId, Event, EventEnvelope, Seq, TurnId};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

use crate::{StoreError, sql};

const TABLE: &str = "approvals";

/// The `status` of a request that waits for an answer.
pub(crate) const PENDING: &str = "pending";
/// The `status` of a request the user answered.
const RESOLVED: &str = "resolved";
/// The `status` of a request that can no longer be answered.
const EXPIRED: &str = "expired";

/// A tool call that waits for the user's approval.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PendingApproval {
    /// The conversation of the waiting turn.
    pub conversation_id: ConversationId,
    /// The waiting turn.
    pub turn_id: TurnId,
    /// The call that waits.
    pub call_id: CallId,
    /// What the call would do, in one line.
    pub summary: String,
    /// A diff of the change, for a call that writes a file.
    pub diff_preview: Option<String>,
    /// The sequence number of the `approval_requested` event.
    pub requested_seq: Seq,
    /// When the approval was requested.
    pub requested_at: Timestamp,
}

/// Brings the approvals up to date with one event of a conversation.
pub(crate) fn apply(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    let seq = sql::seq(envelope.seq);
    let at = sql::micros(envelope.at);
    match &envelope.event {
        Event::ApprovalRequested { turn_id, call_id, summary, diff_preview } => {
            // A call asked again starts over as pending.
            conn.execute(
                "INSERT INTO approvals (call_id, conversation_id, turn_id, summary, diff_preview, \
                 status, requested_seq, requested_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT (call_id) DO UPDATE SET conversation_id = excluded.conversation_id, \
                 turn_id = excluded.turn_id, summary = excluded.summary, \
                 diff_preview = excluded.diff_preview, status = excluded.status, decision = NULL, \
                 resolved_by = NULL, requested_seq = excluded.requested_seq, \
                 requested_at = excluded.requested_at, resolved_seq = NULL, resolved_at = NULL",
                params![
                    call_id.to_string(),
                    conversation_id.to_string(),
                    turn_id.to_string(),
                    summary,
                    diff_preview,
                    PENDING,
                    seq,
                    at
                ],
            )?;
        }
        Event::ApprovalResolved { call_id, decision, origin, .. } => {
            conn.execute(
                "UPDATE approvals SET status = ?2, decision = ?3, resolved_by = ?4, \
                 resolved_seq = ?5, resolved_at = ?6 WHERE call_id = ?1",
                params![
                    call_id.to_string(),
                    RESOLVED,
                    sql::wire_name(decision, "approval decision")?,
                    sql::wire_name(origin, "origin")?,
                    seq,
                    at
                ],
            )?;
        }
        Event::ApprovalExpired { call_id, .. } => {
            conn.execute(
                "UPDATE approvals SET status = ?2, resolved_seq = ?3, resolved_at = ?4 \
                 WHERE call_id = ?1 AND status = ?5",
                params![call_id.to_string(), EXPIRED, seq, at, PENDING],
            )?;
        }
        _ => {}
    }
    Ok(())
}

/// Empties the table before a rebuild.
pub(crate) fn clear(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("DELETE FROM approvals", [])?;
    Ok(())
}

/// The pending approvals of one conversation, or of all of them for `None`, oldest
/// first.
pub fn pending(
    conn: &Connection,
    conversation_id: Option<ConversationId>,
) -> Result<Vec<PendingApproval>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT conversation_id, turn_id, call_id, summary, diff_preview, requested_seq, \
         requested_at FROM approvals WHERE status = ?1 AND (?2 IS NULL OR conversation_id = ?2) \
         ORDER BY requested_seq",
    )?;
    let raw = stmt
        .query_map(
            params![PENDING, conversation_id.map(|id| id.to_string())],
            RawPending::from_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(RawPending::decode).collect()
}

/// The pending approval of one call, if it has one.
pub fn pending_call(
    conn: &Connection,
    call_id: CallId,
) -> Result<Option<PendingApproval>, StoreError> {
    conn.query_row(
        "SELECT conversation_id, turn_id, call_id, summary, diff_preview, requested_seq, \
         requested_at FROM approvals WHERE call_id = ?1 AND status = ?2",
        params![call_id.to_string(), PENDING],
        RawPending::from_row,
    )
    .optional()?
    .map(RawPending::decode)
    .transpose()
}

struct RawPending {
    conversation_id: String,
    turn_id: String,
    call_id: String,
    summary: String,
    diff_preview: Option<String>,
    requested_seq: i64,
    requested_at: i64,
}

impl RawPending {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(RawPending {
            conversation_id: row.get(0)?,
            turn_id: row.get(1)?,
            call_id: row.get(2)?,
            summary: row.get(3)?,
            diff_preview: row.get(4)?,
            requested_seq: row.get(5)?,
            requested_at: row.get(6)?,
        })
    }

    fn decode(self) -> Result<PendingApproval, StoreError> {
        Ok(PendingApproval {
            conversation_id: sql::parse(&self.conversation_id, TABLE, "conversation_id")?,
            turn_id: sql::parse(&self.turn_id, TABLE, "turn_id")?,
            call_id: sql::parse(&self.call_id, TABLE, "call_id")?,
            summary: self.summary,
            diff_preview: self.diff_preview,
            requested_seq: sql::to_seq(self.requested_seq, TABLE, "requested_seq")?,
            requested_at: sql::to_timestamp(self.requested_at, TABLE, "requested_at")?,
        })
    }
}

#[cfg(test)]
mod tests;
