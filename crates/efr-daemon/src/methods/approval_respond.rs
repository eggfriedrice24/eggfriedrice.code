//! `approval.respond`: the user's answer to a tool call that waits for approval.
//!
//! The store refuses an answer to a call that is not pending, so of two racing answers
//! only one commits, and an approval that expired at a restart answers `not_found`. The
//! conversation refuses an answer from a remote surface, such as a phone, to a privilege
//! exit with `forbidden`, and the question stays open for the user's own terminal.
//!
//! An `allow_always` answer saves a standing rule (phase E), so only the user's own
//! terminal gives it: efrd refuses it from a phone with `forbidden` before the
//! conversation sees it, and the conversation checks it again. An `allow_always` to a
//! question that offers no rule is `invalid`. Both leave the question open.

use efr_conversation::ConversationError;
use efr_protocol::{ApprovalDecision, ApprovalRespond, CallId, ErrorBody, ErrorCode, Mode, Origin};
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
    // NOTE: no receipt keeps this refusal either, so the user's own terminal can still
    // answer the question.
    if let Some(refused) = remote_always(params.decision, context.surface(), call_id) {
        return Err(refused);
    }
    let outcome = match state.conversations.live(conversation_id) {
        Some(handle) => match handle.respond_approval(params, context.surface()).await {
            Ok(result) => Ok(serde_json::to_value(result)
                .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?),
            Err(ConversationError::DuplicateCommand { receipt }) => {
                receipts::replay(METHOD, *receipt)
            }
            // NOTE: the question stays open for the user's own terminal, so no receipt
            // keeps this refusal.
            Err(
                error @ (ConversationError::RemotePrivilegeAnswer { .. }
                | ConversationError::RemoteAlwaysAnswer { .. }),
            ) => Err(DaemonError::Rejected {
                body: ErrorBody::new(ErrorCode::Forbidden, error.to_string()),
            }),
            Err(error @ ConversationError::NoAlwaysOffer { .. }) => Err(DaemonError::Rejected {
                body: ErrorBody::new(ErrorCode::Invalid, error.to_string()),
            }),
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

/// The refusal of `decision` from `surface` for `call_id` when it is an `allow_always`
/// from a surface that is not the user's own machine, such as a phone; `None` for any
/// other answer.
pub(crate) fn remote_always(
    decision: ApprovalDecision,
    surface: Origin,
    call_id: CallId,
) -> Option<DaemonError> {
    let remote = efr_permissions::effective_mode(Mode::Auto, surface) != Mode::Auto;
    (decision == ApprovalDecision::AllowAlways && remote).then(|| {
        let error = ConversationError::RemoteAlwaysAnswer { origin: surface, call_id };
        DaemonError::Rejected { body: ErrorBody::new(ErrorCode::Forbidden, error.to_string()) }
    })
}
