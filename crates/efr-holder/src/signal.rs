//! The signals a holder delivers, and to whom.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A signal the daemon may ask a holder to deliver.
///
/// The set is closed and small on purpose: these are the signals a hidden shell's
/// lifecycle needs (interrupt a command, end the shell politely, end it by force). The
/// holder maps each one to the platform's number, so the contract carries no numbers.
/// A resize needs no signal here: the kernel sends `SIGWINCH` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Signal {
    /// `SIGHUP`: the terminal went away.
    Hangup,
    /// `SIGINT`: what Ctrl+C sends.
    Interrupt,
    /// `SIGQUIT`: what Ctrl+\ sends.
    Quit,
    /// `SIGTERM`: please exit.
    Terminate,
    /// `SIGKILL`: exit now; cannot be caught.
    Kill,
}

impl fmt::Display for Signal {
    /// The conventional name, such as `SIGHUP`, for logs and error messages.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Signal::Hangup => "SIGHUP",
            Signal::Interrupt => "SIGINT",
            Signal::Quit => "SIGQUIT",
            Signal::Terminate => "SIGTERM",
            Signal::Kill => "SIGKILL",
        })
    }
}

/// Who receives a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SignalTarget {
    /// The process the holder spawned: the shell itself.
    Child,
    /// The foreground process group of the PTY: the command the shell is running, or
    /// the shell when it sits at its prompt. This is where the terminal itself would
    /// send Ctrl+C.
    ForegroundGroup,
}

#[cfg(test)]
mod tests;
