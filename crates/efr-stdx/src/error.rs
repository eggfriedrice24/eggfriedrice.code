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

    /// `XDG_RUNTIME_DIR` is unset and `EFR_RUNTIME_DIR` does not replace it.
    #[error("XDG_RUNTIME_DIR is not set and EFR_RUNTIME_DIR does not replace it")]
    RuntimeDirUnset,

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
}
