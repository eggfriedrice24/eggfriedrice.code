//! Whether a running command waits for input, and what an answer may be.
//!
//! A run looks once per `quiet_period` while its command runs (between `C` and `D`, or
//! the two sentinels):
//!
//! - [`InputWait::Hidden`]: the terminal has echo off and canonical input on, as a
//!   getpass-style read leaves it, and the output has been quiet for `quiet_period`.
//! - [`InputWait::Visible`]: the terminal is in any other modes, raw or cooked, echo on
//!   or off, the output has been quiet for `visible_input_quiet`, and the main screen
//!   has the cursor after some text, as after `[Y/n] `. Only then is the screen read. A
//!   command whose last line is merely unfinished looks the same, so this is a guess,
//!   and it never stops a command. The modes cannot narrow it: a relay such as `sudo`
//!   (with `use_pty`, its default) or `script` puts the terminal in raw mode and runs
//!   the program on a terminal of its own, whose modes are not read here.
//! - [`InputWait::None`] otherwise, and when the modes cannot be read. A full-screen
//!   program on the alternate screen is judged only at the run's timeout, as before.
//!
//! A visible wait looks secret when the terminal is not in line mode, as behind a relay
//! (`sudo`'s own pty, `ssh`, `docker exec`), and the cursor's row reads like a password
//! prompt ([`looks_secret`]). The program on the inner terminal decides whether the
//! answer is shown, so a client hides what the user types; the answer still goes as a
//! visible one, so the kind check holds.
//!
//! What a run may report depends on the run ([`Offer`]). A prompt that reads command
//! lines (a shell's `$ `, a REPL's `>>> `) looks like a visible question, and an answer
//! there would run as a command line, so two kinds of run report hidden waits only. A
//! sentinel run ([`RunMode::Sentinel`]) types into a shell started inside the hidden
//! one, whose prompt comes back after each command. A run whose command line starts an
//! interactive shell or a REPL (`nested_shell.rs`) leaves one at its prompt. A shell's
//! or a REPL's prompt is never a getpass-style read, so the password prompt of `sudo
//! -i`, `ssh` or `su`, or of a command in a nested shell, is still reported.
//!
//! A wait belongs to the job that was in the terminal's foreground at the look that
//! reported it, by its process group, and an answer reaches only that job. It must also
//! be of the wait's kind, which is what a client asked the user for: once `sudo -i` took
//! its password, its relay would run a visible answer as a command line until the next
//! look ends the hidden wait. An answer resets the wait to `None` for one look, so the same prompt asked again (`Sorry, try
//! again.`) is a new change that a client asks the user about again; so does another
//! job in the foreground, whose own wait is a new change at the look after.
//!
//! NOTE: a wait is about a job, not about one prompt. When one hidden prompt of a job
//! ends and another of the same job starts, the change shows only at the next look (and
//! not at all when the new prompt prints nothing, since both are `Hidden`), so an answer
//! typed for the first prompt in that time goes to the second. A prompt of another job
//! never gets it: the group differs.

use std::time::Duration;

use efr_protocol::{InputRespond, InputWait, ScreenSnapshot};
use efr_screen::row_text;
use jiff::Timestamp;

use crate::modes::{InputModes, Job};
use crate::run::cursor_after_text;
use crate::{RunMode, ShellError, nested_shell};

/// Refuses an answer that is not one line of at most [`InputRespond::MAX_TEXT_BYTES`]
/// bytes. The reason never quotes the text, which can be a password.
pub(crate) fn check_answer(text: &str) -> Result<(), ShellError> {
    if text.len() > InputRespond::MAX_TEXT_BYTES {
        return Err(ShellError::InvalidAnswer { reason: "it is longer than input.respond allows" });
    }
    if text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err(ShellError::InvalidAnswer { reason: "it contains a control character" });
    }
    Ok(())
}

/// Refuses to type an answer into a terminal in `modes`, the modes of the job in its
/// foreground. A hidden answer needs a getpass-style read, a line with echo off, so it
/// never reaches the output. A visible answer takes any modes: behind a relay such as
/// `sudo`'s own terminal they are the relay's raw ones, and the program on the inner
/// terminal decides what the text does there.
///
/// NOTE: the modes cannot keep an answer from the shell: zsh runs its precmd hooks in
/// cooked mode and its line editor in raw mode, and an external command that a later
/// hook starts is cooked too. The caller refuses while the shell's own process group
/// holds the terminal, and while any job other than the one whose wait was reported
/// does ([`check_job`]); the integration drains unread input before `D`. The session's
/// `answer` names what is left.
pub(crate) fn check_modes(modes: InputModes, hidden: bool) -> Result<(), &'static str> {
    if !hidden {
        return Ok(());
    }
    if !modes.canonical {
        return Err("the terminal is not reading a line");
    }
    if modes.echo {
        return Err("the terminal echoes what is typed");
    }
    Ok(())
}

