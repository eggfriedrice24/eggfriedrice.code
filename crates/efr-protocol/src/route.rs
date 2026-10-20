//! How efrd routes a prompt from a terminal: the scope key of a prompt, the label a
//! client shows for it, and the switch of conversation that a prompt caused.
//!
//! A terminal keeps one conversation per scope key: one for each registered project
//! that it worked in, and one for every place outside registered projects. A prompt
//! goes to the conversation of its key, so the conversation follows the scope when a
//! prompt is sent, never on `cd`. The result of `prompt.send` says which scope the
//! prompt had and, when the conversation changed, how.

use std::path::PathBuf;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ProjectId;

/// The scope key of a prompt: the registered project whose root holds the prompt's
/// working directory, or `outside` for every other place (`~`, `/etc`, a git
/// repository that is not registered).
///
/// On the wire: `{"kind": "project", "value": "<project id>"}` or
/// `{"kind": "outside"}`. A kind that this build does not know reads as `unknown`,
/// with or without a `value`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RouteScope {
    /// A registered project.
    Project(ProjectId),
    /// Every place that is not inside a registered project's root.
    Outside,
    /// A kind of key from a newer daemon. A reader treats it as `outside`.
    Unknown,
}

/// The wire form of a [`RouteScope`] as a reader takes it.
#[derive(Deserialize)]
struct RawRouteScope {
    kind: String,
    #[serde(default)]
    value: Option<serde_json::Value>,
}

impl<'de> Deserialize<'de> for RouteScope {
    // NOTE: written by hand, because serde reads the content of an adjacently tagged
    // `other` variant as a unit: a key of a newer daemon with a value would fail the
    // whole event, where it must read as `unknown` (check first VR1-8).
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawRouteScope::deserialize(deserializer)?;
        match raw.kind.as_str() {
            "project" => {
                let value = raw.value.ok_or_else(|| serde::de::Error::missing_field("value"))?;
                serde_json::from_value(value)
                    .map(RouteScope::Project)
                    .map_err(serde::de::Error::custom)
            }
            "outside" => Ok(RouteScope::Outside),
            _ => Ok(RouteScope::Unknown),
        }
    }
}

impl RouteScope {
    /// The project of the key, when it is one.
    pub fn project(&self) -> Option<ProjectId> {
        match self {
            RouteScope::Project(id) => Some(*id),
            _ => None,
        }
    }
}

/// The scope of a prompt as a client shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteLabel {
    /// The scope key.
    pub scope: RouteScope,
    /// The project's name, for a project. Absent outside projects and for a project
    /// that is no longer registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The project's root for a project; the working directory of the prompt
    /// outside projects.
    pub dir: PathBuf,
}

/// Where a prompt from a terminal went, for the line before the reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Routed {
    /// The scope of the prompt.
    pub scope: RouteLabel,
    /// How the conversation changed. Absent when the prompt continued the terminal's
    /// current conversation, was the terminal's first prompt, or started a new
    /// conversation on purpose (`,new`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch: Option<Switch>,
    /// A git repository that is not a registered project holds the prompt's working
    /// directory. Absent when there is none, or when the hint was shown recently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<RepoHint>,
}

/// How the conversation of a terminal changed with a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Switch {
    /// The prompt went back to the terminal's conversation of its scope.
    Back {
        /// Its title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// When it was last active, before this prompt.
        updated_at: Timestamp,
    },
    /// The prompt picked up the project's newest conversation, which no open terminal
    /// held.
    PickedUp {
        /// Its title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// When it was last active, before this prompt.
        updated_at: Timestamp,
    },
    /// The prompt started a new conversation.
    New {
        /// The scope of the terminal's conversation before. Absent when the reason is
        /// not a change of scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<RouteLabel>,
        /// Why a new conversation started.
        reason: NewReason,
    },
    /// A kind of switch from a newer daemon.
    #[serde(other)]
    Unknown,
}

/// Why a prompt started a new conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NewReason {
    /// The terminal had no conversation for the prompt's scope.
    Scope,
    /// The terminal's conversation of the scope was idle for longer than
    /// `conversation.tty_idle_hours`.
    Idle {
        /// How many whole hours it was idle.
        hours: u64,
    },
    /// Another terminal took the terminal's conversation of the scope.
    Moved {
        /// That terminal.
        tty: String,
        /// The title of the conversation that moved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// A reason from a newer daemon.
    #[serde(other)]
    Unknown,
}

/// A git repository that is not a registered project, so writes there ask the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoHint {
    /// The repository's work tree.
    pub root: PathBuf,
}

/// How a conversation came to a terminal (`terminal_attached`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttachHow {
    /// A prompt from a new terminal in the project picked it up, because no open
    /// terminal held it.
    PickedUp,
    /// The user resumed it in the terminal (`,resume`).
    Resumed,
    /// A way from a newer daemon.
    #[serde(other)]
    Unknown,
}

impl AttachHow {
    /// Every known way, for the fixtures.
    pub const ALL: [AttachHow; 3] = [AttachHow::PickedUp, AttachHow::Resumed, AttachHow::Unknown];
}

#[cfg(test)]
mod tests;
