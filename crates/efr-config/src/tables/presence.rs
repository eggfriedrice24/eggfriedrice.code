//! `[presence]`: when efr counts you away from a conversation, and how long a question
//! waits then (efr's auto spec, section 9.5, phase A).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[presence]`: whether a person is at a conversation to answer its questions. Every
/// key applies from the next question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct PresenceSettings {
    /// The minutes without input in a conversation after which efr counts you away,
    /// from 1 to 1440. Input is a prompt, a steer, an answer or keys for the hidden
    /// shell. `efr away` and `,away` also count you away; your next input ends it.
    pub away_after_minutes: u32,
    /// How long a question waits while you are away, in seconds, from 1 to 86400.
    /// Then the call does not run, and the model goes on without it.
    pub away_wait_secs: u32,
    /// `true` sends a question that waits while you are away to your paired phones.
    /// No phone client exists yet, so it does nothing today.
    pub push: bool,
}

impl Default for PresenceSettings {
    fn default() -> Self {
        PresenceSettings { away_after_minutes: 10, away_wait_secs: 300, push: true }
    }
}
