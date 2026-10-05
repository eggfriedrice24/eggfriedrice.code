//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;

use efr_client::ClientError;
use efr_daemon::DaemonError;
use efr_protocol::ProtocolError;
use efr_test_support::TestSupportError;
use serde_json::Value;

/// Every way the test daemon, the fake PTY holder or a scenario replay can fail.
///
/// A replay that sees something other than its transcript says so with
/// [`TestDaemonError::Mismatch`], which carries the line and both JSON bodies, so the
/// `Debug` output of a failed test shows the difference.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TestDaemonError {
    /// A helper of `efr-test-support` failed: the temporary tree, a transcript, the
    /// replay provider.
    #[error("a test support helper failed")]
    Support {
        /// The error from `efr-test-support`.
        #[source]
        source: TestSupportError,
    },

    /// The daemon failed to start or to stop.
    #[error("the daemon failed")]
    Daemon {
        /// The daemon's error.
        #[source]
        source: DaemonError,
    },

    /// The daemon's serving task panicked or was cancelled.
    #[error("the daemon's task did not finish")]
    DaemonTask {
        /// The error from tokio.
        #[source]
        source: tokio::task::JoinError,
    },

    /// The protocol client failed.
    #[error("the protocol client failed")]
    Client {
        /// The client's error.
        #[source]
        source: ClientError,
    },

    /// A file or socket of the test daemon could not be used.
    #[error("{what} failed")]
    Io {
        /// What was being done, such as "creating the fake PTY".
        what: &'static str,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// A value could not be turned into JSON or back.
    #[error("{what} failed")]
    Json {
        /// What was being done, such as "encoding an event".
        what: &'static str,
        /// The error from serde_json.
        #[source]
        source: serde_json::Error,
    },

    /// A file the test daemon writes could not be written.
    #[error("could not write {}", .path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// A fake PTY that the test waited for was never spawned, because the holder is
    /// gone.
    #[error("the fake PTY {index} was never spawned")]
    PtyNeverSpawned {
        /// Its place in the order of spawns, from 0.
        index: usize,
    },

    /// The session closed the fake PTY while the test still read from it.
    #[error("the fake PTY closed after {} typed bytes", .typed.len())]
    PtyClosed {
        /// What was typed before it closed.
        typed: Vec<u8>,
    },

    /// No scenario of this name is in the table of `replay.rs`.
    #[error("there is no scenario named {name:?}")]
    UnknownScenario {
        /// The name asked for.
        name: String,
    },

    /// A record of a transcript cannot be replayed: a client frame that does not
    /// decode, an event before any conversation, a placeholder nothing bound.
    #[error("line {line} of the transcript cannot be replayed: {problem}")]
    InvalidRecord {
        /// The line, from 1.
        line: usize,
        /// What is wrong.
        problem: &'static str,
    },

    /// A record names an id placeholder that no earlier result or event bound.
    #[error("line {line} of the transcript names {placeholder}, which nothing bound yet")]
    UnboundPlaceholder {
        /// The line, from 1.
        line: usize,
        /// The placeholder, such as `<call:2>`.
        placeholder: String,
    },

    /// A client frame of a transcript is not a valid frame.
    #[error("line {line} of the transcript is not a valid client frame")]
    InvalidFrame {
        /// The line, from 1.
        line: usize,
        /// The decoding error.
        #[source]
        source: ProtocolError,
    },

    /// What the daemon did differs from the transcript.
    #[error("line {line} of the transcript expected another {kind}")]
    Mismatch {
        /// The line of the record, from 1.
        line: usize,
        /// The record's kind, such as `event` or `pty_bytes`.
        kind: &'static str,
        /// The record's body, with placeholders.
        expected: Value,
        /// What the daemon sent, with placeholders.
        actual: Value,
    },

    /// The subscription that a replay follows ended before the event it waited for.
    #[error("line {line} of the transcript waits for an event, but the subscription ended")]
    SubscriptionEnded {
        /// The line of the event record.
        line: usize,
    },

    /// A subscription a test followed ended before the event it waited for.
    #[error("the subscription ended before the event the test waited for")]
    StreamEnded,

    /// A transcript has `pty_bytes` records, but the daemon runs real shells.
    #[error("pty_bytes records need the fake PTY holder, and the daemon runs real shells")]
    NoFakeHolder,

    /// The replay provider did not get the request a record waits for within the
    /// real time limit of `efr_test_support::Wait`.
    #[error("line {line} of the transcript waits for a provider request that never came")]
    RequestNeverCame {
        /// The line of the `provider_request` record.
        line: usize,
    },

    /// The fake Responses server got a request that differs from its record.
    #[error("request {index} to the Responses server differs from the record at line {line}")]
    ResponsesMismatch {
        /// The request, from 0.
        index: usize,
        /// The line of the `provider_request` record.
        line: usize,
        /// The record's body, redacted.
        expected: Value,
        /// The request body, redacted.
        actual: Value,
    },

    /// The fake Responses server got another number of requests than its records.
    #[error("the Responses server got {received} requests, but the transcript has {expected}")]
    ResponsesCount {
        /// The requests it got.
        received: usize,
        /// The `provider_request` records.
        expected: usize,
    },

    /// Blessing a scenario did not settle: every run changed a provider request.
    #[error("blessing {name} did not settle after {runs} runs")]
    BlessUnsettled {
        /// The scenario.
        name: String,
        /// The runs tried.
        runs: usize,
    },
}

impl From<TestSupportError> for TestDaemonError {
    fn from(source: TestSupportError) -> Self {
        TestDaemonError::Support { source }
    }
}

impl From<DaemonError> for TestDaemonError {
    fn from(source: DaemonError) -> Self {
        TestDaemonError::Daemon { source }
    }
}

impl From<ClientError> for TestDaemonError {
    fn from(source: ClientError) -> Self {
        TestDaemonError::Client { source }
    }
}
