//! The one public error type of the crate.

use std::path::PathBuf;

/// Every way that building the engine's inputs can fail.
///
/// Deciding itself never fails: a requirement that the engine cannot judge, such as a
/// relative path, is denied with a reason, not returned as an error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PermissionsError {
    /// A location that paths are classified by is not an absolute path.
    #[error("{} is not an absolute path", .path.display())]
    NotAbsolute {
        /// The relative path.
        path: PathBuf,
    },

    /// The home directory is `/`, so user paths cannot be told apart from system paths.
    #[error("the home directory is /, so user paths cannot be told apart from system paths")]
    HomeIsRoot,
}
