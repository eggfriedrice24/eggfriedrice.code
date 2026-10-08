//! `turn.interrupt`: stop the running turn, in two phases.
//!
//! The request is recorded at once (`turn_interrupt_requested`); the turn records
//! `turn_interrupted` when the model's stream and any running command have stopped.
//! Esc in the input row of a turn also lists the prompts that its view queued
//! (`withdraw`) and its unread steers (`resend_steers`): the conversation withdraws
//! those prompts and sends those steers again as one prompt that runs next, in the
//! step and the append that record the request, so no queued prompt starts in between.

use efr_conversation::ConversationError;
use efr_protocol::TurnInterrupt;
use efr_transport::{ConnectionContext, Responder};

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "turn.interrupt";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: TurnInterrupt,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    let conversation_id = params.conversation_id;
    // NOTE: unread steers sent again become a prompt that this client follows, so the
    // notices wait for it, as for `prompt.send`.
    let mut prompting =
        (!params.resend_steers.is_empty()).then(|| state.connections.prompting(context.conn_id()));
    let outcome = match state.conversations.live(conversation_id) {
        Some(handle) => match handle.interrupt(params, context.surface()).await {
            Ok(result) => {
                if let (Some(_), Some(prompting)) = (&result.resent, prompting.as_mut()) {
                    prompting.sent_to(conversation_id);
                }
                Ok(serde_json::to_value(result)
                    .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?)
            }
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
