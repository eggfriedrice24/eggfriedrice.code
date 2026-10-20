//! The rules of routing a prompt by scope, as pure functions: the scope key of a
//! prompt, whether a terminal's slot continues, the pick-up of a project's newest
//! conversation, and the switch that the line before the reply names.
//!
//! A terminal keeps one slot per scope key: one for each registered project that it
//! worked in, and one for every place outside projects (`conversations.rs` keeps the
//! map, `methods/prompt_send.rs` reads it and the store). Nothing here reads a clock,
//! a file or a process: the caller passes what it read.

use std::path::Path;
use std::time::Duration;

use efr_protocol::{ConversationId, ConversationStatus, NewReason, RouteLabel, RouteScope, Switch};
use efr_scope::Registry;
use jiff::Timestamp;

/// The scope key of a prompt whose working directory is `pwd`, and `resolved` with
/// symbolic links resolved: the registered project whose root holds either (the
/// deepest root wins, so a subdirectory of a project is that project), else
/// `outside`. Rule 1 of `efr_scope::derive`, without git.
pub(crate) fn route_key(registry: &Registry, pwd: &Path, resolved: Option<&Path>) -> RouteScope {
    std::iter::once(pwd)
        .chain(resolved)
        .find_map(|form| registry.containing(form))
        .map_or(RouteScope::Outside, |project| RouteScope::Project(project.id()))
}

/// The label of the key `scope` for a prompt in `pwd`: the project's name and root,
/// or `pwd` outside projects. A project that is no longer registered keeps its key
/// with no name.
pub(crate) fn label(registry: &Registry, scope: &RouteScope, pwd: &Path) -> RouteLabel {
    match scope.project().and_then(|id| registry.get(&id)) {
        Some(project) => RouteLabel {
            scope: scope.clone(),
            name: Some(project_name(project.name(), project.root())),
            dir: project.root().to_path_buf(),
        },
        None => RouteLabel { scope: scope.clone(), name: None, dir: pwd.to_path_buf() },
    }
}

/// A project's name: the registered one, else the last part of its root.
fn project_name(name: Option<&str>, root: &Path) -> String {
    name.map(str::to_owned).unwrap_or_else(|| {
        root.file_name()
            .map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
    })
}

/// Whether a terminal's slot takes a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The prompt continues the slot's conversation.
    Continues,
    /// The slot's conversation was not active for the idle time; its shell still runs.
    Idle,
    /// Another shell sends the prompt and the shell that holds the slot has exited: a
    /// new tab got the `/dev/pts` number of a closed one.
    ShellGone,
}

/// The key of the slot that a prompt uses: the key of its working directory
/// (`pwd_key`), or, with `follow_project` off or without a working directory, the key
/// of the terminal's current slot (`current`), so the prompt keeps to the terminal's
/// one conversation (DV54). A `,new` always takes the slot of the pwd's key.
pub(crate) fn slot_key(
    current: Option<&RouteScope>,
    pwd_key: &RouteScope,
    follow_project: bool,
    has_pwd: bool,
    new_conversation: bool,
) -> RouteScope {
    match current {
        Some(key) if !new_conversation && (!follow_project || !has_pwd) => key.clone(),
        _ => pwd_key.clone(),
    }
}

/// Whether a prompt may pick up a conversation for the slot `key`: only a project's
/// key, with `follow_project` on and a working directory. The outside scope never picks
/// one up (OQV19): the outside conversation of a closed terminal is most often about
/// another task, and `,resume` brings one back.
pub(crate) fn may_pick_up(key: &RouteScope, follow_project: bool, has_pwd: bool) -> bool {
    follow_project && has_pwd && key.project().is_some()
}

/// The idle time of `conversation.tty_idle_hours`; `None` for 0, which continues a
/// conversation forever.
pub(crate) fn idle_limit(hours: u64) -> Option<Duration> {
    (hours > 0).then(|| Duration::from_secs(hours.saturating_mul(3600)))
}

