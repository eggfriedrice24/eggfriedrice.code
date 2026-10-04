//! The one public error type of the crate.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use efr_stdx::StdxError;

use crate::RegistryProblem;

/// Every way that scope derivation and the project registry can fail.
///
/// A directory that is in no git repository is not an error, and neither is a
/// repository that git refuses to open: both make the scope `Machine`. Errors are
/// failures to look at all, and the caller falls back to `Machine`, the scope that
/// widens nothing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScopeError {
    /// A path that must be absolute, such as the working directory or the home
    /// directory, is relative.
    #[error("{} is not an absolute path", .path.display())]
    NotAbsolute {
        /// The relative path.
        path: PathBuf,
    },

    /// The home directory is `/`, so the user's files cannot be told apart from the
    /// machine's.
    #[error("the home directory is /")]
    HomeIsRoot,

    /// The registry file exists but could not be read.
    #[error("could not read the project registry {}", .path.display())]
    ReadRegistry {
        /// The registry file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// The registry file is not valid TOML of the registry's shape.
    #[error("could not parse the project registry {}", .path.display())]
    ParseRegistry {
        /// The registry file.
        path: PathBuf,
        /// The error from the TOML parser, with the position.
        #[source]
        source: toml::de::Error,
    },

    /// The registry file parses but breaks a rule of the registry.
    #[error("the project registry {} is invalid: {problem}", .path.display())]
    InvalidRegistry {
        /// The registry file.
        path: PathBuf,
        /// The broken rule.
        problem: RegistryProblem,
    },

    /// A project cannot be added to the registry.
    #[error("the project cannot be registered: {problem}")]
    InvalidProject {
        /// The broken rule.
        problem: RegistryProblem,
    },

    /// The registry could not be written as TOML.
    #[error("could not serialize the project registry")]
    SerializeRegistry {
        /// The error from the TOML writer.
        #[source]
        source: toml::ser::Error,
    },

    /// The directory that holds the registry file could not be created.
    #[error("could not create the directory {}", .path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// The registry file could not be written.
    #[error("could not write the project registry {}", .path.display())]
    WriteRegistry {
        /// The registry file.
        path: PathBuf,
        /// The error from the atomic write.
        #[source]
        source: StdxError,
    },

    /// git could not be started, for example because it is not installed.
    #[error("could not run {}", .program.to_string_lossy())]
    RunGit {
        /// The program that was started, normally `git`.
        program: OsString,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// git did not finish in time, for example on a hung network file system.
    #[error("git did not finish within {after:?}")]
    GitTimedOut {
        /// The timeout.
        after: Duration,
    },

    /// A directory could not be listed.
    #[error("could not list the directory {}", .path.display())]
    ReadDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },
}
