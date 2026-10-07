//! [`SnapshotError`], the one error type of this crate.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// What went wrong with a snapshot.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SnapshotError {
    /// git could not be started, or its pipes failed.
    #[error("git could not be run")]
    RunGit {
        /// The program.
        program: OsString,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// git did not finish in time.
    #[error("git did not finish within {after:?}")]
    GitTimedOut {
        /// The limit.
        after: Duration,
    },
    /// A git command of the store failed.
    #[error("git {command} failed in the snapshot store {store}")]
    GitFailed {
        /// The git subcommand, such as `write-tree`.
        command: &'static str,
        /// The store's git directory.
        store: PathBuf,
    },
    /// git answered with output that this crate cannot read.
    #[error("git {command} gave output that efr cannot read")]
    BadOutput {
        /// The git subcommand.
        command: &'static str,
    },
    /// A file or directory of the store or of a root could not be read or written.
    #[error("{} could not be read or written", path.display())]
    Io {
        /// The path.
        path: PathBuf,
        /// Why.
        #[source]
        source: io::Error,
    },
    /// A look at the file system did not finish in time, as on a hung network mount.
    #[error("the look at {} did not finish within {after:?}", path.display())]
    InspectTimedOut {
        /// The path.
        path: PathBuf,
        /// The limit.
        after: Duration,
    },
    /// A task on the blocking pool panicked or was cancelled.
    #[error("a snapshot task on the blocking pool did not finish")]
    TaskFailed,
}