/// Whether the slot held by the shell `holder` continues for a prompt from the shell
/// `asking` at `now`, when the slot's conversation was last active at `updated_at`.
/// The shell is checked first (F21): a new tab on a reused terminal is a new shell,
/// however long the old conversation was idle. A shell that is not the holder while
/// the holder still runs (a nested `zsh`, a `sudo -s`) continues the slot.
pub(crate) fn continues(
    holder: Option<u32>,
    asking: Option<u32>,
    updated_at: Option<Timestamp>,
    now: Timestamp,
    idle: Option<Duration>,
    alive: impl FnOnce(u32) -> bool,
) -> Verdict {
    if let (Some(holder), Some(asking)) = (holder, asking)
        && holder != asking
        && !alive(holder)
    {
        return Verdict::ShellGone;
    }
    if let (Some(limit), Some(updated_at)) = (idle, updated_at)
        && idle_for(updated_at, now).is_some_and(|idle| idle > limit)
    {
        return Verdict::Idle;
    }
    Verdict::Continues
}

/// How long since `updated_at`; `None` when it is in the future.
fn idle_for(updated_at: Timestamp, now: Timestamp) -> Option<Duration> {
    Duration::try_from(now.duration_since(updated_at)).ok()
}

/// The single newest conversation of a project's key (F4), as a pick-up sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) id: ConversationId,
    pub(crate) status: ConversationStatus,
    pub(crate) updated_at: Timestamp,
    /// A live slot of a terminal points to it, or a terminal follows it.
    pub(crate) held: bool,
}

/// The conversation that a prompt from a new terminal picks up: the candidate when it
/// is idle, was active within `window` (`None` is no age limit), and no open terminal
/// holds it. Never an older one: when the newest does not qualify, a new
/// conversation starts.
pub(crate) fn pick_up(
    candidate: Option<&Candidate>,
    now: Timestamp,
    window: Option<Duration>,
) -> Option<ConversationId> {
    let candidate = candidate?;
    let recent = window
        .is_none_or(|window| idle_for(candidate.updated_at, now).is_none_or(|idle| idle <= window));
    (candidate.status == ConversationStatus::Idle && recent && !candidate.held)
        .then_some(candidate.id)
}

/// What the terminal held before the prompt, for the switch line: its current slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Before {
    pub(crate) conversation_id: ConversationId,
    /// The label of the current slot's scope: a project, or its last directory.
    pub(crate) label: RouteLabel,
}

/// Where the prompt went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The terminal's slot of the key continued.
    Continued { conversation_id: ConversationId, title: Option<String>, updated_at: Timestamp },
    /// The prompt picked up the project's newest conversation.
    PickedUp { title: Option<String>, updated_at: Timestamp },
    /// The user asked for a new conversation (`,new`).
    Fresh,
    /// A new conversation, because the slot was idle for `hours`.
    Idle { hours: u64 },
    /// A new conversation, because another terminal took the slot's conversation.
    Moved { tty: String, title: Option<String> },
    /// A new conversation, because the terminal had none for the key, or the slot's
    /// shell has exited.
    New,
}

/// The switch that the line before the reply names, from the terminal's current slot
/// `before` and where the prompt went. The first prompt of a terminal (or of a new
/// shell on a reused one) and a `,new` name none.
pub(crate) fn switch(before: Option<&Before>, outcome: Outcome) -> Option<Switch> {
    match outcome {
        Outcome::Continued { conversation_id, title, updated_at } => {
            let changed = before.is_some_and(|before| before.conversation_id != conversation_id);
            changed.then_some(Switch::Back { title, updated_at })
        }
        Outcome::PickedUp { title, updated_at } => Some(Switch::PickedUp { title, updated_at }),
        Outcome::Fresh => None,
        Outcome::Idle { hours } => {
            Some(Switch::New { from: None, reason: NewReason::Idle { hours } })
        }
        Outcome::Moved { tty, title } => {
            Some(Switch::New { from: None, reason: NewReason::Moved { tty, title } })
        }
        Outcome::New => before.map(|before| Switch::New {
            from: Some(before.label.clone()),
            reason: NewReason::Scope,
        }),
    }
}

/// The whole hours between `updated_at` and `now`, for the idle reason.
pub(crate) fn idle_hours(updated_at: Timestamp, now: Timestamp) -> u64 {
    idle_for(updated_at, now).map_or(0, |idle| idle.as_secs() / 3600)
}

#[cfg(test)]
mod tests;
