//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

/// The step of a spawn that failed before the child could start.
///
/// The [`PtyHolder`](efr_holder::PtyHolder) methods return
/// [`HolderError`](efr_holder::HolderError), because the trait fixes that type. A failed
/// spawn is [`HolderError::Spawn`](efr_holder::HolderError::Spawn), whose `source` is an
/// [`io::Error`] with the same [`io::ErrorKind`] as the system error; when the failure
/// happened while the PTY was set up, that `io::Error` wraps a `PtyError` that names
/// the step, so a log line says whether `/dev/pts` or the program was the problem. A
/// caller that needs the step downcasts the `source` with
/// [`io::Error::get_ref`]. A program that cannot be started is not a `PtyError`: its
/// `source` is the system error of `execve` itself.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PtyError {
    /// `posix_openpt` failed: no PTY master could be opened.
    #[error("could not open a PTY master")]
    OpenMaster {
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// `grantpt` failed on the new master.
    #[error("could not grant access to the slave side of the PTY")]
    GrantSlave {
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// `unlockpt` failed on the new master.
    #[error("could not unlock the slave side of the PTY")]
    UnlockSlave {
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// `ptsname` failed on the new master.
    #[error("could not find the name of the slave side of the PTY")]
    SlaveName {
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// The slave device could not be opened.
    #[error("could not open the PTY slave {}", .path.display())]
    OpenSlave {
        /// The slave device, such as `/dev/pts/3`.
        path: PathBuf,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// The window size or the input modes of the new terminal could not be set.
    #[error("could not set up the terminal of the new PTY")]
    ConfigureTerminal {
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// The spawn was not called inside a tokio runtime, which the holder needs to reap
    /// the child.
    #[error("the PTY holder needs a tokio runtime to reap its children")]
    NoRuntime,

    /// The child started, but tokio reported no process id for it.
    #[error("the new child process has no process id")]
    MissingChildPid,
}

impl PtyError {
    /// The kind of the underlying system error, or [`io::ErrorKind::Other`] when there
    /// is none.
    fn kind(&self) -> io::ErrorKind {
        match self {
            PtyError::OpenMaster { source }
            | PtyError::GrantSlave { source }
            | PtyError::UnlockSlave { source }
            | PtyError::SlaveName { source }
            | PtyError::OpenSlave { source, .. }
            | PtyError::ConfigureTerminal { source } => source.kind(),
            PtyError::NoRuntime | PtyError::MissingChildPid => io::ErrorKind::Other,
        }
    }

    /// Wraps the error in an [`io::Error`] of the same kind, the form
    /// `HolderError::Spawn` carries.
    pub(crate) fn into_io(self) -> io::Error {
        io::Error::new(self.kind(), self)
    }
}

#[cfg(test)]
mod tests;
