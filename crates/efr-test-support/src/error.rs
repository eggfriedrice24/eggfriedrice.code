//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_stdx::StdxError;
use efr_store::StoreError;
use serde_json::Value;

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

    /// A directory to create in the temporary tree is named by a path that could leave
    /// the tree.
    #[error("{} is not a relative path inside the temporary tree", .path.display())]
    OutsideTree {
        /// The path as given.
        path: PathBuf,
    },

    /// The temporary tree does not make valid efr roots.
    #[error("the temporary directories are not valid efr roots")]
    Dirs {
        /// The error from `efr-stdx`.
        #[source]
        source: StdxError,
    },

    /// The in-memory store could not be opened.
    #[error("could not open the in-memory store")]
    OpenStore {
        /// The error from `efr-store`.
        #[source]
        source: StoreError,
    },

    /// A read from the store failed.
    #[error("could not read the store")]
    ReadStore {
        /// The error from `efr-store`.
        #[source]
        source: StoreError,
    },

    /// A transcript file could not be read.
    #[error("could not read the transcript {}", .path.display())]
    ReadTranscript {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A transcript file holds an invalid record.
    #[error("the transcript {} is invalid", .path.display())]
    InvalidTranscript {
        /// The file.
        path: PathBuf,
        /// What is wrong, with its line.
        #[source]
        source: Box<TestSupportError>,
    },

    /// A transcript line is not a JSON object with the record members.
    #[error("line {line} of the transcript is not a valid record")]
    RecordSyntax {
        /// The line, from 1.
        line: usize,
        /// The JSON error, which names an unknown member or a member of the wrong type.
        #[source]
        source: serde_json::Error,
    },

    /// A transcript line breaks the rules of its record kind.
    #[error("line {line} of the transcript is invalid: {problem}")]
    InvalidRecord {
        /// The line, from 1.
        line: usize,
        /// What is wrong.
        problem: &'static str,
    },

    /// A provider record holds something the replay provider cannot read: a request
    /// body that is not an `efr_provider::Request`, or answer data that is not JSON, a
    /// `ProviderEvent` or an error record.
    #[error("line {line} of the transcript is not a valid provider record")]
    ProviderRecord {
        /// The line, from 1.
        line: usize,
        /// The JSON error.
        #[source]
        source: serde_json::Error,
    },

    /// The replay provider got a request after the last exchange of its transcript.
    #[error("the replay provider got a request after the last provider_request of its transcript")]
    UnexpectedRequest {
        /// The request, redacted.
        request: Value,
    },

    /// A request differs from the `provider_request` record it was compared with.
    #[error(
        "the request does not match the provider_request at line {line}; the first difference \
         is at {pointer:?}"
    )]
    RequestMismatch {
        /// The line of the record.
        line: usize,
        /// The JSON pointer of the first difference; empty when the whole request
        /// differs.
        pointer: String,
        /// The record's body, redacted.
        expected: Value,
        /// The request, redacted.
        actual: Value,
    },

    /// The replay ended with exchanges of the transcript that were never requested.
    #[error(
        "the replay provider served {served} requests, but its transcript has {remaining} \
         more, the next at line {next_line}"
    )]
    UnusedRequests {
        /// The requests that matched.
        served: usize,
        /// The exchanges left.
        remaining: usize,
        /// The line of the next `provider_request` record.
        next_line: usize,
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
