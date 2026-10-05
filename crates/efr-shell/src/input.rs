//! Whether a running command waits for input, and what an answer may be.
//!
//! A run looks once per `quiet_period` while its command runs (between `C` and `D`, or
//! the two sentinels):
//!
//! - [`InputWait::Hidden`]: the terminal has echo off and canonical input on, as a
//!   getpass-style read leaves it, and the output has been quiet for `quiet_period`.
//! - [`InputWait::Visible`]: echo and canonical input are on, the output has been quiet
//!   for `visible_input_quiet`, and the main screen has the cursor after some text, as
//!   after `[Y/n] `. Only then is the screen read. A command whose last line is merely
//!   unfinished looks the same, so this is a guess, and it never stops a command.
//! - [`InputWait::None`] otherwise, and when the modes cannot be read. A full-screen
//!   program on the alternate screen is judged only at the run's timeout, as before.
//!
//! An answer resets the wait to `None` for one look, so the same prompt asked again
//! (`Sorry, try again.`) is a new change that a client asks the user about again.

use std::time::Duration;

use efr_protocol::{InputWait, ScreenSnapshot};
use jiff::Timestamp;

use crate::ShellError;
use crate::modes::InputModes;
use crate::run::cursor_after_text;

/// The longest answer, in bytes, without the carriage return that ends it.
pub(crate) const MAX_ANSWER: usize = 1024;

/// Refuses an answer that is not one line of at most [`MAX_ANSWER`] bytes. The reason
/// never quotes the text, which can be a password.
pub(crate) fn check_answer(text: &str) -> Result<(), ShellError> {
    if text.len() > MAX_ANSWER {
        return Err(ShellError::InvalidAnswer { reason: "it is longer than 1024 bytes" });
    }
    if text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err(ShellError::InvalidAnswer { reason: "it contains a control character" });
    }
    Ok(())
}

/// Refuses to type an answer into a terminal in `modes`. Both kinds need canonical
/// input: a program that reads a line has it, and the shell's own line editor does
/// not, so an answer that comes just after the command ended can never run as a
/// command line. A hidden answer also needs echo off, so it never reaches the output.
pub(crate) fn check_modes(modes: InputModes, hidden: bool) -> Result<(), &'static str> {
    if !modes.canonical {
        return Err("the terminal is not reading a line");
    }
    if hidden && modes.echo {
        return Err("the terminal echoes what is typed");
    }
    Ok(())
}

/// What the session knows about a run when it is asked to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probe {
    /// True between the command's start and its end.
    pub(crate) running: bool,
    /// When the shell last printed anything.
    pub(crate) last_output: Option<Timestamp>,
    /// The terminal's input modes; `None` when they could not be read.
    pub(crate) modes: Option<InputModes>,
    /// How many answers were typed for this run.
    pub(crate) answers: u64,
}

/// The quiet times of a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Quiet {
    pub(crate) hidden: Duration,
    pub(crate) visible: Duration,
}

/// What a look found before the screen is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    Settled(InputWait),
    /// Visible if the screen shows a prompt, otherwise none.
    ReadScreen,
}

/// Judges a probe at `now`. The screen is read only when the output is quiet and the
/// modes allow a visible prompt.
pub(crate) fn look(probe: &Probe, now: Timestamp, quiet: Quiet) -> Look {
    let Some(modes) = probe.modes.filter(|_| probe.running) else {
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
    if modes.echo && modes.canonical && quiet_for(quiet.visible) {
        return Look::ReadScreen;
    }
    Look::Settled(InputWait::None)
}

/// True when the main screen shows a prompt: the cursor sits after some text.
pub(crate) fn visible_prompt(snapshot: &ScreenSnapshot) -> bool {
    !snapshot.alternate_screen && cursor_after_text(snapshot)
}

/// The wait a run reported last, so each change is reported once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InputWatch {
    current: InputWait,
    answers: u64,
}

impl InputWatch {
    /// The wait reported last.
    pub(crate) fn current(&self) -> InputWait {
        self.current
    }

    /// Takes the wait of a look that saw `answers` answers; returns it when it differs
    /// from the last one.
    pub(crate) fn settle(&mut self, wait: InputWait, answers: u64) -> Option<InputWait> {
        let wait = if answers == self.answers { wait } else { InputWait::None };
        self.answers = answers;
        self.change(wait)
    }

    /// The run ended or was left: `None`, when that is a change.
    pub(crate) fn end(&mut self) -> Option<InputWait> {
        self.change(InputWait::None)
    }

    fn change(&mut self, wait: InputWait) -> Option<InputWait> {
        if wait == self.current {
            return None;
        }
        self.current = wait;
        Some(wait)
    }
}

#[cfg(test)]
mod tests;