/// Refuses to type an answer unless a wait was reported and `foreground`, the process
/// group in the terminal's foreground now, is the group of the job that was there at
/// the look that reported it; returns that wait. Typed for a job that ended, an answer
/// stays in the terminal for whatever reads next: a later job, or the line editor,
/// which would run it as a command line.
pub(crate) fn check_job(
    waiting: Option<Waiting>,
    foreground: u32,
) -> Result<Waiting, &'static str> {
    match waiting {
        None => Err("no wait was reported for the command"),
        Some(waiting) if waiting.group != foreground => {
            Err("the job that waited no longer holds the terminal")
        }
        Some(waiting) => Ok(waiting),
    }
}

/// True when a run of `command` in `mode` takes a manual answer
/// ([`ShellSessions::answer_manual`](crate::ShellSessions::answer_manual)): when it
/// reports visible waits, so its command does not type into a shell or a REPL that
/// reads command lines. A client offers a manual answer only for such a run.
pub fn takes_manual_answers(mode: RunMode, command: &str) -> bool {
    Offer::of(mode, command) == Offer::All
}

/// Refuses a manual answer for a run that reports hidden waits only. Such a run types
/// into a shell or a REPL that reads command lines: a sentinel run's line, or a command
/// that starts one. Once the command ends, that shell reads what is still unread as its
/// next command line, and nothing drains it there, so a manual answer that the command
/// did not read would run as a command.
pub(crate) fn check_manual(offer: Offer) -> Result<(), &'static str> {
    match offer {
        Offer::All => Ok(()),
        Offer::Hidden => Err("the run types into a shell that reads command lines"),
    }
}

/// Refuses a manual answer unless `foreground`, the process group in the terminal's
/// foreground now, is `looked`, the group that the last look saw while the command
/// ran. A group that appeared after that look, such as a precmd hook's command once the
/// command ended, does not read the answer, and the line editor would run it.
pub(crate) fn check_looked(looked: Option<u32>, foreground: u32) -> Result<(), &'static str> {
    match looked {
        None => Err("no look has seen the command's job yet"),
        Some(group) if group != foreground => {
            Err("the job that the last look saw no longer holds the terminal")
        }
        Some(_) => Ok(()),
    }
}

/// Refuses a hidden answer for a visible wait and a visible one for a hidden wait. The
/// modes alone let both through: a visible answer takes any modes, and a program that
/// asked a visible question can turn echo off before the answer comes.
pub(crate) fn check_kind(waiting: Waiting, hidden: bool) -> Result<(), &'static str> {
    match (waiting.hidden, hidden) {
        (true, false) => Err("the command waits for hidden input"),
        (false, true) => Err("the command waits for visible input"),
        _ => Ok(()),
    }
}

/// A wait that a run reported, as an answer is checked against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Waiting {
    /// The process group of the job in the terminal's foreground at the look.
    pub(crate) group: u32,
    /// True for [`InputWait::Hidden`].
    pub(crate) hidden: bool,
}

/// What the session knows about a run when it is asked to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probe {
    /// True between the command's start and its end.
    pub(crate) running: bool,
    /// When the shell last printed anything.
    pub(crate) last_output: Option<Timestamp>,
    /// The job in the terminal's foreground with its input modes; `None` while the
    /// shell itself holds the terminal, and when the group or the modes could not be
    /// read.
    pub(crate) job: Option<Job>,
    /// How many answers were typed for this run.
    pub(crate) answers: u64,
}

/// The quiet times of a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Quiet {
    pub(crate) hidden: Duration,
    pub(crate) visible: Duration,
}

/// Which waits a run reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Offer {
    /// Hidden and visible waits.
    All,
    /// Hidden waits only: the run leaves a shell or a REPL at its prompt, which looks
    /// like a visible question.
    Hidden,
}

impl Offer {
    /// The waits that a run in `mode` of `command` reports.
    pub(crate) fn of(mode: RunMode, command: &str) -> Self {
        match mode {
            RunMode::Sentinel => Offer::Hidden,
            RunMode::Auto if nested_shell::starts_shell(command) => Offer::Hidden,
            RunMode::Auto => Offer::All,
        }
    }
}

/// What a look found before the screen is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    Settled(InputWait),
    /// Visible if the screen shows a prompt, otherwise none.
    ReadScreen,
}

