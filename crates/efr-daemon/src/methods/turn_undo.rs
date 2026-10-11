//! `turn.undo`: take back the file and ref changes of a turn (`efr undo`).
//!
//! The handler refuses a phone itself (`forbidden`): undo writes files on this machine,
//! and a phone holds the `operate` scope. A model-side peer holds `read` only, so the
//! scope check refuses it before this handler runs. The conversation is the one that
//! the params name, else the one of `turn_id`, else the active conversation of the
//! terminal that the connection's `hello` named, as for `conversation.diff`. The
//! candidates are the turns of the log that ran and ended, newest first; a turn that a
//! restart stopped is one too, and its undo needs `force`. The conversation's actor
//! runs the undo through the toolbox and records `undo_applied` with the receipt of an
//! apply. A refusal of the undo is `conflict`, a busy conversation is `busy`; both
//! carry their reason in the error's data. A retried command id answers from its
//! receipt.

use efr_conversation::{ConversationError, UndoBusyReason, UndoCandidate};
use efr_protocol::{ErrorBody, ErrorCode, Origin, TurnId, TurnUndo, UndoRefused};
use efr_store::conversations::{Turn, TurnStatus};
use efr_transport::{ConnectionContext, Responder};
use jiff::Timestamp;

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "turn.undo";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: TurnUndo,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    match undo(state, context, params).await {
        Ok(answer) => {
            responder.item(&answer).await?;
            Ok(())
        }
        // NOTE: a refused undo is no final answer: the next undo can work once a turn
        // ends or the user changes a file, so it keeps no receipt.
        Err(error @ DaemonError::Rejected { .. }) => Err(error),
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}

async fn undo(
    state: &State,
    context: &ConnectionContext,
    params: TurnUndo,
) -> Result<serde_json::Value, DaemonError> {
    let origin = context.surface();
    if origin == Origin::Phone {
        return Err(DaemonError::Rejected { body: phone_refusal() });
    }
    let conversation_id = match (params.conversation_id, params.turn_id) {
        (Some(conversation_id), _) => conversation_id,
        (None, Some(turn_id)) => state
            .readers
            .with(move |conn| efr_store::conversations::turn(conn, turn_id))
            .await?
            .map(|turn| turn.conversation_id)
            .ok_or(DaemonError::TurnNotFound { turn_id })?,
        (None, None) => state
            .connections
            .tty(context.conn_id())
            .and_then(|tty| state.conversations.active(&tty))
            .map(|active| active.conversation_id)
            .ok_or(DaemonError::NoActiveConversation)?,
    };
    ensure_exists(state, conversation_id).await?;
    let turns = state
        .readers
        .with(move |conn| efr_store::conversations::turns(conn, conversation_id))
        .await?;
    if let Some(turn_id) = params.turn_id
        && !turns.iter().any(|turn| turn.id == turn_id)
    {
        return Err(DaemonError::TurnNotFound { turn_id });
    }
    let candidates = candidates(&turns);
    tracing::debug!(%conversation_id, candidates = candidates.len(), "turn.undo");
    // NOTE: a conversation without a live actor has no running turn; its actor starts
    // here, as for a compaction.
    let handle = match state.conversations.live(conversation_id) {
        Some(handle) => handle,
        None => state.conversations.open(conversation_id),
    };
    let params = TurnUndo { conversation_id: Some(conversation_id), ..params };
    match handle.undo(params, candidates, origin).await {
        Ok(result) => serde_json::to_value(result)
            .map_err(|source| DaemonError::EncodeResult { method: METHOD, source }),
        Err(ConversationError::DuplicateCommand { receipt }) => receipts::replay(METHOD, *receipt),
        Err(ConversationError::UndoRefused(refused)) => {
            Err(DaemonError::Rejected { body: refusal(refused) })
        }
        Err(ConversationError::UndoBusy { reason, .. }) => {
            Err(DaemonError::Rejected { body: busy(reason) })
        }
        Err(error) => Err(error.into()),
    }
}

/// The turns of `turns` (the log's order, oldest first) that an undo may take back,
/// newest first: each that ran and ended, also one that a restart stopped while it ran.
/// A prompt that never started, or that the user took back, changed nothing.
pub(crate) fn candidates(turns: &[Turn]) -> Vec<UndoCandidate> {
    turns
        .iter()
        .rev()
        .filter_map(|turn| {
            candidate(turn.id, &turn.prompt, turn.status, turn.started_at, turn.ended_at)
        })
        .collect()
}

/// The candidate of one turn of the log; `None` for a turn that never ran or has not
/// ended. The title is the first line of the prompt.
pub(crate) fn candidate(
    turn_id: TurnId,
    prompt: &str,
    status: TurnStatus,
    started_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
) -> Option<UndoCandidate> {
    if started_at.is_none() || !status.is_finished() || status == TurnStatus::Withdrawn {
        return None;
    }
    let stopped = status == TurnStatus::Cancelled;
    let title = prompt.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned);
    Some(UndoCandidate { turn_id, title, ended: ended_at.filter(|_| !stopped), stopped })
}

/// The wire error of a refused undo: `conflict`, with the refusal as `refused` in its
/// data.
fn refusal(refused: UndoRefused) -> ErrorBody {
    let data = serde_json::json!({ "refused": refused });
    ErrorBody::new(ErrorCode::Conflict, refused.to_string()).with_data(data)
}

/// The wire error of an undo from a phone: `forbidden`, with the refusal in its data.
fn phone_refusal() -> ErrorBody {
    let refused = UndoRefused::PhoneOrigin;
    let data = serde_json::json!({ "refused": refused });
    ErrorBody::new(ErrorCode::Forbidden, refused.to_string()).with_data(data)
}

/// The wire error of an undo that must wait: `busy`, with what runs as `busy` in its
/// data (`turn`, `compaction`, `undo` or `call_runs`).
fn busy(reason: UndoBusyReason) -> ErrorBody {
    let (name, message) = match reason {
        UndoBusyReason::Turn => ("turn", "a turn of the conversation runs; undo after it ends"),
        UndoBusyReason::Compaction => {
            ("compaction", "efr compacts the conversation; undo after it ends")
        }
        UndoBusyReason::Undo => ("undo", "another undo of the conversation runs"),
        UndoBusyReason::CallRuns => (
            "call_runs",
            "a command of the conversation still runs after its timeout; undo after it ends",
        ),
        _ => ("other", "the conversation is busy; undo later"),
    };
    ErrorBody::new(ErrorCode::Busy, message).with_data(serde_json::json!({ "busy": name }))
}

#[cfg(test)]
mod tests;
