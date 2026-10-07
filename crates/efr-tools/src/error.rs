//! The one public error type of the crate.

use std::error::Error;
use std::io;
use std::path::PathBuf;

use efr_shell::ShellError;
use efr_stdx::StdxError;

/// Every way a tool call can fail before it produces a result.
///
/// The conversation turns an error into a failed tool result for the model, with the
/// `Display` text and the source chain. Expected outcomes the model should act on,
/// such as a command that exits non-zero or a busy shell, are results, not errors.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolError {
    /// No tool is registered under this name.
    #[error("no tool is named {name:?}")]
    UnknownTool {
        /// The name the call used.
        name: String,
    },

    /// A tool with this name is registered already.
    #[error("a tool named {name:?} is registered already")]
    DuplicateTool {
        /// The name.
        name: String,
    },

    /// A tool name must be 1 to 64 ASCII letters, digits, `_`, `-` or `.`.
    #[error("{name:?} is not a valid tool name")]
    InvalidName {
        /// The name.
        name: String,
    },

    /// The call's input does not match the tool's schema.
    #[error("the input of the {tool} tool does not match its schema")]
    InvalidInput {
        /// The tool.
        tool: String,
        /// What did not match.
        #[source]
        source: serde_json::Error,
    },

    /// The shell call's `needs` asks for more than its limits allow.
    #[error("the needs of the shell call ask for more than the tool allows")]
    InvalidNeeds {
        /// Which limit.
        #[source]
        source: efr_protocol::ProtocolError,
    },

    /// The call named an empty path.
    #[error("the path is empty")]
    EmptyPath,

    /// The path goes through a symbolic link. The permission engine judged the path as
    /// written, so the tool works only on the real path; calling the tool again with
    /// `real` lets the engine judge that one.
    #[error("{} goes through a symbolic link; the real path is {}", .path.display(), .real.display())]
    ThroughSymlink {
        /// The path as resolved from the call.
        path: PathBuf,
        /// Where it leads.
        real: PathBuf,
    },

    /// The path is not a regular file.
    #[error("{} is not a regular file", .path.display())]
    NotAFile {
        /// The path.
        path: PathBuf,
    },

    /// The file is larger than the tool handles.
    #[error("{} has {bytes} bytes, more than the {limit} this tool handles", .path.display())]
    TooLarge {
        /// The path.
        path: PathBuf,
        /// Its size.
        bytes: u64,
        /// The limit.
        limit: u64,
    },

    /// The file holds binary data, not text.
    #[error("{} is not a text file", .path.display())]
    NotText {
        /// The path.
        path: PathBuf,
    },

    /// Reading the file or its metadata failed.
    #[error("could not read {}", .path.display())]
    Read {
        /// The path.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// Writing the file failed.
    #[error("could not write {}", .path.display())]
    Write {
        /// The path.
        path: PathBuf,
        /// The error from the write.
        #[source]
        source: StdxError,
    },

    /// The original of a file could not be recorded in the write journal, so the file
    /// was not written.
    #[error("could not record the original of {} before writing it", .path.display())]
    Journal {
        /// The path.
        path: PathBuf,
        /// The journal's error.
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },

    /// The hidden shell failed in a way the model cannot act on.
    #[error("the hidden shell failed")]
    Shell {
        /// The shell's error.
        #[source]
        source: ShellError,
    },

    /// A file operation on the blocking pool did not finish (it panicked or the
    /// runtime is shutting down).
    #[error("a file operation on {} did not finish", .path.display())]
    Interrupted {
        /// The path.
        path: PathBuf,
    },
}

impl ToolError {
    /// A journal failure, for implementations of
    /// [`WriteJournal`](crate::WriteJournal) in other crates.
    pub fn journal(
        path: impl Into<PathBuf>,
        source: impl Into<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        ToolError::Journal { path: path.into(), source: source.into() }
    }
}

#[cfg(test)]
mod tests;
