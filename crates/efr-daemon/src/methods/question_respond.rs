//! `question.respond`: a person's answer to the model's question (the tool
//! `ask_user`): an option, a typed text or a skip.
//!
//! The method needs `operate`, which a phone holds and a model-side peer never does: an
//! answer has less power than a steer, and it approves nothing. A retried command
//! answers from its receipt, as `sandbox.surface_respond` does, through the same flow
//! ([`respond_parked`]); the conversation checks the answer against the question before
//! it takes it, so a bad answer leaves the question open.

use efr_protocol::QuestionRespond;
use efr_transport::{ConnectionContext, Responder};

use super::sandbox_surface_respond::{Parked, respond_parked};
use crate::DaemonError;
use crate::state::State;

const METHOD: &str = "question.respond";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: QuestionRespond,
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
        handle.respond_question(params, origin).await
    })
    .await
}
