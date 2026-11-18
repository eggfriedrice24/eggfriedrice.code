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

/// Declares the id of the `n`th item of one conversation, such as a job `j3` or an
/// attachment `a3`: the prefix letter, then a number from 1 to
/// `MAX_NUMBER` with no leading zero. On the wire a string; any other string is
/// refused. Ids repeat across conversations, so an item is named by its conversation
/// and its id.
macro_rules! numbered_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal, $noun:literal, $a_noun:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// The largest number.
            pub const MAX_NUMBER: u32 = 999_999_999;

            #[doc = concat!("The id of the `n`th ", $noun, " of a conversation, `", $prefix, "{n}`.")]
            /// `n` is at least 1 and at most [`MAX_NUMBER`](Self::MAX_NUMBER); a value
            /// outside takes the nearest end.
            pub fn nth(n: u32) -> Self {
                Self(format!(concat!($prefix, "{}"), n.clamp(1, Self::MAX_NUMBER)))
            }

            #[doc = concat!("The number, such as 3 for `", $prefix, "3`.")]
            pub fn number(&self) -> Option<u32> {
                self.0.strip_prefix($prefix).and_then(|digits| digits.parse().ok())
            }

            #[doc = concat!("The id as text, such as `", $prefix, "3`.")]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// True for the prefix, then a digit from 1 to 9, then at most eight
            /// digits.
            fn valid(text: &str) -> bool {
                let Some(digits) = text.strip_prefix($prefix) else {
                    return false;
                };
                (1..=9).contains(&digits.len())
                    && digits.bytes().all(|byte| byte.is_ascii_digit())
                    && !digits.starts_with('0')
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::ProtocolError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                if Self::valid(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err($crate::ProtocolError::InvalidNumberedId { noun: $a_noun, value: text.to_owned() })
                }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
                    "type": "string",
                    "pattern": concat!("^", $prefix, "[1-9][0-9]{0,8}$"),
                    "description": concat!("The id of ", $a_noun, " of one conversation, such as ", $prefix, "3.")
                })
            }
        }
    };
}

pub(crate) use numbered_id;

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
    /// One pairing of a device that `device.pair_start` opened: its QR code, the
    /// phone's enroll and the confirmation at the terminal.
    PairingId
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
    /// One undo of a turn, or one redo: the name of its refs in efr's snapshot stores
    /// and of its directory in the undo trash.
    UndoId
);

impl UndoId {
    /// The form that a person sees and that names the undo trash directory:
    /// `u-<uuid>`.
    pub fn shown(&self) -> String {
        format!("u-{}", self.0.hyphenated())
    }
}

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
