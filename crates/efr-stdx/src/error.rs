//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crate::env::Var;

/// Every way an `efr-stdx` operation can fail.
///
/// Variants carry the data a caller needs to decide what to do (the variable, the
/// path, the duration). The message names what failed in one sentence and leaves the
/// source's text to the `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StdxError {
    /// The home directory is unknown, so the XDG base directories cannot be found.
    #[error("the home directory could not be found")]
    HomeNotFound,

    /// `XDG_RUNTIME_DIR` is unset, `/run/user/<uid>` is not a usable directory, and
    /// neither `EFR_RUNTIME_DIR` nor `EFR_HOME` replaces them.
    #[error(
        "XDG_RUNTIME_DIR is not set, /run/user/<uid> is not usable, and neither EFR_RUNTIME_DIR nor EFR_HOME replaces them"
    )]
    RuntimeDirUnset,

    /// The daemon's socket path is longer than a Unix socket address holds.
    #[error(
        "the socket path {} is {} bytes, more than the {} a Unix socket holds",
        .path.display(),
        .path.as_os_str().len(),
        crate::paths::MAX_SOCKET_PATH
    )]
    SocketPathTooLong {
        /// The socket path.
        path: PathBuf,
    },

    /// A path that must be absolute is relative.
    #[error("{} is not an absolute path", .path.display())]
    NotAbsolute {
        /// The relative path.
        path: PathBuf,
    },

    /// An environment variable holds bytes that are not valid UTF-8.
    #[error("{var} is not valid UTF-8")]
    NotUnicode {
        /// The variable.
        var: Var,
    },

    /// A path variable holds a relative path.
    #[error("{var} must hold an absolute path, not {}", .path.display())]
    RelativeEnvPath {
        /// The variable.
        var: Var,
        /// The relative path it holds.
        path: PathBuf,
    },

    /// A variable holds a value that does not parse as the expected type.
    #[error("{var} holds {value:?}, which is not {expected}")]
    InvalidEnvValue {
        /// The variable.
        var: Var,
        /// The value it holds.
        value: String,
        /// What the value must be.
        expected: &'static str,
    },

    /// A [`Clock::timeout`](crate::time::Clock::timeout) reached its deadline before
    /// the future finished.
    #[error("the operation did not finish within {after:?}")]
    TimedOut {
        /// The duration that passed.
        after: Duration,
    },

    /// The operating system's random source could not seed a generator.
    #[error("the system random number generator could not be seeded")]
    SeedRng {
        /// The error from the random source.
        #[source]
        source: io::Error,
    },

    /// A path that must name a file ends in `/` or `..`.
    #[error("{} does not name a file", .path.display())]
    NoFileName {
        /// The path.
        path: PathBuf,
    },

    /// A file could not be created.
    #[error("could not create {}", .path.display())]
    CreateFile {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A file could not be written or flushed to disk.
    #[error("could not write {}", .path.display())]
    WriteFile {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A file could not be renamed into place.
    #[error("could not rename {} to {}", .from.display(), .to.display())]
    Rename {
        /// The file that was to be renamed.
        from: PathBuf,
        /// The path it was to get.
        to: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A directory could not be flushed to disk after a rename in it.
    #[error("could not flush the directory {} to disk", .path.display())]
    SyncDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A directory could not be created.
    #[error("could not create the directory {}", .path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A thread name holds a NUL byte, which the operating system cannot store.
    #[error("the thread name {name:?} holds a NUL byte")]
    InvalidThreadName {
        /// The name.
        name: String,
    },

    /// The operating system refused to start a thread.
    #[error("could not start the thread {name}")]
    SpawnThread {
        /// The name of the thread.
        name: String,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests;
