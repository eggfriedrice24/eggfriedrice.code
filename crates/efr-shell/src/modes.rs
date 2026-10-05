//! The input modes of a hidden shell's terminal, read from the PTY master, and the one
//! write that types an answer for a command that waits for input.
//!
//! On Linux `tcgetattr` on a PTY master returns the termios of its slave, which is what
//! the program in the foreground set: a getpass-style read (`sudo`, `ssh`, `su`,
//! `passwd`) turns echo off and keeps canonical line input, and the line editor of the
//! shell itself turns both off.

use std::fmt;
use std::io;
use std::os::fd::{AsFd as _, BorrowedFd};
use std::sync::Arc;

use rustix::termios::LocalModes;

use crate::reader::Master;

/// What a terminal does with the input it gets, as far as answering a prompt goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct InputModes {
    /// The terminal echoes what is typed (`ECHO`), so typed text reaches the output.
    pub echo: bool,
    /// The terminal hands input over a line at a time (`ICANON`), so a program reads
    /// what is typed only once it ends with a newline.
    pub canonical: bool,
}

impl InputModes {
    /// The modes with `echo` and `canonical` as given.
    pub const fn new(echo: bool, canonical: bool) -> Self {
        InputModes { echo, canonical }
    }

    /// True for a getpass-style read: echo off, canonical input on.
    pub fn hidden(self) -> bool {
        !self.echo && self.canonical
    }
}

/// Reads the input modes of a PTY from its master.
///
/// [`Termios`] is the real one. The shell manager calls it from a synchronous step that
/// also writes, so it must not block.
pub trait TerminalModes: Send + Sync + fmt::Debug {
    /// The modes of the terminal whose master is `master`.
    fn read(&self, master: BorrowedFd<'_>) -> io::Result<InputModes>;
}

/// [`TerminalModes`] through `tcgetattr` on the master.
#[derive(Debug, Clone, Copy, Default)]
pub struct Termios;

impl TerminalModes for Termios {
    fn read(&self, master: BorrowedFd<'_>) -> io::Result<InputModes> {
        let termios = rustix::termios::tcgetattr(master)?;
        Ok(InputModes {
            echo: termios.local_modes.contains(LocalModes::ECHO),
            canonical: termios.local_modes.contains(LocalModes::ICANON),
        })
    }
}

/// The master of one shell with the reader of its modes: what the session's actor
/// needs to check a terminal and type an answer in one step.
#[derive(Debug, Clone)]
pub(crate) struct Terminal {
    master: Master,
    modes: Arc<dyn TerminalModes>,
}

impl Terminal {
    pub(crate) fn new(master: Master, modes: Arc<dyn TerminalModes>) -> Self {
        Terminal { master, modes }
    }

    /// The terminal's input modes now.
    pub(crate) fn modes(&self) -> io::Result<InputModes> {
        self.modes.read(self.master.get_ref().as_fd())
    }

    /// Writes `text` and a carriage return in one system call, as Enter would end the
    /// line. A write that the PTY takes only in part is an error: the master is
    /// non-blocking, and an answer is never finished later by another write.
    pub(crate) fn write_line(&self, text: &str) -> io::Result<()> {
        let slices = [io::IoSlice::new(text.as_bytes()), io::IoSlice::new(b"\r")];
        let total = text.len() + 1;
        loop {
            match rustix::io::writev(self.master.get_ref(), &slices) {
                Ok(written) if written == total => return Ok(()),
                Ok(_) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
                // Nothing was written when the call was interrupted, so it is tried again.
                Err(rustix::io::Errno::INTR) => {}
                Err(errno) => return Err(io::Error::from(errno)),
            }
        }
    }
}

#[cfg(test)]
mod tests;
