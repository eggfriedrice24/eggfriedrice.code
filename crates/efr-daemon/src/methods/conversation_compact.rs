//! `conversation.compact`: compact a conversation's context now (`efr compact`,
//! `,compact`).
//!
//! The conversation's actor prunes old tool output and writes a summary of the history
//! before the verbatim tail, records `conversation_compacted` with the command's
//! receipt, and answers when it is done. It never starts a turn. While a turn or another
//! compaction runs, and when nothing lies before the tail, it is refused with
//! `conflict`; a conversation that does not exist is `not_found`. A prompt that arrives
//! meanwhile queues behind it. A retried command id answers from its receipt. The
//! contract is in the README of efr-conversation, section "Context".

use efr_conversation::ConversationError;
use efr_protocol::ConversationCompact;
use efr_transport::Responder;

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "conversation.compact";

pub(crate) async fn handle(
    state: &State,
    params: ConversationCompact,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    match compact(state, params).await {
        Ok(answer) => {
            responder.item(&answer).await?;
            Ok(())
        }
        Err(error @ DaemonError::Rejected { .. }) => Err(error),
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}

async fn compact(
    state: &State,
    params: ConversationCompact,
) -> Result<serde_json::Value, DaemonError> {
    let conversation_id = params.conversation_id;
    tracing::info!(%conversation_id, focus = params.focus.is_some(), "manual compaction");
    // NOTE: a conversation without a live actor has no running turn; its actor starts
    // here and reads the history from the log.
    let handle = match state.conversations.live(conversation_id) {
        Some(handle) => handle,
        None => {
            ensure_exists(state, conversation_id).await?;
            state.conversations.open(conversation_id)
        }
    };
    match handle.compact(params).await {
        Ok(result) => serde_json::to_value(result)
            .map_err(|source| DaemonError::EncodeResult { method: METHOD, source }),
        Err(ConversationError::DuplicateCommand { receipt }) => receipts::replay(METHOD, *receipt),
        Err(error) => Err(error.into()),
    }
}
