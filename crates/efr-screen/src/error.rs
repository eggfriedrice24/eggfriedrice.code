//! The one public error type of the crate.

use efr_stdx::StdxError;

/// Every way an `efr-screen` operation can fail.
///
/// A screen fails in two places only: its thread does not start, or its actor has
/// stopped (after a shutdown, after every handle was dropped, or because the backend
/// panicked) and can take no more commands.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScreenError {
    /// The operating system refused to start the screen thread, or the name is not a
    /// valid thread name.
    #[error("could not start the screen thread {name}")]
    Spawn {
        /// The name of the screen thread.
        name: String,
        /// The error from the thread builder.
        #[source]
        source: StdxError,
    },

    /// The screen actor has stopped, so the command was not delivered or its answer
    /// never came.
    #[error("the screen {name} has stopped")]
    Closed {
        /// The name of the screen thread.
        name: String,
    },
}

#[cfg(test)]
mod tests;
