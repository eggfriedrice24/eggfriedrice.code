//! Presence: whether a person is at a conversation to answer its questions.
//!
//! efrd keeps one state per conversation. Input of an attached client (a prompt, a
//! steer, an answer, keys for the hidden shell) makes it present; no input for
//! `presence.away_after_minutes`, `efr away` or `,away` makes it away. While it is away,
//! a question waits only `presence.away_wait_secs`, and then the call does not run.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether a person is at a conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Presence {
    /// A person had input in the conversation lately; a question waits for the answer.
    #[default]
    Present,
    /// Nobody had input for a while, or the user said so; a question waits a short time
    /// and then the call does not run.
    Away,
}

impl Presence {
    /// The wire form, such as `away`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Presence::Present => "present",
            Presence::Away => "away",
        }
    }
}

/// What changed the presence of a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PresenceSource {
    /// Input of an attached client.
    Input,
    /// The user, with `presence.set` (`efr away`, `,away`).
    User,
    /// No input for `presence.away_after_minutes`.
    Timeout,
}

impl PresenceSource {
    /// The wire form, such as `timeout`.
    pub const fn as_str(self) -> &'static str {
        match self {
            PresenceSource::Input => "input",
            PresenceSource::User => "user",
            PresenceSource::Timeout => "timeout",
        }
    }
}
