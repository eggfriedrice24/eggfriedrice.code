//! The holder socket protocol, for milestone 5.
//!
//! From milestone 5, `efr-ptyd` holds the PTYs and the daemon reaches it over a Unix
//! socket. Every message is a [`RequestFrame`] or a [`ResponseFrame`], encoded with
//! `efr_protocol::framing` (a 4-byte big-endian length, then one JSON object). A
//! response that hands over a PTY master carries the descriptor in the same `sendmsg`
//! call (`SCM_RIGHTS`, through `efr-fdpass`), and [`HolderResponse::fd_count`] says how
//! many descriptors travel with it. Requests carry an id because the daemon calls the
//! holder from many shell sessions at once; responses may come back in any order.
//!
//! The first exchange is `hello` in both directions with
//! [`HOLDER_PROTOCOL_VERSION`]; a mismatch ends the connection with
//! [`HolderError::ProtocolMismatch`]. As on the daemon's own socket, names are
//! snake_case, internally tagged enums use the member `kind`, and unknown fields are
//! ignored.
//!
//! Nothing at milestone 1 sends these messages. They live next to the trait so that the
//! trait and its wire form change together.

use std::fmt;

use efr_protocol::{PtyId, Size};
use serde::{Deserialize, Serialize};

use crate::{ChildStatus, HolderError, PtyInfo, Signal, SignalTarget, SpawnSpec};

/// The version of the holder socket protocol. Both sides send it in `hello`; any
/// difference is a mismatch, because the daemon and `efr-ptyd` ship together.
pub const HOLDER_PROTOCOL_VERSION: u32 = 1;

/// One request from the daemon to the holder service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFrame {
    /// Unique among the requests in flight on the connection; the response carries it.
    pub id: u64,
    /// What the daemon asks for.
    pub request: HolderRequest,
}

/// The holder service's answer to one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseFrame {
    /// The id of the request this answers.
    pub id: u64,
    /// The answer.
    pub response: HolderResponse,
}

/// What the daemon asks the holder service to do: one variant per
/// [`PtyHolder`](crate::PtyHolder) method, plus `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HolderRequest {
    /// The first request on a connection.
    Hello {
        /// The daemon's [`HOLDER_PROTOCOL_VERSION`].
        protocol: u32,
    },
    /// [`PtyHolder::spawn`](crate::PtyHolder::spawn).
    Spawn {
        /// What to run.
        spec: SpawnSpec,
    },
    /// [`PtyHolder::resize`](crate::PtyHolder::resize).
    Resize {
        /// The PTY.
        pty_id: PtyId,
        /// The new size.
        size: Size,
    },
    /// [`PtyHolder::signal`](crate::PtyHolder::signal).
    Signal {
        /// The PTY.
        pty_id: PtyId,
        /// The signal.
        signal: Signal,
        /// Who receives it.
        target: SignalTarget,
    },
    /// [`PtyHolder::list`](crate::PtyHolder::list).
    List,
    /// [`PtyHolder::wait`](crate::PtyHolder::wait). The service answers once it has
    /// reaped the child, and answers other requests on the connection meanwhile.
    Wait {
        /// The PTY.
        pty_id: PtyId,
    },
    /// [`PtyHolder::release`](crate::PtyHolder::release).
    Release {
        /// The PTY.
        pty_id: PtyId,
    },
}

/// The holder service's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HolderResponse {
    /// The answer to `hello`.
    Hello {
        /// The service's [`HOLDER_PROTOCOL_VERSION`].
        protocol: u32,
    },
    /// The answer to `spawn`. The PTY master travels as the one descriptor of the same
    /// message.
    Spawned {
        /// The PTY, from the spec.
        pty_id: PtyId,
        /// The process id of the child.
        child_pid: u32,
    },
    /// The answer to `resize`, `signal` and `release`.
    Done,
    /// The answer to `list`.
    Listed {
        /// Every PTY the service holds.
        ptys: Vec<PtyInfo>,
    },
    /// The answer to `wait`: the child has been reaped.
    Exited {
        /// The PTY.
        pty_id: PtyId,
        /// How the child ended; never running.
        status: ChildStatus,
    },
    /// The request failed. On the wire the error's members sit next to `kind`.
    Error(WireError),
}

impl HolderResponse {
    /// How many file descriptors travel in the same message as this response: one PTY
    /// master for [`HolderResponse::Spawned`], none otherwise. The descriptor-passing
    /// layer sends exactly this many and refuses a message that carries a different
    /// number.
    pub fn fd_count(&self) -> usize {
        match self {
            HolderResponse::Spawned { .. } => 1,
            HolderResponse::Hello { .. }
            | HolderResponse::Done
            | HolderResponse::Listed { .. }
            | HolderResponse::Exited { .. }
            | HolderResponse::Error(_) => 0,
        }
    }

    /// The response itself, or the error it carries as [`HolderError::Remote`].
    pub fn into_result(self) -> Result<HolderResponse, HolderError> {
        match self {
            HolderResponse::Error(error) => Err(error.into()),
            other => Ok(other),
        }
    }
}

/// An error as it crosses the holder socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    /// The kind of failure.
    pub code: HolderErrorCode,
    /// A one-sentence description for logs. Callers decide on `code`, never on this.
    pub message: String,
}

impl From<&HolderError> for WireError {
    /// The holder service's side: its own error, as it sends it back. An error that
    /// came from another holder keeps that holder's code and message instead of being
    /// wrapped a second time.
    fn from(error: &HolderError) -> Self {
        match error {
            HolderError::Remote { code, message } => {
                WireError { code: *code, message: message.clone() }
            }
            local => WireError { code: local.code(), message: local.to_string() },
        }
    }
}

impl From<WireError> for HolderError {
    /// The daemon's side: the service's error, as [`HolderError::Remote`].
    fn from(error: WireError) -> Self {
        HolderError::Remote { code: error.code, message: error.message }
    }
}

/// The kind of a holder failure, local or remote. A closed set: a new code is a new
/// [`HOLDER_PROTOCOL_VERSION`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HolderErrorCode {
    /// The spawn spec failed [`SpawnSpec::validate`].
    InvalidSpec,
    /// A PTY with the spec's id is already held.
    AlreadyExists,
    /// No PTY with the id is held.
    NotFound,
    /// The PTY's child has exited.
    Exited,
    /// Opening the PTY or starting the program failed.
    SpawnFailed,
    /// An operating system call on a held PTY failed.
    Os,
    /// The two sides speak different protocol versions.
    ProtocolMismatch,
    /// Anything else, such as a malformed request.
    Internal,
}

impl fmt::Display for HolderErrorCode {
    /// The wire name, such as `not_found`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HolderErrorCode::InvalidSpec => "invalid_spec",
            HolderErrorCode::AlreadyExists => "already_exists",
            HolderErrorCode::NotFound => "not_found",
            HolderErrorCode::Exited => "exited",
            HolderErrorCode::SpawnFailed => "spawn_failed",
            HolderErrorCode::Os => "os",
            HolderErrorCode::ProtocolMismatch => "protocol_mismatch",
            HolderErrorCode::Internal => "internal",
        })
    }
}

#[cfg(test)]
mod tests;
