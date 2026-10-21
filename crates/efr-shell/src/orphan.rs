//! A command left running in the hidden shell: a run whose timeout passed while its
//! command went on. The shell takes no other marked line until it ends.
//!
//! The run's progress sink does not live past its call, so a task of its own watches
//! the command ([`OrphanWatch`]): once per quiet period it looks whether the command
//! waits for input, as the run did, with the wait that the run reported last. The
//! session's actor turns each change into a notice. The watch also stops a wait that
//! may not stay:
//!
//! - In the sandbox, a hidden wait and a visible wait that looks secret stop at once:
//!   efr types no secret into a sandboxed command.
//! - Outside it, such a wait stays while a client that can answer follows the
//!   conversation ([`Followers`]). When nobody can answer, it stays only for a line that
//!   may ask at the terminal and that a person approved, the user or the user's own
//!   rule. Every other such wait stops at once, as a run's does. So a line that fakes a
//!   `sudo` prompt in an exit that the reviewer or full mode let through never waits
//!   for the user at a later prompt.
//! - Any wait ends at the run's limit, counted from its start: the watch sends `SIGINT`
//!   to the job that waits and, when the command is still there after the close grace,
//!   closes the shell (`SIGHUP`, then `SIGKILL`). The next call starts a new shell.
//!
//! A stop sends `SIGINT` only while the job whose wait was reported holds the terminal,
//! as read right before the signal. The task holds no lock across an await and sleeps
//! on the injected clock.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use efr_holder::{PtyHolder, Signal, SignalTarget};
use efr_protocol::{ApprovedBy, CallId, ConversationId, InputWait, ShellCommandEnd};
use efr_stdx::time::Clock;
use jiff::Timestamp;

use crate::input::{InputWatch, Offer, Quiet};
use crate::session::{Looked, Msg, SessionHandle};

/// A command left running in a conversation's hidden shell, as
/// [`ShellSessions::busy`](crate::ShellSessions::busy) tells it. The daemon maps it to
/// the one wire type, `efr_protocol::BusyShell`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftRunning {
    /// The tool call whose command it is, when the run named one.
    pub call: Option<CallId>,
    /// The first 200 characters of the command line, as the model wrote it.
    pub command: String,
    /// When its run was asked for.
    pub since: Timestamp,
    /// True when the command runs in the auto mode's sandbox.
    pub contained: bool,
    /// What it waits for now.
    pub input: InputWait,
    /// True for a visible wait whose prompt reads like a password prompt while the
    /// terminal is not in line mode.
    pub looks_secret: bool,
    /// The row of the screen that holds the cursor while the command waits, at most 200
    /// characters; empty while it does not wait.
    pub prompt: String,
    /// Who approved the call; `None` when it needed no approval.
    pub approved_by: Option<ApprovedBy>,
}

/// Tells how many clients that can answer follow a conversation. The daemon counts the
/// connections with the `terminal` scope that follow it.
///
/// It is asked from the watch of a command left running and must not block.
pub trait Followers: Send + Sync + fmt::Debug {
    /// How many clients that can answer follow the conversation now.
    fn answerers(&self, conversation: ConversationId) -> usize;
}

/// [`Followers`] for a manager whose caller gives none: nobody follows.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NoFollowers;

impl Followers for NoFollowers {
    fn answerers(&self, _conversation: ConversationId) -> usize {
        0
    }
}

/// What the watch of a command left running knows of its run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LeftRun {
    /// The run's id, which a look names.
    pub(crate) id: u64,
    /// Which waits the run reports.
    pub(crate) offer: Offer,
    /// The command runs in the auto mode's sandbox.
    pub(crate) contained: bool,
    /// A hidden wait may stay while nobody follows: the line may ask at the terminal,
    /// and a person approved it.
    pub(crate) kept_unfollowed: bool,
    /// When the run was asked for.
    pub(crate) started: Timestamp,
    /// The longest a wait of the command may last, counted from `started`.
    pub(crate) limit: Option<Duration>,
    /// The wait that the run reported last.
    pub(crate) watch: InputWatch,
}

