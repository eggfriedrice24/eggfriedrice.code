//! The progress bar of the terminal's tab (OSC 9;4) while a turn runs.
//!
//! Ghostty 1.2 and later, kitty 0.47 and later and Windows Terminal draw it. Other
//! terminals read OSC 9 as a desktop notification, so `9;4;3` would pop up a note that
//! reads "4;3". `render.progress = "auto"` therefore sends it only to the terminals
//! that are known to draw it, and never through tmux, which may pass it on to anything.
//! No query decides it: a query needs a reply on stdin, which would take the keys that
//! the user typed ahead for the shell.
//!
//! The view sends the sequences; this module only names them and decides whether they
//! go out at all.

use efr_config::Progress;

use crate::terminal::TermFacts;

/// An indeterminate bar: the turn runs. Ghostty moves it one step per sequence, so the
/// view sends it again on every tick.
pub(crate) const RUNNING: &str = "\x1b]9;4;3\x1b\\";

/// A paused bar: the turn waits for the user.
pub(crate) const PAUSED: &str = "\x1b]9;4;4\x1b\\";

/// No bar: the turn is over, or efr stops following it.
pub(crate) const CLEAR: &str = "\x1b]9;4;0\x1b\\";

/// A full bar in the error state: the turn failed. The terminal hides it after a time.
pub(crate) const FAILED: &str = "\x1b]9;4;2;100\x1b\\";

/// The first `TERM_PROGRAM_VERSION` of Ghostty that draws the bar.
const GHOSTTY: [u64; 3] = [1, 2, 0];

/// The first `TERM_PROGRAM_VERSION` of kitty that draws the bar.
const KITTY: [u64; 3] = [0, 47, 0];

/// True when the bar goes out: `on` and `auto` only when replies are formatted (stdout
/// is a terminal), `auto` only in a terminal that is known to draw it and not in tmux.
pub(crate) fn wanted(setting: Progress, facts: &TermFacts) -> bool {
    if !facts.formats_stdout() {
        return false;
    }
    match setting {
        Progress::On => true,
        Progress::Auto => !facts.tmux && draws_it(facts),
        _ => false,
    }
}

/// True for a terminal that is known to draw the bar.
fn draws_it(facts: &TermFacts) -> bool {
    if facts.wt_session {
        return true;
    }
    let version = facts.term_program_version.as_deref().map(version);
    match (facts.term_program.as_deref(), version) {
        (Some(program), Some(version)) if program.eq_ignore_ascii_case("ghostty") => {
            version >= GHOSTTY
        }
        (Some(program), Some(version)) if program.eq_ignore_ascii_case("kitty") => version >= KITTY,
        _ => false,
    }
}

/// The first three numbers of a version such as `1.3.1` or `1.2.0-dev+abc`; a missing
/// or unreadable part counts as 0.
fn version(text: &str) -> [u64; 3] {
    let mut parts = text
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().ok())
        .chain(std::iter::repeat(None));
    let mut numbers = [0; 3];
    for number in &mut numbers {
        match parts.next().flatten() {
            Some(value) => *number = value,
            None => break,
        }
    }
    numbers
}

#[cfg(test)]
mod tests;
