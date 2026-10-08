//! Keeping the projections in step with the event log.
//!
//! [`apply`] runs for every event inside the writer's transaction, so a projection
//! never shows an event that did not commit. [`rebuild`] throws the projections away
//! and replays the whole log; a test checks that the result equals what the writer
//! built incrementally.

use efr_protocol::{Event, EventEnvelope, Seq};
use rusqlite::Connection;

use crate::{StoreError, approvals, compactions, conversations, events, shells};

/// How many events a rebuild reads at a time.
const REBUILD_PAGE: u32 = 512;

/// Applies one event to every projection.
pub(crate) fn apply(conn: &Connection, envelope: &EventEnvelope) -> Result<(), StoreError> {
    let Some(conversation_id) = envelope.conversation_id else {
        if belongs_to_no_conversation(&envelope.event) {
            return Ok(());
        }
        return Err(StoreError::MissingConversation { kind: envelope.event.kind().to_owned() });
    };
    conversations::apply(conn, conversation_id, envelope)?;
    approvals::apply(conn, conversation_id, envelope)?;
    shells::apply(conn, conversation_id, envelope)?;
    compactions::apply(conn, conversation_id, envelope)?;
    conversations::refresh_status(conn, conversation_id)
}

/// Empties every projection and replays the log into it. Runs inside a transaction.
pub(crate) fn rebuild(conn: &Connection) -> Result<(), StoreError> {
    approvals::clear(conn)?;
    shells::clear(conn)?;
    compactions::clear(conn)?;
    conversations::clear(conn)?;
    let mut after = Seq::ZERO;
    loop {
        let page = events::read_after(conn, after, REBUILD_PAGE)?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after = last.seq;
        for envelope in &page {
            apply(conn, envelope)?;
        }
    }
}

/// Events that may come without a conversation id: a login, and a change of the
/// sandbox probe, which concerns every conversation. An unknown kind may be either, so
/// it is accepted both ways.
fn belongs_to_no_conversation(event: &Event) -> bool {
    matches!(
        event,
        Event::LoginCompleted { .. } | Event::SandboxUnavailable { .. } | Event::Unknown { .. }
    )
}
