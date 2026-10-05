//! `prompt.send`: send a prompt to a conversation.

use std::fmt;

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
/// `Debug` leaves out `last_command`, because a command line can hold a secret and
/// requests are logged.
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
    /// The mode, model and effort that the prompt asks for its turn; the config gives
    /// the rest. Absent means none, as from a client older than turn settings.
    #[serde(default, skip_serializing_if = "TurnSettings::is_empty")]
    pub settings: TurnSettings,
}

impl fmt::Debug for PromptSend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: last_command is deliberately missing; see the type's doc comment.
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
