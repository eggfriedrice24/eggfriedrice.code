//! The one public error type of the crate.

use std::path::PathBuf;

use efr_protocol::Size;

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
}

#[cfg(test)]
mod tests;
