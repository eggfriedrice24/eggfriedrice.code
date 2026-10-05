//! Turn settings: the permission mode, the model and the reasoning effort of a turn.
//!
//! A prompt may ask for any of them ([`TurnSettings`]); the daemon fills in the rest from
//! its config when the turn starts and records what the turn runs with
//! ([`EffectiveSettings`]). A running turn never changes its settings.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ProtocolError;

/// The permission mode of a turn: the base policy, which the user's own rules follow and
/// win over where they match.
///
/// The floors hold in every mode: an interactive call asks, a privileged program asks,
/// secrets are denied unless a user rule names them, and no tool writes efr's config. A
/// turn from a remote origin runs with at most [`Mode::Cautious`].
///
/// The variants are ordered from the strictest to the loosest, so the remote cap is
/// `mode.min(Mode::Cautious)`.
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
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Mode {
    /// Every call asks, except what the user's own rules allow.
    Manual,
    /// The built-in rules that let commands and files be read without a question. The
    /// default.
    #[default]
    Cautious,
    /// `cautious` plus a curated list: writes in the project and `$SCRATCH`, the
    /// project's builds and tests, and local git. General network access still asks.
    Auto,
}

impl Mode {
    /// Every mode, from the strictest to the loosest.
    pub const ALL: [Mode; 3] = [Mode::Manual, Mode::Cautious, Mode::Auto];

    /// The wire form of the mode, such as `auto`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Manual => "manual",
            Mode::Cautious => "cautious",
            Mode::Auto => "auto",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Mode {
    type Err = ProtocolError;

    /// Reads the wire form, such as `auto`, as a flag or a variable gives it.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Mode::ALL
            .into_iter()
            .find(|mode| mode.as_str() == text)
            .ok_or_else(|| ProtocolError::UnknownMode { value: text.to_owned() })
    }
}

/// The settings that a prompt asks for its turn. Each one it leaves out comes from the
/// daemon's config when the turn starts.
///
/// The daemon checks each given value at `prompt.send` and again when the turn starts:
/// the mode by name, the model against `models.list`, the effort against the efforts
/// that `models.list` names for the turn's model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct TurnSettings {
    /// The permission mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// The model id, such as `gpt-5.5`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The reasoning effort, such as `high`. A plain string, because the efforts belong to
    /// the model and the backend, so a new effort needs no protocol change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl TurnSettings {
    /// True when the prompt asks for no setting, so every one comes from the config.
    pub fn is_empty(&self) -> bool {
        self.mode.is_none() && self.model.is_none() && self.effort.is_none()
    }
}

/// The settings that a turn runs with: the prompt's [`TurnSettings`] over the config's
/// defaults, checked by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveSettings {
    /// The permission mode. For a turn from a remote origin it is at most `cautious`,
    /// whatever the prompt or the config asked for.
    pub mode: Mode,
    /// The model id.
    pub model: String,
    /// The reasoning effort; absent when none is sent, so the backend uses its own
    /// default for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Which values came from the prompt rather than from the config. Absent when none
    /// did.
    #[serde(default, skip_serializing_if = "OverriddenSettings::is_empty")]
    pub overridden: OverriddenSettings,
}

/// Which of a turn's [`EffectiveSettings`] the prompt set; the others are the config's.
/// Each flag is left out when it is false.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct OverriddenSettings {
    /// The mode came from the prompt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mode: bool,
    /// The model came from the prompt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub model: bool,
    /// The effort came from the prompt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub effort: bool,
}

impl OverriddenSettings {
    /// True when the prompt set none of the values.
    pub fn is_empty(&self) -> bool {
        !(self.mode || self.model || self.effort)
    }
}

#[cfg(test)]
mod tests;
