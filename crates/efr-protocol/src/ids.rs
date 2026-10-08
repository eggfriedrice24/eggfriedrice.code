//! Identifiers on the wire.
//!
//! Entity ids are newtypes over a UUID, so a turn id cannot go where a conversation id
//! belongs. The daemon and the clients mint them as version 7 UUIDs with
//! `efr_stdx::id::uuid_v7(clock, rng)` and wrap the result with `from_uuid`; this crate
//! has no clock and no generator of its own (the README says why). On the wire every id
//! is the lowercase hyphenated form.
//!
//! The parsers accept any UUID version. Only the daemon and its own clients mint ids,
//! and a stricter parser would turn a client's harmless version 4 id into a failed
//! request.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Declares a UUID newtype with the same API and wire form as every other entity id.
/// Paths inside are absolute, so that `scope.rs` can use the macro for `ProjectId`.
macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
            schemars::JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// Wraps a UUID. Callers pass a version 7 UUID from
            /// `efr_stdx::id::uuid_v7`, so that ids sort by creation time.
            pub const fn from_uuid(uuid: uuid::Uuid) -> Self {
                Self(uuid)
            }

            /// The UUID inside.
            pub const fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::ProtocolError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                uuid::Uuid::parse_str(text).map(Self).map_err(|source| {
                    $crate::ProtocolError::InvalidId {
                        id_type: stringify!($name),
                        value: text.to_owned(),
                        source,
                    }
                })
            }
        }
    };
}

pub(crate) use uuid_id;

uuid_id!(
    /// One conversation: the unit a `,` line talks to and a subscription follows.
    ConversationId
);

uuid_id!(
    /// One turn of a conversation: a prompt and everything the model does for it.
    TurnId
);

uuid_id!(
    /// A client-generated id that makes a write idempotent. The daemon stores a receipt
    /// per command id, and a retry with the same id returns the stored result.
    CommandId
);

uuid_id!(
    /// One tool call inside a turn. The daemon mints it; the provider's own call id stays
    /// in the provider's raw items.
    CallId
);

uuid_id!(
    /// One pseudo-terminal: a hidden conversation shell or, later, a proxied terminal.
    PtyId
);

uuid_id!(
    /// An enrolled device such as a phone. The Unix socket does not use it.
    DeviceId
);

uuid_id!(
    /// A question to the user that is not an approval of a tool call, such as whether
    /// to keep a git setting that a sandboxed call changed.
    QuestionId
);

uuid_id!(
    /// One compaction of a conversation's context: a `conversation_compacted` event.
    CompactionId
);

uuid_id!(
    /// The identity of one daemon installation, created at its first start. Clients pin
    /// it, so they notice when a socket path leads to a different daemon.
    DaemonId
);

/// A position in an ordered stream: the global sequence number of the event log, or the
/// byte offset in a PTY recording.
///
/// Event sequence numbers start at 1 and only grow, so a client resumes a subscription
/// from the largest one it has seen.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Seq(u64);

impl Seq {
    /// The position before the first event or the first byte.
    pub const ZERO: Seq = Seq(0);

    /// The sequence number `value`.
    pub const fn new(value: u64) -> Self {
        Seq(value)
    }

    /// The number inside.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The id that a client gives a request. It is unique among the requests in flight on
/// one connection; every server frame about the request carries it, and a cancel frame
/// names it.
///
/// It is a number, not a UUID, because it only has to be unique per connection and a
/// counter is the cheapest way to get that.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct RequestId(u64);

impl RequestId {
    /// The request id `value`.
    pub const fn new(value: u64) -> Self {
        RequestId(value)
    }

    /// The number inside.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests;
