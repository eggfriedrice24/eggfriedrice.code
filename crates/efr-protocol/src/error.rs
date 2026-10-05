//! Errors: the wire error a server frame carries, and the one error type of this crate.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{RequestId, Seq};

/// What went wrong with a request, as a closed set that every client can handle.
///
/// The set is closed on the wire: a new code is a breaking change and bumps
/// [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION). The mapping from the daemon's own
/// errors to these codes lives in one place, `efr-daemon/src/error.rs`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    /// The client and the daemon speak different protocol versions. Sent in answer to
    /// `hello`, before any other method runs.
    ProtocolMismatch,
    /// The connection has not proven who it is.
    Unauthorized,
    /// The connection is known but lacks the scope that the method needs.
    Forbidden,
    /// A conversation, turn, approval or PTY that the request names does not exist.
    NotFound,
    /// The request contradicts the current state, such as steering a finished turn.
    Conflict,
    /// The daemon cannot take the request now; the client may retry later.
    Busy,
    /// A subscriber fell behind its bounded queue and was closed. The error data holds
    /// `last_seq`, the last sequence number it received, to resume from.
    Overflow,
    /// The request was cancelled by its client or by the daemon shutting down.
    Cancelled,
    /// The frame or its parameters are malformed.
    Invalid,
    /// The daemon failed in a way that is not the client's fault.
    Internal,
}

impl ErrorCode {
    /// Every code, in declaration order.
    pub const ALL: [ErrorCode; 10] = [
        ErrorCode::ProtocolMismatch,
        ErrorCode::Unauthorized,
        ErrorCode::Forbidden,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::Busy,
        ErrorCode::Overflow,
        ErrorCode::Cancelled,
        ErrorCode::Invalid,
        ErrorCode::Internal,
    ];

    /// The wire form of the code, such as `not_found`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::ProtocolMismatch => "protocol_mismatch",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Busy => "busy",
            ErrorCode::Overflow => "overflow",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Invalid => "invalid",
            ErrorCode::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The body of an error: a code to act on, a message for a person, and optional data
/// whose shape depends on the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    /// What went wrong.
    pub code: ErrorCode,
    /// One sentence for a person. Clients show it; they never parse it.
    pub message: String,
    /// Structured details. [`ErrorCode::Overflow`] carries `{"last_seq": n}` and
    /// [`ErrorCode::ProtocolMismatch`] carries `{"daemon": n, "client": m}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorBody {
    /// An error with no data.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ErrorBody { code, message: message.into(), data: None }
    }

    /// The same error with `data` attached.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The error that closes a subscriber that fell behind. `last_seq` is the last
    /// sequence number it received, so it can subscribe again from there.
    pub fn overflow(last_seq: Seq) -> Self {
        ErrorBody::new(
            ErrorCode::Overflow,
            format!("the subscriber fell behind; resume after seq {last_seq}"),
        )
        .with_data(serde_json::json!({ "last_seq": last_seq.get() }))
    }

    /// The answer to a `hello` whose protocol version differs from the daemon's.
    pub fn protocol_mismatch(daemon: u32, client: u32) -> Self {
        ErrorBody::new(
            ErrorCode::ProtocolMismatch,
            format!("the daemon speaks protocol {daemon} and the client speaks {client}"),
        )
        .with_data(serde_json::json!({ "client": client, "daemon": daemon }))
    }

    /// The `last_seq` value of an overflow error, when the data holds one.
    pub fn last_seq(&self) -> Option<Seq> {
        self.data.as_ref()?.get("last_seq")?.as_u64().map(Seq::new)
    }
}

/// A server frame that ends a request with an error: `{id, error}`.
///
/// `id` is absent only when the server could not read a request id from the frame it
/// rejects; the server then closes the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorFrame {
    /// The request that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// What went wrong.
    pub error: ErrorBody,
}

/// Every way an `efr-protocol` operation can fail.
///
/// Framing errors leave the byte stream out of step, so a peer that sees one closes the
/// connection. A decode error affects one frame only: the length prefix was valid, so the
/// next frame still starts at the right byte.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// A frame is larger than [`MAX_FRAME_LEN`](crate::framing::MAX_FRAME_LEN), either on
    /// the way out or in the length prefix of an incoming frame.
    #[error("a frame of {len} bytes is larger than the limit of {max} bytes")]
    FrameTooLarge {
        /// The length of the frame's JSON payload.
        len: usize,
        /// The limit.
        max: usize,
    },

    /// The byte stream ended inside a frame.
    #[error("the stream ended inside a frame, after {buffered} bytes of it")]
    Truncated {
        /// How many bytes of the unfinished frame had arrived, prefix included.
        buffered: usize,
    },

    /// A value could not be written as JSON, such as a path that is not UTF-8.
    #[error("a frame could not be encoded as JSON")]
    Encode {
        /// The error from the JSON encoder.
        #[source]
        source: serde_json::Error,
    },

    /// A frame payload is not valid JSON or does not have the expected shape.
    #[error("a frame could not be decoded")]
    Decode {
        /// The request id of the frame, when it could be read, so that the server can
        /// answer that request with [`ErrorCode::Invalid`].
        id: Option<RequestId>,
        /// The error from the JSON decoder.
        #[source]
        source: serde_json::Error,
    },

    /// A string is not a UUID, so it is not an id.
    #[error("{value:?} is not a valid {id_type}")]
    InvalidId {
        /// The id type that was expected, such as `ConversationId`.
        id_type: &'static str,
        /// The string that failed to parse.
        value: String,
        /// The error from the UUID parser.
        #[source]
        source: uuid::Error,
    },

    /// A string is not the name of a permission mode.
    #[error("{value:?} is not a permission mode")]
    UnknownMode {
        /// The string that failed to parse.
        value: String,
    },
}

#[cfg(test)]
mod tests;
