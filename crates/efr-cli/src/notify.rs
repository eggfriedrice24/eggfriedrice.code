//! The desktop notice of the terminal (OSC 777) while `efr` follows a turn: one when a
//! question or an answer line waited [`WAIT_AFTER`] with no key, and one at the end of
//! the last turn of the view when that turn took at least `notify.after_secs` and no
//! key came in the last [`WAIT_AFTER`].
//!
//! Ghostty shows `OSC 777 ; notify ; <title> ; <body>` as a notice of the desktop, also
//! in the focused split, so no notice goes within a time after a key: a user who is at
//! the keys never sees it. tmux drops the sequence, and other terminals may print
//! it. So `notify.terminal = "auto"` sends it only in Ghostty, never through a
//! multiplexer (tmux, GNU screen, zellij), and only when replies are formatted. No query
//! decides it: a query needs a reply on stdin, which would take the keys that the user
//! typed ahead for the shell.
//!
//! The title is `efr`, and the body is `<place>: <what>`
//! ([`efr_stdx::text::notice_body`]). The operating system keeps a history of notices,
//! so a body never holds a prompt, a command line, a title or text of the model.
//!
//! The view sends the sequence in its frame; this module names it and decides whether
//! it goes out at all.

use std::path::Path;
use std::time::Duration;

use efr_config::{AutoOnOff, NotifySettings};
use efr_protocol::{RouteLabel, RouteScope};
use efr_stdx::text::{INPUT_WAITS, QUESTION_WAITS, notice_body, notice_ended, notice_place};
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;

use crate::terminal::TermFacts;

/// How long a question or an answer line waits with no key before its notice goes.
pub(crate) const WAIT_AFTER: Duration = Duration::from_secs(10);

/// The desktop notices of one followed view: where they come from, and how long a turn
/// must take for its end to send one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notices {
    /// The `<place>` of each body ([`place`]).
    place: String,
    /// `notify.after_secs`.
    after: Duration,
    /// When the last key came.
    key_at: Option<Timestamp>,
}

impl Notices {
    /// The notices that `settings` asks for on the terminal of `facts`, with `place` as
    /// the place of each body; `None` when no notice goes out ([`wanted`]).
    pub(crate) fn of(settings: &NotifySettings, facts: &TermFacts, place: String) -> Option<Self> {
        wanted(settings.terminal, facts).then(|| Notices {
            place,
            after: Duration::from_secs(settings.after_secs.into()),
            key_at: None,
        })
    }

    /// A key came at `now`.
    pub(crate) fn keyed(&mut self, now: Timestamp) {
        self.key_at = Some(now);
    }

    /// True when a key came less than [`WAIT_AFTER`] before `now`: the user is at the
    /// keys, so no notice goes out. A wait sends its notice only after that time with
    /// no key, but the end of a turn can come right after a key, such as the answer to
    /// a card.
    pub(crate) fn at_the_keys(&self, now: Timestamp) -> bool {
        self.key_at.is_some_and(|at| {
            Duration::try_from(now.duration_since(at)).ok().is_none_or(|since| since < WAIT_AFTER)
        })
    }

    /// The body for a wait of the user: a card (`question`) or an answer line.
    pub(crate) fn wait(&self, question: bool) -> String {
        notice_body(&self.place, if question { QUESTION_WAITS } else { INPUT_WAITS })
    }

    /// The body for the end of the last turn of the view, which took `took` and failed
    /// or not: `app: done in 4m 12s`, `app: failed after 1m 3s`. `None` when the turn
    /// took less than `notify.after_secs`.
    pub(crate) fn ended(&self, took: Duration, failed: bool) -> Option<String> {
        (took >= self.after).then(|| notice_ended(&self.place, took, failed))
    }
}

/// The time of the desktop notice of a wait of the user, which `W` names: it starts
/// with the wait, a key or the end of the wait stops it, and each wait has one at most.
pub(crate) struct WaitNotice<W> {
    /// The time until the notice, while it runs.
    sleep: Option<Sleep>,
    /// The wait that had its time last: no second time starts before another wait.
    seen: Option<W>,
}

impl<W> Default for WaitNotice<W> {
    fn default() -> Self {
        WaitNotice { sleep: None, seen: None }
    }
}

impl<W: PartialEq> WaitNotice<W> {
    /// Follows the view: `wait` is the wait of the user that shows here while notices
    /// go out, else `None`. The time starts on the first call of a wait, also when
    /// another wait follows at once (a card, then the answer line that still waits),
    /// and stops when the wait ends.
    pub(crate) fn watch(&mut self, wait: Option<W>, clock: &dyn Clock) {
        if wait == self.seen {
            return;
        }
        self.sleep = wait.is_some().then(|| clock.sleep(WAIT_AFTER));
        self.seen = wait;
    }

    /// A key came: the user is at the keys, so this wait sends no notice.
    pub(crate) fn key(&mut self) {
        // NOTE: the view also keeps the time of the key, so the end of the turn sends no
        // notice right after a key ([`Notices::keyed`]).
        self.sleep = None;
    }

    /// Ends when the notice is due; never while no time runs. It stops the time, so the
    /// notice is due once.
    pub(crate) async fn due(&mut self) {
        match &mut self.sleep {
            Some(sleep) => {
                sleep.as_mut().await;
                self.sleep = None;
            }
            None => std::future::pending().await,
        }
    }

    /// True while the time runs.
    #[cfg(test)]
    pub(crate) fn runs(&self) -> bool {
        self.sleep.is_some()
    }
}

/// True when a desktop notice goes out ([`TermFacts::allows`]): `auto` only in Ghostty.
pub(crate) fn wanted(setting: AutoOnOff, facts: &TermFacts) -> bool {
    facts.allows(setting, facts.ghostty())
}

/// The sequence that shows `body` as a desktop notice with the title `efr`. `body`
/// comes from [`efr_stdx::text::notice_body`], which keeps every control character out.
pub(crate) fn sequence(body: &str) -> String {
    format!("\x1b]777;notify;efr;{body}\x1b\\")
}

/// The `<place>` of a body, by the rule of efrd's notices ([`notice_place`]): the name
/// of the project of `scope`, else the last part of its directory, else the last part
/// of `cwd`, else `~`.
pub(crate) fn place(scope: Option<&RouteLabel>, cwd: Option<&Path>) -> String {
    let project = scope.and_then(|scope| match scope.scope {
        RouteScope::Project(_) => scope.name.clone(),
        _ => None,
    });
    notice_place(project, scope.map(|scope| scope.dir.as_path()).or(cwd))
}

#[cfg(test)]
mod tests;
