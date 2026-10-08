//! `prompt.withdraw`: take back a prompt that waits in the queue (Alt+Up in the input
//! row of a turn).
//!
//! The conversation takes the prompt out of its queue and records `prompt_withdrawn`,
//! the last event of that turn, in one step: a prompt either starts or is withdrawn,
//! never both. A prompt that started, ended or was withdrawn is a conflict; a turn that
//! the conversation never queued, and a terminal without a queued prompt, are not
//! found. A retried command id answers from its receipt.

use efr_conversation::ConversationError;
use efr_protocol::PromptWithdraw;
use efr_transport::{ConnectionContext, Responder};

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "prompt.withdraw";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: PromptWithdraw,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    match withdraw(state, context, params).await {
        Ok(answer) => {
            responder.item(&answer).await?;
            Ok(())
        }
        Err(error @ DaemonError::Rejected { .. }) => Err(error),
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}

async fn withdraw(
    state: &State,
    context: &ConnectionContext,
    params: PromptWithdraw,
) -> Result<serde_json::Value, DaemonError> {
    let conversation_id = params.conversation_id;
    // NOTE: a conversation without a live actor has an empty queue, and the actor
    // tells a turn that ended from one that this conversation never had.
    let handle = match state.conversations.live(conversation_id) {
        Some(handle) => handle,
        None => {
            ensure_exists(state, conversation_id).await?;
            state.conversations.open(conversation_id)
        }
    };
    match handle.withdraw(params, context.surface()).await {
        Ok(result) => serde_json::to_value(result)
            .map_err(|source| DaemonError::EncodeResult { method: METHOD, source }),
        Err(ConversationError::DuplicateCommand { receipt }) => receipts::replay(METHOD, *receipt),
        Err(error) => Err(error.into()),
    }
}