/// The task that watches one command left running.
pub(crate) struct OrphanWatch {
    pub(crate) session: SessionHandle,
    pub(crate) run: LeftRun,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) holder: Arc<dyn PtyHolder>,
    pub(crate) followers: Arc<dyn Followers>,
    pub(crate) quiet: Quiet,
    pub(crate) quiet_period: Duration,
    pub(crate) close_grace: Duration,
}

impl OrphanWatch {
    /// Looks once per quiet period until the command ends, the shell ends, or a stop
    /// is done.
    pub(crate) async fn run(mut self) {
        loop {
            let next = self.clock.sleep(self.quiet_period);
            tokio::select! {
                () = next => {}
                _ = self.session.ended() => return,
            }
            let looked = self
                .session
                .look(self.run.id, self.run.offer, self.quiet, &*self.clock, &mut self.run.watch)
                .await;
            match looked {
                Ok(Looked::Same) => {}
                Ok(Looked::Changed { told: Ok(()), .. }) => {}
                Ok(Looked::Gone | Looked::Changed { told: Err(_), .. }) | Err(_) => return,
            }
            if self.must_stop() {
                self.stop().await;
                return;
            }
        }
    }

    /// True when the command's wait may not stay: a wait for a secret that no client
    /// may answer, or any wait past the run's limit.
    fn must_stop(&self) -> bool {
        let wait = self.run.watch.current();
        if wait == InputWait::None {
            return false;
        }
        let secret = wait == InputWait::Hidden
            || (wait == InputWait::Visible && self.run.watch.looks_secret());
        if secret && self.run.contained {
            return true;
        }
        if secret
            && !self.run.kept_unfollowed
            && self.followers.answerers(self.session.conversation) == 0
        {
            return true;
        }
        let elapsed = Duration::try_from(self.clock.now().duration_since(self.run.started))
            .unwrap_or_default();
        self.run.limit.is_some_and(|limit| elapsed >= limit)
    }

    /// Stops the command: `SIGINT` to the job that waits, while it holds the terminal;
    /// then, when the command is still there after the close grace, the shell is
    /// closed.
    async fn stop(&self) {
        let session = &self.session;
        let conversation = session.conversation;
        let id = self.run.id;
        let how = Some(ShellCommandEnd::Limit);
        if session.send(Msg::Stopping { id, how }).await.is_err() {
            return;
        }
        // NOTE: the command may have ended since the look. zsh then holds the terminal
        // again and runs its precmd hooks, where the signal could cut short the hook
        // that prints `D`. So the group is read right before the signal, which goes out
        // only while the job whose wait was reported holds the terminal.
        let group = self.run.watch.waiting().map(|waiting| waiting.group);
        match group.map(|group| session.terminal.holds(group)) {
            Some(Ok(true)) => {
                tracing::info!(%conversation, "a command left running in the hidden shell waits for input that may not wait; interrupting it");
                let signal = self
                    .holder
                    .signal(session.pty_id, Signal::Interrupt, SignalTarget::ForegroundGroup)
                    .await;
                if let Err(error) = signal {
                    tracing::warn!(%conversation, %error, "could not interrupt a command left running in the hidden shell");
                }
            }
            Some(Ok(false)) | None => {
                tracing::debug!(%conversation, "the job that waited no longer holds the terminal; nothing is interrupted");
            }
            Some(Err(error)) => {
                tracing::warn!(%conversation, %error, "could not read who holds the terminal of the hidden shell");
            }
        }
        if self.clock.timeout(self.close_grace, session.orphan_gone(id)).await.is_ok() {
            return;
        }
        tracing::info!(%conversation, "a command left running in the hidden shell did not end; closing the shell");
        if let Err(error) = session.close(&*self.holder, &*self.clock, self.close_grace).await {
            tracing::warn!(%conversation, %error, "could not close the hidden shell");
        }
    }
}

#[cfg(test)]
mod tests;
