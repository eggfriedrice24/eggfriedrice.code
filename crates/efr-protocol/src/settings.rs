//! Turn settings: the permission mode, the model and the reasoning effort of a turn.
//!
//! A prompt may ask for any of them ([`TurnSettings`]); the daemon fills in the rest from
//! its config when the turn starts and records what the turn runs with
//! ([`EffectiveSettings`]). A running turn never changes its settings.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ModeFallback, ProtocolError};

/// The permission mode of a turn: the base policy, which the user's own rules follow and
/// win over where they match.
///
/// Some floors hold in every mode: a privileged program such as `sudo` asks, secrets
/// are denied unless a user rule names them, and no tool writes efr's config. Full mode
/// keeps these floors and the other questions that `docs/permissions.md` names in its
/// section "Full mode"; the other modes also ask for an interactive call. A turn from a
/// remote origin runs with at most [`Mode::Cautious`].
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
    /// Shell commands run at once in a kernel sandbox (bubblewrap, Landlock and
    /// seccomp): they can write only in the turn's project, the registered projects
    /// that the command names, `$SCRATCH`, a private `/tmp` and private tool caches, and
    /// they have no network. An action that leaves the sandbox is an exit, and the user
    /// answers it. When the sandbox is not available, a turn runs as `cautious` and
    /// [`EffectiveSettings::fallback`] says why.
    Auto,
    /// Every call runs at once as the user, with no sandbox and no question. efr still
    /// reads each line for a small set of floors (`docs/permissions.md`, "Full mode").
    /// The user allows it with `permissions.allow_full`; a phone turn runs with at most
    /// `cautious`, a turn that a job notice starts with at most `auto`.
    Full,
}

impl Mode {
    /// Every mode, from the strictest to the loosest.
    pub const ALL: [Mode; 4] = [Mode::Manual, Mode::Cautious, Mode::Auto, Mode::Full];

    /// The wire form of the mode, such as `auto`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Manual => "manual",
            Mode::Cautious => "cautious",
            Mode::Auto => "auto",
            Mode::Full => "full",
        }
    }
}

/// Why the daemon refuses full mode although `permissions.allow_full` is true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FullRefusal {
    /// efrd runs as root.
    Root,
    /// `snapshot.enabled` is false, so `efr undo` cannot revert the turn.
    SnapshotOff,
    /// `undo.enabled` is false.
    UndoOff,
    /// `shell.program` is no zsh, so efr cannot clear sudo's cache before a call.
    NoZsh,
}

impl FullRefusal {
    /// Every refusal, in the order in which the daemon checks them.
    pub const ALL: [FullRefusal; 4] =
        [FullRefusal::Root, FullRefusal::SnapshotOff, FullRefusal::UndoOff, FullRefusal::NoZsh];

    /// The wire form of the refusal, such as `snapshot_off`.
    pub const fn as_str(self) -> &'static str {
        match self {
            FullRefusal::Root => "root",
            FullRefusal::SnapshotOff => "snapshot_off",
            FullRefusal::UndoOff => "undo_off",
            FullRefusal::NoZsh => "no_zsh",
        }
    }

    /// The text after "full mode cannot run: ", such as "efrd runs as root".
    pub const fn describe(self) -> &'static str {
        match self {
            FullRefusal::Root => "efrd runs as root",
            FullRefusal::SnapshotOff => {
                "snapshot.enabled is false, so efr undo cannot revert the turn"
            }
            FullRefusal::UndoOff => "undo.enabled is false, so efr undo cannot revert the turn",
            FullRefusal::NoZsh => {
                "shell.program is no zsh, so efr cannot clear sudo's cached password before \
                 a call"
            }
        }
    }
}

impl fmt::Display for FullRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
    /// The provider that runs the turn, such as `anthropic-api`. Absent from a daemon
    /// before it recorded the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Which values came from the prompt rather than from the config. Absent when none
    /// did.
    #[serde(default, skip_serializing_if = "OverriddenSettings::is_empty")]
    pub overridden: OverriddenSettings,
    /// Why the turn runs with a stricter mode than it asked for, such as `auto` without
    /// a working sandbox; `mode` is the mode it runs with. Absent when it runs with the
    /// mode it asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<ModeFallback>,
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
