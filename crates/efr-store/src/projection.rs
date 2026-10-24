//! Keeping the projections in step with the event log.
//!
//! [`apply`] runs for every event inside the writer's transaction, so a projection
//! never shows an event that did not commit. [`rebuild`] throws the projections away
//! and replays the whole log; a test checks that the result equals what the writer
//! built incrementally.

use std::collections::HashSet;

use efr_protocol::{Event, EventEnvelope, Seq};
use rusqlite::Connection;

use crate::{StoreError, approvals, compactions, conversations, deletes, events, jobs, shells};

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
    if let Event::ConversationDeleted { .. } = envelope.event {
        return deletes::tombstone(conn, conversation_id, envelope);
    }
    conversations::apply(conn, conversation_id, envelope)?;
    approvals::apply(conn, conversation_id, envelope)?;
    shells::apply(conn, conversation_id, envelope)?;
    compactions::apply(conn, conversation_id, envelope)?;
    jobs::apply(conn, conversation_id, envelope)?;
    conversations::refresh_status(conn, conversation_id)
}

/// Empties every projection and replays the log into it. Runs inside a transaction.
/// The events of a deleted conversation give no rows: the log may still hold some of
/// them while the delete runs, and its tombstone event alone after it.
pub(crate) fn rebuild(conn: &Connection) -> Result<(), StoreError> {
    let deleted: HashSet<String> = deletes::deleted_ids(conn)?.into_iter().collect();
    approvals::clear(conn)?;
    shells::clear(conn)?;
    compactions::clear(conn)?;
    jobs::clear(conn)?;
    conversations::clear(conn)?;
    let mut after = Seq::ZERO;
    loop {
        let page = events::read_after(conn, after, REBUILD_PAGE)?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after = last.seq;
        for envelope in &page {
            let gone = envelope.conversation_id.is_some_and(|id| deleted.contains(&id.to_string()));
            if !gone {
                apply(conn, envelope)?;
            }
        }
    }
}

/// Events that may come without a conversation id: a login, a logout, and a change of
/// the sandbox probe, which concerns every conversation. An unknown kind may be either,
/// so it is accepted both ways.
fn belongs_to_no_conversation(event: &Event) -> bool {
    matches!(
        event,
        Event::LoginCompleted { .. }
            | Event::LogoutCompleted { .. }
            | Event::SandboxUnavailable { .. }
            | Event::Unknown { .. }
    )
}
