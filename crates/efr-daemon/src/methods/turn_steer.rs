//! `turn.steer`: guidance for the running turn (`,!`, Enter in the input row of a
//! turn), read by the model before its next step.
//!
//! A late steer, one that no model call of the turn would read, is never recorded as
//! `turn_steered`. Without `if_late` it is a conflict, as before. With
//! `if_late: {kind: "queue"}` the conversation records it as a queued prompt in the
//! same step, and the result says `queued`; a conversation without a live actor gets
//! one for that, because a late steer then has no turn at all.

use efr_conversation::ConversationError;
use efr_protocol::{LateSteer, TurnSteer};
use efr_transport::{ConnectionContext, Responder};

use crate::methods::prompt_send::{ensure_exists, reprobe_for_auto};
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "turn.steer";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
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
    match steer(state, context, params).await {
        Ok(answer) => {
            responder.item(&answer).await?;
            Ok(())
        }
        Err(error @ DaemonError::Rejected { .. }) => Err(error),
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}

async fn steer(
    state: &State,
    context: &ConnectionContext,
    params: TurnSteer,
) -> Result<serde_json::Value, DaemonError> {
    let conversation_id = params.conversation_id;
    let may_queue = matches!(params.if_late, Some(LateSteer::Queue { .. }));
    let handle = match state.conversations.live(conversation_id) {
        Some(handle) => handle,
        None => {
            ensure_exists(state, conversation_id).await?;
            if !may_queue {
                return Err(DaemonError::NoRunningTurn { conversation_id });
            }
            state.conversations.open(conversation_id)
        }
    };
    if let Some(LateSteer::Queue { settings, .. }) = &params.if_late {
        reprobe_for_auto(state, settings).await;
    }
    // NOTE: counted before the steer is sent, as for `prompt.send`: a late steer that
    // becomes a prompt may end before its client follows it, and the notices must wait
    // for that client.
    let mut prompting = may_queue.then(|| state.connections.prompting(context.conn_id()));
    match handle.steer(params, context.surface()).await {
        Ok(result) => {
            if let (true, Some(prompting)) = (result.queued, prompting.as_mut()) {
                prompting.sent_to(conversation_id);
            }
            serde_json::to_value(result)
                .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })
        }
        Err(ConversationError::DuplicateCommand { receipt }) => receipts::replay(METHOD, *receipt),
        Err(error) => Err(error.into()),
    }
}
