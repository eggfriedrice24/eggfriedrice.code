//! The one public error type of the crate.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use efr_protocol::{ErrorBody, ProtocolError, RequestId};

/// Every way an `efr-client` operation can fail.
///
/// Variants carry the data a caller acts on (the socket, the versions, the daemon's
/// error body). The message names what failed in one sentence and leaves the source's
/// text to the `source()` chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// `daemon.json` exists but could not be read.
    #[error("could not read {}", .path.display())]
    ReadDaemonJson {
        /// The file.
        path: PathBuf,
        /// The error from the file system.
        #[source]
        source: io::Error,
    },

    /// `daemon.json` is not the JSON the daemon writes.
    #[error("{} is not a valid daemon.json", .path.display())]
    InvalidDaemonJson {
        /// The file.
        path: PathBuf,
        /// The error from the parser.
        #[source]
        source: serde_json::Error,
    },

    /// `daemon.json` names a socket by a relative path, which has no defined meaning
    /// for a client in another directory.
    #[error("{} names the relative socket path {}", .path.display(), .socket.display())]
    RelativeSocket {
        /// The file.
        path: PathBuf,
        /// The socket path it names.
        socket: PathBuf,
    },

    /// Nothing listens at the socket: it does not exist, or it is left over from a
    /// daemon that stopped.
    #[error("no daemon is listening on {}", .socket.display())]
    DaemonNotRunning {
        /// The socket.
        socket: PathBuf,
    },

    /// Connecting to the socket failed for another reason, such as missing permission.
    #[error("could not connect to {}", .socket.display())]
    Connect {
        /// The socket.
        socket: PathBuf,
        /// The error from the operating system.
        #[source]
        source: io::Error,
    },

    /// The socket did not accept the connection in time.
    #[error("connecting to {} did not finish within {after:?}", .socket.display())]
    ConnectTimedOut {
        /// The socket.
        socket: PathBuf,
        /// The timeout.
        after: Duration,
    },

    /// The daemon did not answer hello in time.
    #[error("the daemon did not answer hello within {after:?}")]
    HelloTimedOut {
        /// The timeout.
        after: Duration,
    },

    /// The daemon speaks another protocol version, as `daemon.json` or hello reports.
    #[error("the daemon speaks protocol {daemon} and this client speaks {client}")]
    ProtocolMismatch {
        /// The daemon's version.
        daemon: u32,
        /// This client's version.
        client: u32,
    },

    /// The daemon answered the request with an error.
    #[error("the daemon failed the request with {}: {}", .body.code, .body.message)]
    Server {
        /// The daemon's error: the code to act on, a message for a person, and data.
        body: ErrorBody,
    },

    /// A unary method finished without sending its result.
    #[error("{method} finished without a result")]
    MissingResult {
        /// The wire name of the method.
        method: &'static str,
    },

    /// A result or stream item is not the type that the caller asked for.
    #[error("an item of {method} has an unexpected shape")]
    DecodeItem {
        /// The wire name of the method.
        method: &'static str,
        /// The error from the parser.
        #[source]
        source: serde_json::Error,
    },

    /// `call` was used for a streaming method; use `stream`.
    #[error("{method} is a streaming method")]
    NotUnary {
        /// The wire name of the method.
        method: &'static str,
    },

    /// The stream's consumer fell behind its bounded queue, so the client cancelled
    /// the request instead of buffering without limit.
    #[error("the consumer of request {id} fell behind and the request was cancelled")]
    StreamOverflow {
        /// The request.
        id: RequestId,
    },

    /// The connection closed before the request finished.
    #[error("the connection to the daemon is closed")]
    Closed,

    /// Reading from or writing to the socket failed.
    #[error("the connection to the daemon failed")]
    Io {
        /// The error from the operating system.
        #[from]
        source: io::Error,
    },

    /// A frame could not be encoded or decoded, or the byte stream is out of step.
    #[error("a frame broke the protocol")]
    Protocol {
        /// The error from the framing or the JSON codec.
        #[source]
        source: ProtocolError,
    },
}

impl From<ProtocolError> for ClientError {
    fn from(source: ProtocolError) -> Self {
        ClientError::Protocol { source }
    }
}

#[cfg(test)]
mod tests;
