//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_stdx::StdxError;

/// Every way a test helper can fail.
///
/// Variants carry the data a test needs to see what went wrong (the path, the line).
/// The message names what failed in one sentence and leaves the source's text to the
/// `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TestSupportError {
    /// The temporary directory for a test could not be created.
    #[error("could not create a temporary directory")]
    CreateTempDir {
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// The real path of a temporary directory could not be found.
    #[error("could not resolve the temporary directory {}", .path.display())]
    ResolveTempDir {
        /// The directory as created.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A directory inside the temporary tree could not be created.
    #[error("could not create the directory {}", .path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// The temporary tree does not make valid efr roots.
    #[error("the temporary directories are not valid efr roots")]
    Dirs {
        /// The error from `efr-stdx`.
        #[source]
        source: StdxError,
    },

    /// A fixture was looked up from a source file that is not inside a crate on disk.
    #[error("could not find the crate that holds {}", .source_file.display())]
    NoCrateForSource {
        /// The source file, as `file!()` gave it.
        source_file: PathBuf,
    },
}

#[cfg(test)]
mod tests;
