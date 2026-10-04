//! The one public error type of the crate.

/// Every way an `efr-render` operation can fail.
///
/// Rendering itself never fails: markdown has no syntax errors, and a grammar that
/// cannot highlight a line falls back to plain text. Only choosing options can.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RenderError {
    /// A theme name matches none of the embedded themes.
    #[error("there is no theme named {name:?}")]
    UnknownTheme {
        /// The name as the caller gave it.
        name: String,
    },
}
