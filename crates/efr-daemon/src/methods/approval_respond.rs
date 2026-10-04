//! `approval.respond`: the user's answer to a tool call that waits for approval.
//!
//! The store refuses an answer to a call that is not pending, so of two racing answers
//! only one commits, and an approval that expired at a restart answers `not_found`.

use efr_conversation::ConversationError;
use efr_protocol::ApprovalRespond;
use efr_transport::{ConnectionContext, Responder};

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "approval.respond";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: ApprovalRespond,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    let conversation_id = params.conversation_id;
    let call_id = params.call_id;
    let outcome = match state.conversations.live(conversation_id) {
        Some(handle) => match handle.respond_approval(params, context.surface()).await {
            Ok(result) => Ok(serde_json::to_value(result)
                .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?),
            Err(ConversationError::DuplicateCommand { receipt }) => {
                receipts::replay(METHOD, *receipt)
            }
            Err(error) => Err(error.into()),
        },
        None => match ensure_exists(state, conversation_id).await {
            Ok(()) => Err(DaemonError::ApprovalNotPending { call_id }),
            Err(error) => Err(error),
        },
    };
    match outcome {
        Ok(answer) => {
            responder.item(&answer).await?;
            Ok(())
        }
        Err(error @ DaemonError::Rejected { .. }) => Err(error),
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}
