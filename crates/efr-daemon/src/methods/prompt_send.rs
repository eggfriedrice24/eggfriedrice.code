//! `prompt.send`: a prompt for a conversation.
//!
//! Routing (`routing.rs` holds the rules, `conversations.rs` the slots): the named
//! conversation; else, for a prompt from a terminal (the context's tty, or the
//! hello's), the conversation of the terminal's slot for the prompt's scope key: the
//! registered project whose root holds the prompt's working directory, or `outside`.
//! So the conversation follows the scope when a prompt is sent, not on `cd`, and a
//! prompt about two projects stays in the project where the user is. With
//! `new_conversation` (`,new`), a new one in that slot; the other slots stay. A prompt
//! with no terminal starts a conversation of its own, as before.
//!
//! The slot of the key continues ([`routing::continues`]) unless the prompt comes from
//! another shell while the slot's shell has exited (checked first: Linux hands out
//! `/dev/pts` numbers lowest first, so a new tab usually gets the number of a closed
//! one), or its conversation was idle for `conversation.tty_idle_hours`. Then, for a
//! project key only, the prompt picks up the project's single newest conversation of
//! those that a terminal held (never one of a script or the phone, DV55) when it is
//! idle, was active within those hours and no open terminal holds it, and only when
//! the connection is the shell's or `efr`'s and its process runs on the claimed
//! terminal (S-12). The pick-up records `terminal_attached` in the prompt's batch. Else
//! a new conversation starts. With `conversation.follow_project = false` the prompt
//! goes to the terminal's current slot whatever its key, with no pick-up. The result
//! names the scope and the switch, for the line before the reply. A prompt outside
//! projects in a git work tree also names that work tree, at most once a day for each
//! (`repo_hint.rs`, `conversation.repo_hint`).
//!
//! A retried command id answers from its receipt and starts nothing; a refusal that a
//! retry cannot change is kept as a rejected receipt.
//!
//! While the model catalog has no list yet (the Anthropic catalog before its first
//! fetch), the prompt first waits for one fetch (`catalog.rs`, `Models::ready`), so its
//! turn knows the limits of its model. When the fetch gives no list either, the prompt
//! fails with the cause (`Providers::check_list`): no key is stored, so it says how to
//! log in, or the fetch failed, so it says why.
//!
//! A prompt that names no model of its own fails before anything waits when
//! `[model] name` is a model of another company than `[model] provider`
//! (`efr_config::ForeignModel`): the error names the cause and the fix.
//!
//! The connection counts as one that may still show the conversation from the moment
//! the request arrives until it closes (`connections.rs`): `efr` subscribes to the turn
//! right after the answer, and a turn that ended before that must not leave a notice
//! for the terminal that is about to show it.

use std::path::PathBuf;
use std::sync::Arc;

use efr_conversation::{ConversationError, PromptRouting};
use efr_protocol::{
    AttachHow, ConversationId, ConversationSummary, Event, Mode, PromptSend, PromptSendResult,
    RepoHint, RouteLabel, RouteScope, Routed, Seq, TurnSettings,
};
use efr_scope::{Basis, Registry};
use efr_stdx::id::uuid_v7;
use efr_transport::{ConnectionContext, Responder};
use serde_json::Value;

use crate::conversations::{self, Slot};
use crate::routing::{self, Before, Candidate, Outcome, Verdict};
use crate::state::State;
use crate::{DaemonError, receipts};

const METHOD: &str = "prompt.send";

/// Refuses an empty prompt, and one that names a conversation and asks for a new one.
pub(crate) fn validate(params: &PromptSend) -> Result<(), DaemonError> {
    if params.text.trim().is_empty() {
        return Err(DaemonError::InvalidParams { reason: "the prompt is empty" });
    }
    if params.conversation_id.is_some() && params.new_conversation {
        return Err(DaemonError::InvalidParams {
            reason: "new_conversation and conversation_id exclude each other",
        });
    }
    Ok(())
}

/// The terminal a prompt comes from: its context's tty, else the hello's.
pub(crate) fn prompt_tty(params: &PromptSend, hello_tty: Option<String>) -> Option<String> {
    params.context.as_ref().and_then(|context| context.tty.clone()).or(hello_tty)
}

