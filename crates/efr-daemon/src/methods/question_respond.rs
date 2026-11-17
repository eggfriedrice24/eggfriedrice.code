//! `question.respond`: a person's answer to the model's question (the tool
//! `ask_user`).
//!
//! NOTE: a stub. No turn asks a question yet, so each answer gets `not_found`, and
//! `hello` does not announce `user_questions`. The handler of the question takes its
//! place, on the model of `sandbox_surface_respond.rs`: the receipt first, then the
//! actor.

use efr_conversation::ConversationError;
use efr_protocol::QuestionRespond;

use crate::DaemonError;
use crate::methods::prompt_send::ensure_exists;
use crate::state::State;

pub(crate) async fn handle(state: &State, params: QuestionRespond) -> Result<(), DaemonError> {
    ensure_exists(state, params.conversation_id).await?;
    Err(DaemonError::Conversation {
        source: ConversationError::QuestionNotPending { question_id: params.question_id },
    })
}
