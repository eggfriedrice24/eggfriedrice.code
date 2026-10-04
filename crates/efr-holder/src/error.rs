//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_protocol::{PtyId, Size};

use crate::Signal;

/// Every way a holder operation can fail.
///
/// A holder that receives a malformed [`SpawnSpec`](crate::SpawnSpec) reports it with one
/// of the spec variants before it opens anything, so the caller learns which field to
/// fix. No variant carries an environment value: the hidden shell's environment is
/// copied from the user's, which can hold tokens.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HolderError {
    /// The program to run is not an absolute path. A holder never searches `PATH`,
    /// because the daemon and `efr-ptyd` would search different ones.
    #[error("the program {} is not an absolute path", .program.display())]
    ProgramNotAbsolute {
        /// The program as the spec names it.
        program: PathBuf,
    },

    /// The working directory is not an absolute path.
    #[error("the working directory {} is not an absolute path", .cwd.display())]
    CwdNotAbsolute {
        /// The directory as the spec names it.
        cwd: PathBuf,
    },

    /// The terminal size has no cells, which the kernel accepts but no shell can use.
    #[error("the terminal size {}x{} has no cells", .size.cols, .size.rows)]
    EmptySize {
        /// The size as the spec names it.
        size: Size,
    },

    /// An environment variable name is empty or contains `=` or a NUL byte, so it cannot
    /// reach the child as written.
    #[error("{name:?} is not a valid environment variable name")]
    InvalidEnvName {
        /// The rejected name.
        name: String,
    },

    /// An environment variable value contains a NUL byte. Only the name is kept.
    #[error("the value of the environment variable {name} contains a NUL byte")]
    NulInEnvValue {
        /// The variable's name.
        name: String,
    },

    /// The program path, the working directory or an argument contains a NUL byte,
    /// which `execve` cannot pass.
    #[error("the {field} of the spawn spec contains a NUL byte")]
    NulByte {
        /// Which part: `"program"`, `"cwd"` or `"argument"`.
        field: &'static str,
    },

    /// The holder already holds a PTY with this id. A spawn retried after a lost reply
    /// gets this instead of a second shell.
    #[error("the holder already holds the PTY {pty_id}")]
    AlreadyExists {
        /// The id in the spec.
        pty_id: PtyId,
    },

    /// The holder holds no PTY with this id, or it was released.
    #[error("the holder holds no PTY {pty_id}")]
    NotFound {
        /// The id asked for.
        pty_id: PtyId,
    },

    /// The operation needs a running child, and the PTY's child has exited.
    #[error("the child of the PTY {pty_id} has exited")]
    Exited {
        /// The PTY.
        pty_id: PtyId,
    },

    /// Opening the PTY or starting the program failed.
    #[error("could not start {} on a new PTY", .program.display())]
    Spawn {
        /// The program of the spec.
        program: PathBuf,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// Setting the terminal size failed.
    #[error("could not resize the PTY {pty_id}")]
    Resize {
        /// The PTY.
        pty_id: PtyId,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// Delivering a signal failed.
    #[error("could not send {signal} to the PTY {pty_id}")]
    Signal {
        /// The PTY.
        pty_id: PtyId,
        /// The signal.
        signal: Signal,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// Releasing a PTY failed.
    #[error("could not release the PTY {pty_id}")]
    Release {
        /// The PTY.
        pty_id: PtyId,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests;
