//! The one public error type of the crate.

/// Every way an `efr-stdx` operation can fail.
///
/// Variants carry the data a caller needs to decide what to do (the variable, the
/// path, the duration). The message names what failed in one sentence and leaves the
/// source's text to the `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StdxError {}