/// Where a prompt from a terminal goes.
#[derive(Debug)]
struct Routing {
    tty: String,
    /// The key of the slot that the prompt uses.
    key: RouteScope,
    /// The scope of the prompt, for its head line; `None` without a shell context.
    label: Option<RouteLabel>,
    /// The conversation; `None` starts a new one in the slot.
    target: Option<ConversationId>,
    /// A pick-up that claimed the slot, with what the slot held before.
    claim: Option<Option<Slot>>,
    /// The title of the conversation that the prompt continues or picks up.
    title: Option<String>,
    switch: Option<efr_protocol::Switch>,
    /// The git work tree outside projects that the prompt's line names (DV53).
    hint: Option<RepoHint>,
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
    refuse_foreign_model(state, &params.settings)?;
    reprobe_for_auto(state, &params.settings).await;
    state.providers.models().ready().await;
    let settings = Arc::clone(&state.settings.borrow());
    state.providers.check_list(&settings, params.settings.model.as_deref())?;
    validate(&params)?;
    // NOTE: counted before the prompt is recorded, because its turn may end before this
    // answers, and the notices must wait for the client that follows it.
    let mut prompting = state.connections.prompting(context.conn_id());
    let hello_tty = state.connections.tty(context.conn_id());
    let shell_pid = params.context.as_ref().and_then(|context| context.shell_pid);
    // NOTE: only a prompt whose own context names the terminal moves its slot, as the
    // projection's rule for `prompt_queued` does.
    let context_tty = params.context.as_ref().and_then(|context| context.tty.clone());
    let routing = match (prompt_tty(&params, hello_tty), params.conversation_id) {
        (Some(tty), None) => Some(route(state, context, &params, tty, shell_pid).await?),
        _ => None,
    };
    let sent = match (routing, params.conversation_id) {
        (_, Some(conversation_id)) => {
            ensure_exists(state, conversation_id).await?;
            keep_desktop(state, conversation_id, &params);
            let sent = state.conversations.open(conversation_id).send_prompt(params, origin).await;
            if let (Ok(result), Some(tty)) = (&sent, &context_tty) {
                state.conversations.routed(tty, conversation_id, shell_pid, result.seq);
            }
            sent
        }
        (Some(routing), None) => send_routed(state, params, origin, routing, &context_tty).await,
        (None, None) => {
            let conversation_id = ConversationId::from_uuid(uuid_v7(&*state.clock, &*state.rng));
            let handle = state.conversations.create(conversation_id, origin, None, None);
            keep_desktop(state, conversation_id, &params);
            let sent = handle.send_prompt(params, origin).await;
            if sent.is_err() {
                state.conversations.forget(conversation_id).await;
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

/// Sends a prompt from a terminal where `routing` says, and brings the terminal's
/// slots up to date once it committed.
async fn send_routed(
    state: &State,
    params: PromptSend,
    origin: efr_protocol::Origin,
    routing: Routing,
    context_tty: &Option<String>,
) -> Result<PromptSendResult, ConversationError> {
    let Routing { tty, key, label, target, claim, title, switch, hint } = routing;
    let shell_pid = params.context.as_ref().and_then(|context| context.shell_pid);
    // NOTE: the input marks the conversation that routing picked, before the prompt
    // (so its turn never reads the idle state that the input ends) and after it; the
    // terminal's current conversation may be another scope's, which the prompt does
    // not touch (`methods.rs`, `Input::Routed`).
    let touched = target.map(|id| state.presence.touch(id)).unwrap_or_default();
    let own = context_tty.as_deref() == Some(tty.as_str());
    let route = label.map(|scope| Routed { scope, switch, hint: hint.clone() });
    let sent = match (target, claim) {
        (Some(conversation_id), Some(before)) => {
            keep_desktop(state, conversation_id, &params);
            let attached = Event::TerminalAttached {
                tty: tty.clone(),
                route: key.clone(),
                shell_pid,
                how: AttachHow::PickedUp,
            };
            let handle = state.conversations.open(conversation_id);
            let routing = PromptRouting::new(Some(attached), route);
            let sent = handle.send_routed_prompt(params, origin, routing).await;
            match &sent {
                Ok(result) => {
                    let routed_seq = if own { result.seq } else { previous(result.seq) };
                    state.conversations.attached(&tty, &key, conversation_id, routed_seq, title);
                    tracing::info!(tty, conversation_id = %conversation_id, "a new terminal picked up the project's conversation");
                }
                // NOTE: nothing was recorded, so the slot gets back what it held (S-18).
                Err(_) => state.conversations.release_claim(&tty, &key, before),
            }
            sent
        }
        (Some(conversation_id), None) => {
            keep_desktop(state, conversation_id, &params);
            let handle = state.conversations.open(conversation_id);
            let sent =
                handle.send_routed_prompt(params, origin, PromptRouting::new(None, route)).await;
            if let (Ok(result), Some(context_tty)) = (&sent, context_tty) {
                state.conversations.routed(context_tty, conversation_id, shell_pid, result.seq);
            }
            sent
        }
        (None, _) => {
            let conversation_id = ConversationId::from_uuid(uuid_v7(&*state.clock, &*state.rng));
            let handle = state.conversations.create(
                conversation_id,
                origin,
                Some(tty.clone()),
                Some(key.clone()),
            );
            keep_desktop(state, conversation_id, &params);
            let sent =
                handle.send_routed_prompt(params, origin, PromptRouting::new(None, route)).await;
            match &sent {
                Ok(result) => {
                    // NOTE: `conversation_created` is the event before the prompt; the
                    // prompt moves the slot only when its context names the terminal.
                    let (routed_seq, pid) =
                        if own { (result.seq, shell_pid) } else { (previous(result.seq), None) };
                    state.conversations.set_slot(&tty, key, conversation_id, pid, routed_seq);
                }
                Err(_) => state.conversations.forget(conversation_id).await,
            }
            sent
        }
    };
    state.presence.record(touched).await;
    if let Ok(result) = &sent {
        state.presence.input(result.conversation_id).await;
        if let Some(hint) = hint {
            hint_shown(state, hint.root).await;
        }
    }
    sent
}

/// Records that the hint for the git work tree at `root` was shown now, in efrd and in
/// the state root's file.
async fn hint_shown(state: &State, root: PathBuf) {
    let (file, text) = state.repo_hints.shown(root, state.clock.now());
    let written = tokio::task::spawn_blocking(move || crate::repo_hint::write(&file, &text)).await;
    if written.is_err() {
        tracing::warn!("writing the repository hints panicked");
    }
}

/// The hint for a prompt outside projects in `pwd` (DV53): the root of the git work
/// tree that holds it, when that work tree got no hint in the last day. It runs one git
/// discovery (`efr_scope::derive`, as the turn's resolver does); `$HOME` and the
/// directories above it are never a repository here. A slow or failed run gives no
/// hint, never an error.
async fn repo_hint(
    state: &State,
    pwd: &std::path::Path,
    registry: &Registry,
    now: jiff::Timestamp,
) -> Option<RepoHint> {
    let begin = std::time::Instant::now();
    let derived = efr_scope::derive(pwd, &state.engine_parts.home, registry, &state.git).await;
    let elapsed_ms = begin.elapsed().as_millis();
    let root = match derived {
        Ok(derivation) if derivation.basis == Basis::WorkTree => {
            derivation.repo.map(|repo| repo.root)
        }
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(error = %error, elapsed_ms, "no repository hint: git could not tell");
            return None;
        }
    };
    tracing::debug!(elapsed_ms, root = ?root, "looked for a git work tree for the repository hint");
    let root = root?;
    state.repo_hints.due(&root, now).then_some(RepoHint { root })
}

/// The sequence number of the event right before `seq`, in the same batch.
fn previous(seq: Seq) -> Seq {
    Seq::new(seq.get().saturating_sub(1))
}

/// Where a prompt without a conversation id from the terminal `tty`, by the shell
/// `shell_pid`, goes (DV2), with the claim of a pick-up taken (DV3).
async fn route(
    state: &State,
    context: &ConnectionContext,
    params: &PromptSend,
    tty: String,
    shell_pid: Option<u32>,
) -> Result<Routing, DaemonError> {
    let settings = Arc::clone(&state.settings.borrow());
    let follow = settings.conversation.follow_project;
    let idle = routing::idle_limit(settings.conversation.tty_idle_hours);
    let now = state.clock.now();
    let pwd = params.context.as_ref().map(|context| context.pwd.clone());
    let (registry, resolved) = read_place(state, pwd.clone()).await;
    let pwd_key = pwd
        .as_deref()
        .map_or(RouteScope::Outside, |pwd| routing::route_key(&registry, pwd, resolved.as_deref()));
    let label = pwd.as_deref().map(|pwd| routing::label(&registry, &pwd_key, pwd));
    let hint = match (pwd.as_deref(), &pwd_key) {
        (Some(pwd), RouteScope::Outside) if settings.conversation.repo_hint => {
            repo_hint(state, pwd, &registry, now).await
        }
        _ => None,
    };
    let current = state.conversations.current(&tty);
    let alive = |pid: u32| state.procs.alive(pid);
    let before = match &current {
        Some((key, slot)) if !shell_gone(slot, shell_pid, alive) => {
            let summary = summary(state, slot.conversation_id).await?;
            Some(Before {
                conversation_id: slot.conversation_id,
                label: slot_label(&registry, key, summary.as_ref()),
            })
        }
        _ => None,
    };
    let key = routing::slot_key(
        current.as_ref().map(|(key, _)| key),
        &pwd_key,
        follow,
        pwd.is_some(),
        params.new_conversation,
    );
    let new = |outcome: Outcome| Routing {
        tty: tty.clone(),
        key: key.clone(),
        label: label.clone(),
        target: None,
        claim: None,
        title: None,
        switch: routing::switch(before.as_ref(), outcome),
        hint: hint.clone(),
    };
    if params.new_conversation {
        return Ok(new(Outcome::Fresh));
    }
    let slot = state.conversations.slot(&tty, &key);
    let mut ended = None;
    if let Some(slot) = slot {
        let summary = summary(state, slot.conversation_id).await?;
        let updated_at = summary.as_ref().map(|summary| summary.updated_at);
        match routing::continues(slot.shell_pid, shell_pid, updated_at, now, idle, alive) {
            Verdict::Continues => {
                let title = summary.as_ref().and_then(|summary| summary.title.clone());
                let outcome = Outcome::Continued {
                    conversation_id: slot.conversation_id,
                    title: title.clone(),
                    updated_at: updated_at.unwrap_or(now),
                };
                return Ok(Routing {
                    target: Some(slot.conversation_id),
                    title,
                    switch: routing::switch(before.as_ref(), outcome),
                    ..new(Outcome::Fresh)
                });
            }
            verdict => {
                tracing::info!(tty, conversation_id = %slot.conversation_id, ?verdict, "the terminal's conversation of the scope ended");
                ended = Some((verdict, updated_at));
            }
        }
    }
    if routing::may_pick_up(&key, follow, pwd.is_some())
        && let Some(picked) = pick_up(state, context, &tty, &key, slot, shell_pid).await?
    {
        let (conversation_id, summary) = picked;
        let outcome =
            Outcome::PickedUp { title: summary.title.clone(), updated_at: summary.updated_at };
        return Ok(Routing {
            target: Some(conversation_id),
            claim: Some(slot),
            title: summary.title,
            switch: routing::switch(before.as_ref(), outcome),
            ..new(Outcome::Fresh)
        });
    }
    let outcome = match ended {
        Some((Verdict::Idle, Some(updated_at))) => {
            Outcome::Idle { hours: routing::idle_hours(updated_at, now) }
        }
        Some(_) => Outcome::New,
        None => match state.conversations.take_moved(&tty, &key) {
            Some((to, title)) => Outcome::Moved { tty: to, title },
            None => Outcome::New,
        },
    };
    Ok(new(outcome))
}

/// The project's newest conversation for `tty`'s slot `key`, claimed, when a new
/// terminal may pick it up (DV3): the peer runs on the terminal, and the candidate is
/// idle, recent and held by no other open terminal. `slot` is what the slot holds now.
async fn pick_up(
    state: &State,
    context: &ConnectionContext,
    tty: &str,
    key: &RouteScope,
    slot: Option<Slot>,
    shell_pid: Option<u32>,
) -> Result<Option<(ConversationId, ConversationSummary)>, DaemonError> {
    if !conversations::peer_on_tty(state, context, tty) {
        tracing::debug!(tty, "no pick-up: the peer does not run on the terminal it names");
        return Ok(None);
    }
    let route = key.clone();
    let Some(newest) = state
        .readers
        .with(move |conn| efr_store::conversations::newest_of_route(conn, &route))
        .await?
    else {
        return Ok(None);
    };
    let held = conversations::held(state, newest.id, Some(tty));
    if let Some(held) = &held {
        tracing::debug!(tty, holder = held.tty(), conversation_id = %newest.id, "no pick-up: another terminal holds the conversation");
    }
    let candidate = Candidate {
        id: newest.id,
        status: newest.status,
        updated_at: newest.updated_at,
        held: held.is_some(),
    };
    let window = routing::idle_limit(state.settings.borrow().conversation.tty_idle_hours);
    let Some(conversation_id) = routing::pick_up(Some(&candidate), state.clock.now(), window)
    else {
        return Ok(None);
    };
    let expected = slot.map(|slot| slot.conversation_id);
    let alive = |pid: u32| state.procs.alive(pid);
    if state.conversations.claim(tty, key, conversation_id, shell_pid, expected, &alive) {
        Ok(Some((conversation_id, newest)))
    } else {
        tracing::info!(tty, conversation_id = %conversation_id, "another terminal claimed the conversation first");
        Ok(None)
    }
}

/// True when the slot's shell is another one than `asking` and has exited.
fn shell_gone(slot: &Slot, asking: Option<u32>, alive: impl Fn(u32) -> bool) -> bool {
    matches!((slot.shell_pid, asking), (Some(holder), Some(asking)) if holder != asking && !alive(holder))
}

/// The label of a slot's key: its project, else the last directory of its
/// conversation.
fn slot_label(
    registry: &Registry,
    key: &RouteScope,
    summary: Option<&ConversationSummary>,
) -> RouteLabel {
    let dir = summary.and_then(|summary| summary.cwd.clone()).unwrap_or_default();
    routing::label(registry, key, &dir)
}

async fn summary(
    state: &State,
    conversation_id: ConversationId,
) -> Result<Option<ConversationSummary>, DaemonError> {
    Ok(state.readers.with(move |conn| efr_store::conversations::get(conn, conversation_id)).await?)
}

/// The project registry as the file holds it now, and `pwd` with its links resolved,
/// read on the blocking pool; the key needs no git run. A registry that cannot be read
/// counts as empty, so the prompt goes outside projects, and the next turn reports it.
/// A registry that differs from the zsh plugin's copy (`projects_file.rs`), after a
/// hand edit of `projects.toml`, is written there again.
pub(crate) async fn read_place(state: &State, pwd: Option<PathBuf>) -> (Registry, Option<PathBuf>) {
    let file = state.engine_parts.registry.clone();
    let projects = crate::projects_file::path(state.dirs.runtime());
    let read = tokio::task::spawn_blocking(move || {
        let registry = Registry::load(&file);
        let written = registry.as_ref().ok().map(|registry| {
            crate::projects_file::sync(&projects, &crate::projects_file::text(registry))
        });
        crate::projects_file::log(written);
        let resolved = pwd.and_then(|pwd| std::fs::canonicalize(pwd).ok());
        (registry, resolved)
    })
    .await;
    match read {
        Ok((Ok(registry), resolved)) => (registry, resolved),
        Ok((Err(error), resolved)) => {
            tracing::warn!(error = %error, "the project registry could not be read for routing");
            (Registry::empty(), resolved)
        }
        Err(_) => {
            tracing::warn!("reading the project registry for routing panicked");
            (Registry::empty(), None)
        }
    }
}

/// Keeps the desktop variables of the terminal of a prompt with a shell context for
/// the desktop grants of its conversation's calls; they go to no model. A prompt
/// without a context, such as one from the phone, keeps the ones before it.
fn keep_desktop(state: &State, conversation_id: ConversationId, params: &PromptSend) {
    if let Some(context) = &params.context {
        state.sandbox.set_desktop(conversation_id, context.desktop.as_deref());
    }
}

/// Refuses a prompt that names no model of its own while `[model] name` is a model of
/// another company than `[model] provider`.
fn refuse_foreign_model(state: &State, asked: &TurnSettings) -> Result<(), DaemonError> {
    if asked.model.is_some() {
        return Ok(());
    }
    let settings = Arc::clone(&state.settings.borrow());
    match settings.foreign_model() {
        Some(foreign) => Err(DaemonError::ForeignModel {
            name: foreign.name.to_owned(),
            company: foreign.company,
            provider: foreign.provider.to_owned(),
        }),
        None => Ok(()),
    }
}

/// Runs the sandbox probe again before a prompt whose `asked` settings are `auto`, or
/// whose default mode is `auto`, while the last probe said the sandbox is unavailable,
/// so a user who fixed the cause needs no restart (efr's auto spec, section 12.1).
pub(crate) async fn reprobe_for_auto(state: &State, asked: &TurnSettings) {
    let settings = Arc::clone(&state.settings.borrow());
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
    let exists = summary(state, conversation_id).await?.is_some();
    if exists { Ok(()) } else { Err(DaemonError::ConversationNotFound { conversation_id }) }
}

#[cfg(test)]
mod tests;
