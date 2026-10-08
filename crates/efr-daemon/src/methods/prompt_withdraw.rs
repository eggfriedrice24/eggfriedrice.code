//! `prompt.withdraw`: take back a prompt that waits in the queue (Alt+Up in the input
//! row of a turn).
//!
//! NOTE: a stub. The conversation actor cannot withdraw a prompt yet, so the daemon
//! refuses every request with `invalid` and records nothing. The contract is in the
//! README of efr-protocol.

use efr_protocol::PromptWithdraw;

use crate::DaemonError;

pub(crate) fn handle(params: &PromptWithdraw) -> Result<(), DaemonError> {
    tracing::debug!(conversation_id = %params.conversation_id, "prompt.withdraw is a stub");
    Err(DaemonError::InvalidParams { reason: "this daemon cannot withdraw prompts yet" })
}
