//! `turn.interrupt`: stop the running turn.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, Seq, TurnId, WithdrawnPrompt};

/// The params of `turn.interrupt`.
///
/// Interrupting has two phases: the daemon records the request at once
/// (`turn_interrupt_requested`) and records `turn_interrupted` when the model's stream
/// has really stopped.
///
/// Esc in the input row of a turn also hands back what the client sent for that turn:
/// `resend_steers` and `withdraw`. The daemon does all of it in the one actor step that
/// records `turn_interrupt_requested`, so no queued prompt can start in between. It
/// records, in this order: `turn_interrupt_requested`, one `prompt_withdrawn` for each
/// withdrawn prompt in queue order, then the `prompt_queued` of the resent steers.
/// When the turn is not running (`conflict`), it does none of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnInterrupt {
    /// Makes the request idempotent.
    pub command_id: CommandId,
    /// The conversation whose running turn to stop.
    pub conversation_id: ConversationId,
    /// The turn the client means. When it is given and is not the running turn, the
    /// daemon answers `conflict` instead of stopping a different turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// The `turn_steered` events of this turn, by sequence number, whose text the
    /// client wants sent again as a new prompt. The daemon takes those that no
    /// `steering_delivered` names yet and skips the others. It joins their texts in
    /// sequence order with newlines into one prompt, which runs next, before the
    /// prompts that wait in the queue. The prompt has the context and the settings of
    /// the interrupted turn's prompt, and its `prompt_queued` names the steers and has
    /// this request's `command_id`. Absent means none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resend_steers: Vec<Seq>,
    /// Queued prompts of this conversation to withdraw, by turn. The daemon withdraws
    /// those that still wait and skips the others: a prompt that started, ended or was
    /// withdrawn, and a turn that it does not know. Absent means none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withdraw: Vec<TurnId>,
}

/// The result of `turn.interrupt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnInterruptResult {
    /// The turn that is being stopped.
    pub turn_id: TurnId,
    /// The sequence number of the `turn_interrupt_requested` event.
    pub seq: Seq,
    /// The prompt that the unread steers of `resend_steers` became. Absent when none
    /// of them was unread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resent: Option<ResentSteers>,
    /// The prompts of `withdraw` that the daemon withdrew, in queue order. Absent when
    /// none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withdrawn: Vec<WithdrawnPrompt>,
}

/// The prompt that unread steers became when their turn was interrupted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResentSteers {
    /// The new turn that answers the prompt.
    pub turn_id: TurnId,
    /// The sequence number of its `prompt_queued` event.
    pub seq: Seq,
    /// The `turn_steered` events whose text the prompt carries, in sequence order.
    pub steers: Vec<Seq>,
}
