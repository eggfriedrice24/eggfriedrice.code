//! The only module that writes to stdout and stderr. Every other module builds text
//! and hands it here.
//!
//! It writes through `std::io::Write` instead of the print macros, so the print lints
//! need no exception: `println!` panics when the reader of a pipe goes away
//! (`efr history | head`), and a write returns the error, which ends the command
//! quietly instead.

use std::fmt;
use std::io::{self, Write};

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

#[cfg(test)]
mod tests;
