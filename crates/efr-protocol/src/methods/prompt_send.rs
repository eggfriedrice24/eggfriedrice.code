//! `prompt.send`: send a prompt to a conversation.

use std::fmt;
use std::path::PathBuf;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    CommandId, ConversationId, EffectiveSettings, Seq, ShellContext, TurnId, TurnSettings,
};

/// The params of `prompt.send`.
///
/// Without `conversation_id`, the prompt goes to the active conversation of the
/// context's tty, and a new conversation starts when the tty has none. A prompt sent
/// while a turn runs queues behind it.
///
/// `Debug` leaves out `last_command` and `recent_commands`, because a command line can
/// hold a secret and requests are logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptSend {
    /// Makes the send idempotent: a retry with the same id returns the first result.
    pub command_id: CommandId,
    /// The conversation to send to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Start a new conversation and make it the tty's active one (`,new`). The daemon
    /// rejects it together with `conversation_id`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_conversation: bool,
    /// The prompt.
    pub text: String,
    /// The user's shell when the prompt was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ShellContext>,
    /// The last command line from the user's shell history, for the turn's live-state
    /// preamble only.
    ///
    /// It is not part of [`ShellContext`], because a command line can hold a secret such
    /// as `export TOKEN=...`. The daemon hands it to the turn in memory and never records
    /// it in an event, which the event log keeps forever and every subscriber receives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_command: Option<String>,
    /// The recent commands of the terminal, oldest first, for the turn's live-state
    /// preamble only, as `last_command`. Absent means none, as from an older client.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_commands: Vec<RecentCommand>,
    /// The mode, model and effort that the prompt asks for its turn; the config gives
    /// the rest. Absent means none, as from a client older than turn settings.
    #[serde(default, skip_serializing_if = "TurnSettings::is_empty")]
    pub settings: TurnSettings,
}

impl fmt::Debug for PromptSend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: last_command and recent_commands are deliberately missing; see the
        // type's doc comment.
        f.debug_struct("PromptSend")
            .field("command_id", &self.command_id)
            .field("conversation_id", &self.conversation_id)
            .field("new_conversation", &self.new_conversation)
            .field("text", &self.text)
            .field("context", &self.context)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

/// A command line that the user ran in the terminal before a prompt: one entry of the
/// zsh plugin's list of the terminal's recent commands.
///
/// It travels like [`PromptSend::last_command`]: never in a [`ShellContext`], never in
/// an event, and only to the turn's preamble. `Debug` shows the command only by its
/// length.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecentCommand {
    /// The command line as the user typed it.
    pub command: String,
    /// Its exit status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i32>,
    /// The shell's `$PWD` when it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// When it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// How long it ran, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl RecentCommand {
    /// A command line with no status, directory or times.
    pub fn new(command: impl Into<String>) -> Self {
        RecentCommand {
            command: command.into(),
            status: None,
            cwd: None,
            ended_at: None,
            duration_ms: None,
        }
    }
}

impl fmt::Debug for RecentCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: the command can hold a secret; only its length shows.
        f.debug_struct("RecentCommand")
            .field("command", &format_args!("<{} bytes>", self.command.len()))
            .field("status", &self.status)
            .field("cwd", &self.cwd)
            .field("ended_at", &self.ended_at)
            .field("duration_ms", &self.duration_ms)
            .finish()
    }
}

/// The result of `prompt.send`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptSendResult {
    /// The conversation that took the prompt.
    pub conversation_id: ConversationId,
    /// The turn that will answer it.
    pub turn_id: TurnId,
    /// The sequence number of the `prompt_queued` event. A client that follows the
    /// answer subscribes with this as `after_seq`.
    pub seq: Seq,
    /// True when another turn was running, so the prompt waits behind it.
    pub queued: bool,
    /// The settings that the turn would run with if it started now. A queued turn
    /// resolves them again when it starts, so its `turn_started` can differ after the
    /// config changed. Absent when the daemon does not report them, as before turn
    /// settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<EffectiveSettings>,
}
