//! `prompt.send`: a prompt for a conversation.
//!
//! Routing: the named conversation; with `new_conversation`, a new one that becomes
//! the terminal's active conversation (`,new`); otherwise the active conversation of
//! the prompt's terminal (the context's tty, or the hello's), and a new one when the
//! terminal has none. A retried command id answers from its receipt and starts
//! nothing; a refusal that a retry cannot change is kept as a rejected receipt.
//!
//! A terminal's conversation ends for routing ([`continues`]) when it has been idle
//! for `conversation.tty_idle_hours`, or when the prompt comes from another shell
//! while the shell that took the terminal has exited: Linux hands out `/dev/pts`
//! numbers lowest first, so a new tab usually gets the number of a closed one, and
//! must not continue its days-old conversation. Another shell while the first still
//! runs (a nested `zsh`, a `sudo -s`) continues it.
//!
//! While the model catalog has no list yet (the Anthropic catalog before its first
//! fetch), the prompt first waits for one fetch (`catalog.rs`, `Models::ready`), so its
//! turn knows the limits of its model.
//!
//! The connection counts as one that may still show the conversation from the moment
//! the request arrives until it closes (`connections.rs`): `efr` subscribes to the turn
//! right after the answer, and a turn that ended before that must not leave a notice
//! for the terminal that is about to show it.

use std::path::Path;
use std::time::Duration;

use efr_conversation::ConversationError;
use efr_protocol::{ConversationId, Mode, PromptSend, TurnSettings};
use efr_stdx::id::uuid_v7;
use efr_transport::{ConnectionContext, Responder};
use jiff::Timestamp;
use serde_json::Value;

use crate::conversations::ActiveTty;
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
    let tty = prompt_tty(params, hello_tty);
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

/// The terminal a prompt comes from: its context's tty, else the hello's.
pub(crate) fn prompt_tty(params: &PromptSend, hello_tty: Option<String>) -> Option<String> {
    params.context.as_ref().and_then(|context| context.tty.clone()).or(hello_tty)
}

/// Whether `active`, the active conversation of the prompt's terminal, takes a prompt
/// from the shell `shell_pid` at `now`. It does not when it was last active (`updated_at`)
/// longer than `idle_limit` ago, or when the shell that took the terminal is another
/// one and `alive` says it has exited.
pub(crate) fn continues(
    active: ActiveTty,
    shell_pid: Option<u32>,
    updated_at: Option<Timestamp>,
    now: Timestamp,
    idle_limit: Option<Duration>,
    alive: impl FnOnce(u32) -> bool,
) -> bool {
    if let (Some(limit), Some(updated_at)) = (idle_limit, updated_at)
        && Duration::try_from(now.duration_since(updated_at)).is_ok_and(|idle| idle > limit)
    {
        return false;
    }
    match (active.shell_pid, shell_pid) {
        (Some(owner), Some(asking)) if owner != asking => alive(owner),
        _ => true,
    }
}

/// True while the process `pid` exists.
fn process_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// The active conversation of `tty` if it continues for a prompt from `shell_pid`.
async fn routed(
    state: &State,
    tty: &str,
    shell_pid: Option<u32>,
) -> Result<Option<ConversationId>, DaemonError> {
    let Some(active) = state.conversations.active(tty) else {
        return Ok(None);
    };
    let conversation_id = active.conversation_id;
    let updated_at = state
        .readers
        .with(move |conn| efr_store::conversations::get(conn, conversation_id))
        .await?
        .map(|summary| summary.updated_at);
    let hours = state.settings.borrow().conversation.tty_idle_hours;
    let idle_limit = (hours > 0).then(|| Duration::from_secs(hours.saturating_mul(3600)));
    let now = state.clock.now();
    if continues(active, shell_pid, updated_at, now, idle_limit, process_alive) {
        Ok(Some(conversation_id))
    } else {
        tracing::info!(tty, conversation_id = %conversation_id, "the terminal's conversation ended; a new one starts");
        Ok(None)
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
    reprobe_for_auto(state, &params.settings).await;
    state.providers.models().ready().await;
    // NOTE: counted before the prompt is recorded, because its turn may end before this
    // answers, and the notices must wait for the client that follows it.
    let mut prompting = state.connections.prompting(context.conn_id());
    let hello_tty = state.connections.tty(context.conn_id());
    let shell_pid = params.context.as_ref().and_then(|context| context.shell_pid);
    let tty = prompt_tty(&params, hello_tty.clone());
    let active = match (&tty, params.conversation_id, params.new_conversation) {
        (Some(tty), None, false) => routed(state, tty, shell_pid).await?,
        _ => None,
    };
    let sent = match target(&params, hello_tty, |_| active)? {
        Target::Existing(conversation_id) => {
            ensure_exists(state, conversation_id).await?;
            let sent = state.conversations.open(conversation_id).send_prompt(params, origin).await;
            if let (Ok(_), Some(tty), Some(pid)) = (&sent, &tty, shell_pid)
                && active == Some(conversation_id)
            {
                state.conversations.adopt(tty, pid);
            }
            sent
        }
        Target::New { tty } => {
            let conversation_id = ConversationId::from_uuid(uuid_v7(&*state.clock, &*state.rng));
            let handle = state.conversations.create(conversation_id, origin, tty.clone());
            let sent = handle.send_prompt(params, origin).await;
            match (&sent, tty) {
                (Ok(_), Some(tty)) => {
                    state.conversations.activate(&tty, conversation_id, shell_pid);
                }
                (Ok(_), None) => {}
                (Err(_), _) => state.conversations.forget(conversation_id).await,
            }
            sent
        }
    };
    match sent {
        Ok(result) => {
            prompting.sent_to(result.conversation_id);
            serde_json::to_value(result)
                .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })
        }
        Err(ConversationError::DuplicateCommand { receipt }) => receipts::replay(METHOD, *receipt),
        Err(error) => Err(error.into()),
    }
}

/// Runs the sandbox probe again before a prompt whose `asked` settings are `auto`, or
/// whose default mode is `auto`, while the last probe said the sandbox is unavailable,
/// so a user who fixed the cause needs no restart (efr's auto spec, section 12.1).
pub(crate) async fn reprobe_for_auto(state: &State, asked: &TurnSettings) {
    let settings = std::sync::Arc::clone(&state.settings.borrow());
    let mode = asked.mode.unwrap_or(settings.permissions.mode);
    if mode == Mode::Auto && !state.sandbox.current().available {
        state.sandbox.probe(&settings).await;
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
