//! `sandbox.surface_respond`: the user's answer to the quarantine question, whether to
//! keep git changes that a contained call made and the launcher moved aside (efr's
//! auto spec, section 5.6).
//!
//! The method needs `approve`, which a model-side peer never holds, and the
//! conversation refuses an answer from a phone. A retried command answers from its
//! receipt, as `approval.respond` does.

use efr_conversation::ConversationError;
use efr_protocol::SandboxSurfaceRespond;
use efr_transport::{ConnectionContext, Responder};

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
    let command_id = params.command_id;
    if let Some(receipt) = receipts::lookup(state, command_id).await? {
        responder.item(&receipts::replay(METHOD, receipt)?).await?;
        return Ok(());
    }
    let conversation_id = params.conversation_id;
    let question_id = params.question_id;
    let outcome = match state.conversations.live(conversation_id) {
        Some(handle) => match handle.respond_surface(params, context.surface()).await {
            Ok(result) => Ok(serde_json::to_value(result)
                .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?),
            Err(ConversationError::DuplicateCommand { receipt }) => {
                receipts::replay(METHOD, *receipt)
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
        Err(error) => Err(receipts::refuse(state, command_id, METHOD, error).await),
    }
}
