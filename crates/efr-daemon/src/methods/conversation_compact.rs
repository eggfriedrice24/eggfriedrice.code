//! `conversation.compact`: compact a conversation's context now (`efr compact`,
//! `,compact`).
//!
//! NOTE: a stub. The conversation cannot compact its context yet, so the daemon refuses
//! every request with `invalid` and records nothing. The contract is in the README of
//! efr-conversation, section "Context".

use efr_protocol::ConversationCompact;

use crate::DaemonError;

pub(crate) fn handle(params: &ConversationCompact) -> Result<(), DaemonError> {
    tracing::debug!(conversation_id = %params.conversation_id, "conversation.compact is a stub");
    Err(DaemonError::InvalidParams { reason: "this daemon cannot compact a conversation yet" })
}
