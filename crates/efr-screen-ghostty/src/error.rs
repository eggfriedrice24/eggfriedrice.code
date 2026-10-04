//! The one public error type of the crate.

/// Every way an `efr-screen-ghostty` operation can fail.
///
/// libghostty-vt reports failures as a small closed set (out of memory, an invalid
/// value, a failed write or read, a limit). `vt_write` itself never fails: malformed
/// input only ever changes the screen, so feeding bytes has no error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GhosttyError {
    /// libghostty-vt could not create a terminal of this size.
    #[error("could not create a {cols}x{rows} ghostty terminal")]
    Create {
        /// The columns asked for.
        cols: u16,
        /// The rows asked for.
        rows: u16,
        /// The error from libghostty-vt.
        #[source]
        source: libghostty_vt::Error,
    },

    /// libghostty-vt refused a setting: an effect callback, a colour, the scrollback
    /// limit or continuation tracking.
    #[error("could not configure the ghostty terminal")]
    Configure {
        /// The error from libghostty-vt.
        #[source]
        source: libghostty_vt::Error,
    },

    /// The terminal state could not be written as a GHOSTSNP snapshot. This happens
    /// while the parser is inside an escape sequence longer than the continuation
    /// limit; a later attempt succeeds once the sequence has ended.
    #[error("could not encode the ghostty screen as a GHOSTSNP snapshot")]
    EncodeSnapshot {
        /// The error from libghostty-vt.
        #[source]
        source: libghostty_vt::Error,
    },

    /// The bytes are not a complete, intact GHOSTSNP snapshot that this libghostty-vt
    /// can read.
    #[error("could not decode a GHOSTSNP snapshot")]
    DecodeSnapshot {
        /// The error from libghostty-vt.
        #[source]
        source: libghostty_vt::Error,
    },
}

#[cfg(test)]
mod tests;
