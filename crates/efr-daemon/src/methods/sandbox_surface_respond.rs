//! `sandbox.surface_respond`: the user's answer to the quarantine question, whether to
//! keep git changes that a contained call made and the launcher moved aside (efr's
//! auto spec, section 5.6).
//!
//! The method needs `approve`, which a model-side peer never holds. A phone may answer
//! Leave; the conversation refuses a Keep from a phone. A retried command answers from
//! its receipt, as `approval.respond` does.

use efr_conversation::{ConversationError, ConversationHandle};
use efr_protocol::{CommandId, ConversationId, QuestionId, SandboxSurfaceRespond};
use efr_transport::{ConnectionContext, Responder};
use serde::Serialize;

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "sandbox.surface_respond";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: SandboxSurfaceRespond,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let parked = Parked {
        method: METHOD,
        command_id: params.command_id,
        conversation_id: params.conversation_id,
        question_id: params.question_id,
    };
    let origin = context.surface();
    respond_parked(state, parked, responder, |handle| async move {
        handle.respond_surface(params, origin).await
    })
    .await
}

/// The answer to a question that a turn parked: the method, the command, and where
/// the question is.
pub(super) struct Parked {
    pub(super) method: &'static str,
    pub(super) command_id: CommandId,
    pub(super) conversation_id: ConversationId,
    pub(super) question_id: QuestionId,
}

/// The flow of an answer to a question that a turn parked (`sandbox.surface_respond`,
/// `question.respond`): a retried command answers from its receipt; the live
/// conversation takes the answer through `respond`; a conversation that is not live
/// has no question that waits. A refusal goes into the receipt of the command.
pub(super) async fn respond_parked<R, F>(
    state: &State,
    parked: Parked,
    responder: &Responder,
    respond: impl FnOnce(ConversationHandle) -> F,
) -> Result<(), DaemonError>
where
    R: Serialize,
    F: Future<Output = Result<R, ConversationError>>,
{
    let Parked { method, command_id, conversation_id, question_id } = parked;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(method, receipt)?).await?;
        return Ok(());
    }
    let outcome = match state.conversations.live(conversation_id) {
        Some(handle) => match respond(handle).await {
            Ok(result) => Ok(serde_json::to_value(result)
                .map_err(|source| DaemonError::EncodeResult { method, source })?),
            Err(ConversationError::DuplicateCommand { receipt }) => {
                receipts::replay(method, *receipt)
            }
            Err(error) => Err(error.into()),
        },
        None => match ensure_exists(state, conversation_id).await {
            Ok(()) => Err(DaemonError::Conversation {
                source: ConversationError::QuestionNotPending { question_id },
            }),
            Err(error) => Err(error),
        },
    };
    match outcome {
        Ok(answer) => {
            responder.item(&answer).await?;
            Ok(())
        }
        Err(error @ DaemonError::Rejected { .. }) => Err(error),
        Err(error) => Err(receipts::refuse(state, command_id, method, error).await),
    }
}