/// Judges a probe at `now` for a run that reports the waits in `offer`. The screen is
/// read only when the output is quiet, the modes are not a getpass-style read's and the
/// run reports visible waits.
pub(crate) fn look(probe: &Probe, now: Timestamp, quiet: Quiet, offer: Offer) -> Look {
    let Some(Job { modes, .. }) = probe.job.filter(|_| probe.running) else {
        return Look::Settled(InputWait::None);
    };
    let silence = probe.last_output.map(|at| Duration::try_from(now.duration_since(at)));
    let quiet_for = |needed: Duration| match silence {
        None => true,
        Some(Ok(silence)) => silence >= needed,
        // A clock that went back counts as no time passed.
        Some(Err(_)) => false,
    };
    if modes.hidden() {
        return Look::Settled(if quiet_for(quiet.hidden) {
            InputWait::Hidden
        } else {
            InputWait::None
        });
    }
    if offer == Offer::All && quiet_for(quiet.visible) {
        return Look::ReadScreen;
    }
    Look::Settled(InputWait::None)
}

/// True when the main screen shows a prompt: the cursor sits after some text.
pub(crate) fn visible_prompt(snapshot: &ScreenSnapshot) -> bool {
    !snapshot.alternate_screen && cursor_after_text(snapshot)
}

/// What a prompt that asks for a secret says, in lower case. `pin` counts only as a
/// word of its own, so `ping` or `spinning` do not.
const SECRET_WORDS: &[&str] =
    &["password", "passphrase", "passcode", "verification code", "one-time code", "one time code"];

/// True when the cursor's row of `snapshot` reads like a prompt for a secret.
pub(crate) fn secret_prompt(snapshot: &ScreenSnapshot) -> bool {
    snapshot
        .rows
        .get(usize::from(snapshot.cursor.row))
        .is_some_and(|row| looks_secret(&row_text(row)))
}

/// True when `line` reads like a prompt for a secret: it names a password, a
/// passphrase, a passcode, a PIN, a verification code or a one-time code, in any case.
/// A guess from the text that a program printed, so it only changes what a client
/// shows, never where an answer goes.
pub(crate) fn looks_secret(line: &str) -> bool {
    let line = line.to_lowercase();
    let starts_word =
        |at: usize| !line[..at].chars().next_back().is_some_and(char::is_alphanumeric);
    let ends_word = |at: usize| !line[at..].chars().next().is_some_and(char::is_alphanumeric);
    let found = |word: &str, whole: bool| {
        line.match_indices(word)
            .any(|(at, _)| starts_word(at) && (!whole || ends_word(at + word.len())))
    };
    SECRET_WORDS.iter().any(|word| found(word, false)) || found("pin", true)
}

/// The wait a run reported last, so each change is reported once, and the job it
/// belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InputWatch {
    current: InputWait,
    /// The wait reported last looks like a prompt for a secret.
    secret: bool,
    /// The process group in the terminal's foreground at the last look.
    group: Option<u32>,
    answers: u64,
}

impl InputWatch {
    /// The wait reported last.
    pub(crate) fn current(&self) -> InputWait {
        self.current
    }

    /// True when the wait reported last is a visible one that looks like a prompt for a
    /// secret.
    pub(crate) fn looks_secret(&self) -> bool {
        self.secret
    }

    /// The wait reported last with the process group of its job, as it was at that
    /// look; `None` while no wait is reported.
    pub(crate) fn waiting(&self) -> Option<Waiting> {
        let hidden = match self.current {
            InputWait::Hidden => true,
            InputWait::Visible => false,
            _ => return None,
        };
        self.group.map(|group| Waiting { group, hidden })
    }

    /// Takes the wait of a look that saw `answers` answers and the job of process group
    /// `group` in the foreground, with whether it looks like a prompt for a secret;
    /// returns it when it or that flag differs from the last one.
    ///
    /// A reported wait ends at a look that finds another job in the foreground, which
    /// reports `None`; a wait of that job is a new change at the look after. A wait
    /// without a job is no wait: an answer to it could reach nothing.
    pub(crate) fn settle(
        &mut self,
        wait: InputWait,
        secret: bool,
        group: Option<u32>,
        answers: u64,
    ) -> Option<InputWait> {
        let other_job = self.current != InputWait::None && group != self.group;
        let wait = if answers == self.answers && !other_job && group.is_some() {
            wait
        } else {
            InputWait::None
        };
        self.answers = answers;
        self.group = group;
        self.change(wait, secret && wait == InputWait::Visible)
    }

    /// The run ended or was left: `None`, when that is a change.
    pub(crate) fn end(&mut self) -> Option<InputWait> {
        self.change(InputWait::None, false)
    }

    fn change(&mut self, wait: InputWait, secret: bool) -> Option<InputWait> {
        if wait == self.current && secret == self.secret {
            return None;
        }
        self.current = wait;
        self.secret = secret;
        Some(wait)
    }
}

#[cfg(test)]
mod tests;
