//! `conversation.resume`: make a conversation the conversation of this terminal again,
//! for `,resume N` and `efr resume N`.

use std::path::PathBuf;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, RouteLabel, ShellContext};

/// The params of `conversation.resume`, an operate method.
///
/// The conversation goes into the slot of its own scope key in the terminal of
/// `context`, and that slot becomes the terminal's current one: efrd records
/// `terminal_attached` (`how: resumed`). Another terminal that held it lets it go, and
/// its next prompt there says where it went. When the conversation has no hidden shell
/// that runs, efrd starts one in its last directory.
///
/// Refusals: `invalid` for a context without a tty, `forbidden` for a connection that
/// is not the shell's or `efr`'s on that terminal (a phone, a made-up tty), and
/// `not_found` for a conversation that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConversationResume {
    /// Makes the resume idempotent.
    pub command_id: CommandId,
    /// The conversation.
    pub conversation_id: ConversationId,
    /// The terminal's shell, as the zsh plugin observed it: its `tty` and `shell_pid`
    /// name the terminal and the shell that hold the conversation, and its `pwd` says
    /// whether the next prompt continues it.
    pub context: ShellContext,
    /// Move the conversation also when another open terminal holds it. Without it,
    /// efrd moves nothing and answers `held`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub take: bool,
}

/// The result of `conversation.resume`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConversationResumeResult {
    /// The conversation is the terminal's conversation for its scope now.
    Resumed {
        /// The conversation.
        conversation_id: ConversationId,
        /// Its scope: the slot of the terminal that holds it now.
        scope: RouteLabel,
        /// Its title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// When it was last active, before the resume.
        updated_at: Timestamp,
        /// The directory of its hidden shell: where efrd started it, or where the
        /// shell that runs is. Absent when no shell could start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell_dir: Option<PathBuf>,
        /// True when the terminal held it already: the resume only made its slot the
        /// current one.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        here: bool,
        /// True when a prompt from the working directory of the params continues it:
        /// the directory is in its scope, or `conversation.follow_project` is off.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        continues_here: bool,
    },
    /// Another open terminal holds the conversation, and `take` was not set: nothing
    /// moved.
    Held {
        /// That terminal.
        tty: String,
        /// The conversation's title, for the question.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// An answer from a newer daemon.
    #[serde(other)]
    Unknown,
}
