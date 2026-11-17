//! The waits projection: what a running turn waits for apart from an approval. It
//! serves only the `attention` of a conversation ([`crate::conversations`]).
//!
//! A row has one of three kinds. `input`: a call waits for a line
//! (`tool_call_input_changed` with a `visible` or `hidden` wait), by its call id; the
//! wait `none` and the call's `tool_call_completed` remove it. `question`: a question of
//! the model (`user_question_requested`), by its question id; its
//! `user_question_answered` removes it. `surface`: a quarantine question
//! (`surface_question_requested`), by its question id; its `surface_question_answered`
//! removes it. The end of a turn removes all rows of the turn, so no row outlives its
//! turn.
//!
//! A command that a `shell` call left running in the hidden shell reports its input
//! with `shell_command_input_changed`. That event belongs to no turn, and this
//! projection does not read it.

use efr_protocol::{ConversationId, Event, EventEnvelope, InputWait, TurnId};
use rusqlite::{Connection, params};

use crate::StoreError;

/// The kind of a call that waits for a line.
pub(crate) const INPUT: &str = "input";
/// The kind of a question of the model.
pub(crate) const QUESTION: &str = "question";
/// The kind of a quarantine question.
pub(crate) const SURFACE: &str = "surface";

/// Brings the waits up to date with one event of a conversation.
pub(crate) fn apply(
    conn: &Connection,
    conversation_id: ConversationId,
    envelope: &EventEnvelope,
) -> Result<(), StoreError> {
    match &envelope.event {
        Event::ToolCallInputChanged { turn_id, call_id, input, .. } => match input {
            InputWait::None => remove(conn, conversation_id, &call_id.to_string()),
            _ => add(conn, conversation_id, turn_id, &call_id.to_string(), INPUT),
        },
        Event::ToolCallCompleted { call_id, .. } => {
            remove(conn, conversation_id, &call_id.to_string())
        }
        Event::UserQuestionRequested { turn_id, question_id, .. } => {
            add(conn, conversation_id, turn_id, &question_id.to_string(), QUESTION)
        }
        Event::SurfaceQuestionRequested { turn_id, question_id, .. } => {
            add(conn, conversation_id, turn_id, &question_id.to_string(), SURFACE)
        }
        Event::UserQuestionAnswered { question_id, .. }
        | Event::SurfaceQuestionAnswered { question_id, .. } => {
            remove(conn, conversation_id, &question_id.to_string())
        }
        Event::TurnCompleted { turn_id, .. }
        | Event::TurnFailed { turn_id, .. }
        | Event::TurnInterrupted { turn_id, .. }
        | Event::TurnCancelled { turn_id } => {
            conn.execute(
                "DELETE FROM waits WHERE conversation_id = ?1 AND turn_id = ?2",
                params![conversation_id.to_string(), turn_id.to_string()],
            )?;
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Adds the wait `id` of `kind`. A wait that is already there stays as it is: a call
/// that goes from a visible to a hidden wait still waits.
fn add(
    conn: &Connection,
    conversation_id: ConversationId,
    turn_id: &TurnId,
    id: &str,
    kind: &str,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO waits (id, conversation_id, turn_id, kind) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (id) DO NOTHING",
        params![id, conversation_id.to_string(), turn_id.to_string(), kind],
    )?;
    Ok(())
}

fn remove(conn: &Connection, conversation_id: ConversationId, id: &str) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM waits WHERE id = ?1 AND conversation_id = ?2",
        params![id, conversation_id.to_string()],
    )?;
    Ok(())
}

/// Empties the table before a rebuild.
pub(crate) fn clear(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("DELETE FROM waits", [])?;
    Ok(())
}

#[cfg(test)]
mod tests;
