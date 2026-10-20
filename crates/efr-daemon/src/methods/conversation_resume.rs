//! `conversation.resume`: make a conversation this terminal's conversation again
//! (`,resume N`, `efr resume N`; DV5).
//!
//! Only the shell's or `efr`'s connection on the terminal that the context names may
//! move a conversation there (S-12, as for a pick-up): a phone or a made-up tty gets
//! `forbidden`. When another open terminal holds the conversation (a slot whose shell
//! runs or is not known, or a terminal that follows it), efrd moves nothing and answers
//! `held`, unless the params say `take`.
//!
//! The conversation goes into the terminal's slot of its own scope key (its `route`
//! column), not of the scope of the user's directory (OQV20): a project conversation
//! keeps its project. efrd claims the slot first, as a pick-up does, then records
//! `terminal_attached` (`how: resumed`) with the receipt in one batch, and only then
//! completes the claim: every other slot that held the conversation lets it go, and
//! that terminal's next prompt says where it went. The new event is the terminal's
//! newest routed one, so the slot is its current one: `efr history`, `undo` and `jobs`
//! act on the conversation at once, and its job notices go to this terminal. Resuming
//! the conversation that the slot holds already only makes the slot current.
//!
//! When the conversation has no hidden shell that runs, efrd starts one at once
//! (`ShellSessions::open`, as a job's environment does) in its last directory: the
//! directory of its newest shell (its `shell_started` or newest `cwd_changed`, from
//! the shells projection) when it still exists, else the working directory of its last
//! prompt, else the user's. The next turn moves the shell to the user's directory only
//! when that differs from the last turn's, as always.
//!
//! A retried command id answers from its receipt. A `held` answer keeps no receipt:
//! the client asks the user and sends a new command with `take`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    AttachHow, ConversationId, ConversationResume, ConversationResumeResult, Event, RouteScope,
};
use efr_store::receipts::NewReceipt;
use efr_store::{Batch, StoreError};
use efr_transport::{ConnectionContext, Responder};
use serde_json::Value;

use crate::conversations::{self, Slot};
use crate::methods::prompt_send::read_place;
use crate::routing;
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "conversation.resume";

pub(crate) async fn handle(
    state: &State,
    context: &ConnectionContext,
    params: ConversationResume,
    responder: &Responder,
) -> Result<(), DaemonError> {
    let command_id = params.command_id;
    let answer = match receipts::lookup(state, command_id).await? {
        Some(receipt) => receipts::replay(METHOD, receipt)?,
        None => match resume(state, context, params).await {
            Ok(answer) => answer,
            Err(error @ DaemonError::Rejected { .. }) => return Err(error),
            Err(error) => return Err(receipts::refuse(state, command_id, METHOD, error).await),
        },
    };
    responder.item(&answer).await?;
    Ok(())
}

