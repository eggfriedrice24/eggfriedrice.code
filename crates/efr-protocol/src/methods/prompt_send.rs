//! `prompt.send`: send a prompt to a conversation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, Seq, ShellContext, TurnId};

/// The params of `prompt.send`.
///
/// Without `conversation_id`, the prompt goes to the active conversation of the
/// context's tty, and a new conversation starts when the tty has none. A prompt sent
/// while a turn runs queues behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
}
