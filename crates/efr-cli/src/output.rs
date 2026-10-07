//! The only module that writes to stdout and stderr. Every other module builds text
//! and hands it here.
//!
//! It writes through `std::io::Write` instead of the print macros, so the print lints
//! need no exception: `println!` panics when the reader of a pipe goes away
//! (`efr history | head`), and a write returns the error, which ends the command
//! quietly instead.
//!
//! While a turn runs, the view hides the cursor and may show a progress bar in the
//! terminal's tab. [`set_restore`] keeps the bytes that undo that, and
//! [`restore_terminal`] writes them on a way out that skips the view: a panic (the hook
//! of [`install_panic_hook`]) or the default action of SIGQUIT.

use std::fmt;
use std::io::{self, Write};
use std::sync::{Mutex, PoisonError};

use crate::error::CliError;

/// Where `efr` writes: the process's stdout and stderr, or buffers in tests.
pub(crate) struct Output {
    stdout: Box<dyn Write + Send>,
    stderr: Box<dyn Write + Send>,
}

impl fmt::Debug for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Output").finish_non_exhaustive()
    }
}

impl Output {
    /// The process's stdout and stderr.
    pub(crate) fn process() -> Output {
        Output { stdout: Box::new(io::stdout()), stderr: Box::new(io::stderr()) }
    }

    /// Other writers in place of stdout and stderr.
    #[cfg(test)]
    pub(crate) fn from_writers(
        stdout: Box<dyn Write + Send>,
        stderr: Box<dyn Write + Send>,
    ) -> Output {
        Output { stdout, stderr }
    }

    /// Writes `text` to stdout and flushes it, so a redraw of the live zone reaches the
    /// terminal at once instead of waiting in a line buffer.
    pub(crate) fn out(&mut self, text: &str) -> Result<(), CliError> {
        if text.is_empty() {
            return Ok(());
        }
        self.stdout
            .write_all(text.as_bytes())
            .and_then(|()| self.stdout.flush())
            .map_err(|source| CliError::Output { source })
    }

    /// Writes `text` to stderr. A failure is dropped: stderr is where failures are
    /// reported, so there is nowhere left to report this one.
    pub(crate) fn err(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let written = self.stderr.write_all(text.as_bytes()).and_then(|()| self.stderr.flush());
        drop(written);
    }
}

/// What [`restore_terminal`] writes: the bytes that undo what the view did to the
/// terminal, such as `CSI ? 25 h` for a hidden cursor.
static RESTORE: Mutex<String> = Mutex::new(String::new());

/// Keeps `text` as what a sudden way out must write to stdout to leave the terminal as
/// it was; empty when nothing needs to be undone.
pub(crate) fn set_restore(text: &str) {
    let mut restore = RESTORE.lock().unwrap_or_else(PoisonError::into_inner);
    if *restore != text {
        text.clone_into(&mut restore);
    }
}

/// Takes what [`set_restore`] kept, so it is written once.
fn take_restore() -> String {
    std::mem::take(&mut *RESTORE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Writes what [`set_restore`] kept to stdout, once. A failure is dropped: the process
/// is on its way out.
pub(crate) fn restore_terminal() {
    let text = take_restore();
    if text.is_empty() {
        return;
    }
    let mut stdout = io::stdout();
    drop(stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush()));
}

/// Makes a panic restore the terminal first, then report as before.
pub(crate) fn install_panic_hook() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        report(info);
    }));
}

#[cfg(test)]
mod tests;
