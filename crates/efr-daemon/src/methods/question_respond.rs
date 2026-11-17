//! `question.respond`: a person's answer to the model's question (the tool
//! `ask_user`): an option, a typed text or a skip.
//!
//! The method needs `operate`, which a phone holds and a model-side peer never does: an
//! answer has less power than a steer, and it approves nothing. A retried command
//! answers from its receipt, as `sandbox.surface_respond` does; the conversation checks
//! the answer against the question before it takes it, so a bad answer leaves the
//! question open.

use efr_conversation::ConversationError;
use efr_protocol::QuestionRespond;
use efr_transport::{ConnectionContext, Responder};

use crate::methods::prompt_send::ensure_exists;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "question.respond";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: QuestionRespond,
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
        Some(handle) => match handle.respond_question(params, context.surface()).await {
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
