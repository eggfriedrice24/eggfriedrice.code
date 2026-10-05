//! Closing hidden shells nobody uses.
//!
//! A shell's activity mark is its recorded output plus the input typed into it: when
//! the mark has not moved for the idle time, the shell sits at a prompt, no client is
//! attached or holds a lease on it, and its conversation runs no turn, the shell is
//! closed. The next command of the conversation starts a new one in the user's
//! directory. [`decide`] is the rule; [`collect`] runs it on the injected clock.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::PtyId;
use efr_shell::Phase;
use jiff::Timestamp;
use tokio_util::sync::CancellationToken;

use crate::state::State;

/// How often the collector looks.
pub(crate) const INTERVAL: Duration = Duration::from_secs(60);

/// What the collector remembers about one shell between looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Seen {
    /// The activity mark at the last look.
    pub(crate) mark: u64,
    /// When the mark last moved.
    pub(crate) since: Timestamp,
}

/// What the collector knows about one shell now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Look {
    pub(crate) mark: u64,
    pub(crate) at_prompt: bool,
    /// A client is attached, holds a lease, or a turn of its conversation runs.
    pub(crate) in_use: bool,
}

/// Whether to close a shell seen as `look` at `now`, given what was `seen` before, and
/// what to remember for the next look.
pub(crate) fn decide(
    seen: Option<Seen>,
    look: Look,
    now: Timestamp,
    idle: Duration,
) -> (Seen, bool) {
    let seen = match seen {
        Some(seen) if seen.mark == look.mark && !look.in_use => seen,
        _ => Seen { mark: look.mark, since: now },
    };
    let quiet = Duration::try_from(now.duration_since(seen.since)).unwrap_or(Duration::ZERO);
    (seen, look.at_prompt && !look.in_use && quiet >= idle)
}

/// True for a phase in which the shell waits at its prompt. A shell without marks
/// cannot tell; it counts as waiting because its mark has not moved.
pub(crate) fn at_prompt(phase: Phase) -> bool {
    matches!(phase, Phase::Ready | Phase::Prompting { .. } | Phase::Unmarked)
}

/// The idle time of `shell.idle_minutes`, or `None` when it is 0 and idle shells stay.
pub(crate) fn idle_time(minutes: u64) -> Option<Duration> {
    (minutes > 0).then(|| Duration::from_secs(minutes.saturating_mul(60)))
}

/// Looks every [`INTERVAL`] until `stop`, and closes the shells that [`decide`] picks.
/// The idle time is read from the settings at each look, so a reload changes it.
pub(crate) async fn collect(state: Arc<State>, stop: CancellationToken) {
    let mut seen: HashMap<PtyId, Seen> = HashMap::new();
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = state.clock.sleep(INTERVAL) => {}
        }
        let Some(idle) = idle_time(state.settings.borrow().shell.idle_minutes) else {
            seen.clear();
            continue;
        };
        let now = state.clock.now();
        let activity = state.ptys.activity();
        seen.retain(|pty_id, _| activity.iter().any(|pty| pty.pty_id == *pty_id));
        for pty in activity {
            let Ok(shell) = state.shells.state(pty.conversation).await else {
                continue;
            };
            let running = match state.conversations.live(pty.conversation) {
                Some(handle) => handle.state().await.is_ok_and(|now| now.running.is_some()),
                None => false,
            };
            let look = Look {
                mark: pty.mark,
                at_prompt: at_prompt(shell.phase),
                in_use: pty.attached || running || state.connections.watches_pty(pty.pty_id, now),
            };
            let (next, close) = decide(seen.get(&pty.pty_id).copied(), look, now, idle);
            seen.insert(pty.pty_id, next);
            if close {
                tracing::info!(pty_id = %pty.pty_id, conversation_id = %pty.conversation, "closing an idle hidden shell");
                if let Err(error) = state.shells.close(pty.conversation).await {
                    tracing::warn!(error = %error, "an idle shell could not be closed");
                }
                seen.remove(&pty.pty_id);
            }
        }
    }
}

#[cfg(test)]
mod tests;