async fn resume(
    state: &State,
    context: &ConnectionContext,
    params: ConversationResume,
) -> Result<Value, DaemonError> {
    let ConversationResume { command_id, conversation_id, context: shell, take } = params;
    let Some(tty) = shell.tty.clone() else {
        return Err(DaemonError::InvalidParams {
            reason: "a resume needs the terminal's tty in its context",
        });
    };
    if !conversations::peer_on_tty(state, context, &tty) {
        return Err(DaemonError::ResumeOffTerminal { tty });
    }
    let summary = state
        .readers
        .with(move |conn| efr_store::conversations::get(conn, conversation_id))
        .await?
        .ok_or(DaemonError::ConversationNotFound { conversation_id })?;
    let route = state
        .readers
        .with(move |conn| efr_store::conversations::route(conn, conversation_id))
        .await?
        .unwrap_or(RouteScope::Outside);
    if !take && let Some(held) = conversations::held(state, conversation_id, Some(&tty)) {
        return held_answer(held.tty(), summary.title);
    }
    let settings = Arc::clone(&state.settings.borrow());
    let (registry, resolved) = read_place(state, Some(shell.pwd.clone())).await;
    let pwd_key = routing::route_key(&registry, &shell.pwd, resolved.as_deref());
    let continues_here = !settings.conversation.follow_project || pwd_key == route;
    let dir = summary.cwd.clone().unwrap_or_else(|| shell.pwd.clone());
    let scope = routing::label(&registry, &route, &dir);
    let before = state.conversations.slot(&tty, &route);
    let here = before.is_some_and(|slot| slot.conversation_id == conversation_id);
    let expected = before.map(|slot| slot.conversation_id);
    let alive = |pid: u32| state.procs.alive(pid);
    let claimed = if take {
        state.conversations.take(&tty, &route, conversation_id, shell.shell_pid, expected)
    } else {
        state.conversations.claim(&tty, &route, conversation_id, shell.shell_pid, expected, &alive)
    };
    if !claimed {
        return match conversations::held(state, conversation_id, Some(&tty)) {
            Some(held) if !take => held_answer(held.tty(), summary.title),
            _ => Err(DaemonError::ResumeRaced),
        };
    }
    let shell_dir = start_shell(state, conversation_id, summary.cwd.as_deref(), &shell.pwd).await;
    let result = ConversationResumeResult::Resumed {
        conversation_id,
        scope,
        title: summary.title.clone(),
        updated_at: summary.updated_at,
        shell_dir,
        here,
        continues_here,
    };
    let value = serde_json::to_value(&result)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })?;
    let attached = Event::TerminalAttached {
        tty: tty.clone(),
        route: route.clone(),
        shell_pid: shell.shell_pid,
        how: AttachHow::Resumed,
    };
    let batch = Batch::new().event(conversation_id, attached).receipt(NewReceipt::accepted(
        command_id,
        METHOD,
        value.clone(),
    ));
    match state.writer.append(batch).await {
        Ok(committed) => {
            state.conversations.attached(
                &tty,
                &route,
                conversation_id,
                committed.last_seq(),
                summary.title,
            );
            tracing::info!(tty, conversation_id = %conversation_id, take, "a terminal resumed a conversation");
            Ok(value)
        }
        Err(StoreError::DuplicateCommand { receipt }) => {
            release(state, &tty, &route, before);
            receipts::replay(METHOD, *receipt)
        }
        Err(source) => {
            // NOTE: nothing was recorded, so the slot gets back what it held (S-18).
            release(state, &tty, &route, before);
            Err(DaemonError::Store { source })
        }
    }
}

/// The answer when another open terminal holds the conversation: nothing moved.
fn held_answer(tty: &str, title: Option<String>) -> Result<Value, DaemonError> {
    let held = ConversationResumeResult::Held { tty: tty.to_owned(), title };
    serde_json::to_value(held)
        .map_err(|source| DaemonError::EncodeResult { method: METHOD, source })
}

/// Gives the slot back what it held before a claim whose event was not recorded.
fn release(state: &State, tty: &str, route: &RouteScope, before: Option<Slot>) {
    state.conversations.release_claim(tty, route, before);
}

/// Starts the conversation's hidden shell when none runs, in its last directory that
/// still exists: the newest shell's, else `prompt_dir` (the last prompt's), else
/// `pwd`. The directory of the shell, or `None` when it could not start.
async fn start_shell(
    state: &State,
    conversation_id: ConversationId,
    prompt_dir: Option<&Path>,
    pwd: &Path,
) -> Option<PathBuf> {
    let shells = state
        .readers
        .with(move |conn| efr_store::shells::for_conversation(conn, conversation_id))
        .await;
    let last = match shells {
        Ok(shells) => shells.into_iter().next_back().map(|shell| shell.cwd),
        Err(error) => {
            tracing::warn!(%conversation_id, %error, "the last shell directory could not be read");
            None
        }
    };
    let candidates: Vec<PathBuf> = last
        .into_iter()
        .chain(prompt_dir.map(Path::to_path_buf))
        .chain([pwd.to_path_buf()])
        .collect();
    let dir = tokio::task::spawn_blocking(move || candidates.into_iter().find(|dir| dir.is_dir()))
        .await
        .ok()
        .flatten()?;
    match state.shells.open(conversation_id, &dir).await {
        Ok(info) if info.spawned => Some(dir),
        Ok(_) => state.shells.state(conversation_id).await.ok().map(|shell| shell.cwd),
        Err(error) => {
            tracing::info!(%conversation_id, %error, "the hidden shell of a resumed conversation did not start");
            None
        }
    }
}
