//! `prompt.send`: a prompt for a conversation.
//!
//! Routing: the named conversation; with `new_conversation`, a new one that becomes
//! the terminal's active conversation (`,new`); otherwise the active conversation of
//! the prompt's terminal (the context's tty, or the hello's), and a new one when the
//! terminal has none. A retried command id answers from its receipt and starts
//! nothing; a refusal that a retry cannot change is kept as a rejected receipt.

use efr_conversation::ConversationError;
use efr_protocol::{ConversationId, PromptSend};
use efr_stdx::id::uuid_v7;
use efr_transport::{ConnectionContext, Responder};
use serde_json::Value;

use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "prompt.send";

/// Where a prompt goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A conversation that exists.
    Existing(ConversationId),
    /// A new conversation, active in `tty` when there is one.
    New { tty: Option<String> },
}

/// Where `params` goes, from a connection whose hello named `hello_tty`, when `active`
/// gives each terminal's active conversation.
pub(crate) fn target(
    params: &PromptSend,
    hello_tty: Option<String>,
    active: impl FnOnce(&str) -> Option<ConversationId>,
) -> Result<Target, DaemonError> {
    if params.text.trim().is_empty() {
        return Err(DaemonError::InvalidParams { reason: "the prompt is empty" });
    }
    let tty = params.context.as_ref().and_then(|context| context.tty.clone()).or(hello_tty);
    match (params.conversation_id, params.new_conversation) {
        (Some(_), true) => Err(DaemonError::InvalidParams {
            reason: "new_conversation and conversation_id exclude each other",
        }),
        (Some(conversation_id), false) => Ok(Target::Existing(conversation_id)),
        (None, true) => Ok(Target::New { tty }),
        (None, false) => match tty.as_deref().and_then(active) {
            Some(conversation_id) => Ok(Target::Existing(conversation_id)),
            None => Ok(Target::New { tty }),
        },
    }
}

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: PromptSend,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    let answer = match receipts::lookup(state, command_id).await? {
        Some(receipt) => receipts::replay(METHOD, receipt)?,
        None => match send(state, context, params).await {
            Ok(answer) => answer,
            Err(error @ DaemonError::Rejected { .. }) => return Err(error),
            Err(error) => return Err(receipts::refuse(state, command_id, METHOD, error).await),
        },
    };
    responder.item(&answer).await?;
    Ok(())
}

async fn send(
    state: &State,
    context: &ConnectionContext,
    params: PromptSend,
) -> Result<Value, DaemonError> {
    let origin = context.surface();
    let hello_tty = state.connections.tty(context.conn_id());
    let sent = match target(&params, hello_tty, |tty| state.conversations.active(tty))? {
        Target::Existing(conversation_id) => {
            ensure_exists(state, conversation_id).await?;
            state.conversations.open(conversation_id).send_prompt(params, origin).await
        }
        Target::New { tty } => {
            let conversation_id = ConversationId::from_uuid(uuid_v7(&*state.clock, &*state.rng));
            let handle = state.conversations.create(conversation_id, origin, tty.clone());
            let sent = handle.send_prompt(params, origin).await;
            match (&sent, tty) {
                (Ok(_), Some(tty)) => state.conversations.activate(&tty, conversation_id),
                (Ok(_), None) => {}
                (Err(_), _) => state.conversations.forget(conversation_id).await,
            }
            sent
        }
    };
    match sent {
        Ok(result) => serde_json::to_value(result)
            .map_err(|source| DaemonError::EncodeResult { method: METHOD, source }),
        Err(ConversationError::DuplicateCommand { receipt }) => receipts::replay(METHOD, *receipt),
        Err(error) => Err(error.into()),
    }
}

/// Refuses a conversation that the log does not hold.
pub(crate) async fn ensure_exists(
    state: &State,
    conversation_id: ConversationId,
) -> Result<(), DaemonError> {
    let exists = state
        .readers
        .with(move |conn| efr_store::conversations::get(conn, conversation_id))
        .await?
        .is_some();
    if exists { Ok(()) } else { Err(DaemonError::ConversationNotFound { conversation_id }) }
}

#[cfg(test)]
mod tests;
