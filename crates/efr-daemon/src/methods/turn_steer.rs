//! `turn.steer`: guidance for the running turn (`,!`), read by the model before its next
//! step. Unlike a prompt it never queues: without a running turn it is a conflict.

use efr_conversation::ConversationError;
use efr_protocol::TurnSteer;
use efr_transport::Responder;

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "turn.steer";

pub(crate) async fn handle(
    state: &State,
    params: TurnSteer,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    if params.text.trim().is_empty() {
        let error = DaemonError::InvalidParams { reason: "the guidance is empty" };
        return Err(receipts::refuse(state, command_id, METHOD, error).await);
    }
    let conversation_id = params.conversation_id;
    let outcome = match state.conversations.live(conversation_id) {
        Some(handle) => match handle.steer(params).await {
            Ok(result) => Ok(serde_json::to_value(result)
                .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?),
            Err(ConversationError::DuplicateCommand { receipt }) => {
                receipts::replay(METHOD, *receipt)
            }
            Err(error) => Err(error.into()),
        },
        None => match ensure_exists(state, conversation_id).await {
            Ok(()) => Err(DaemonError::NoRunningTurn { conversation_id }),
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
