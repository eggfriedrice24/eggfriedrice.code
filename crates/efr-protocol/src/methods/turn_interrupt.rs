//! `turn.interrupt`: stop the running turn.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, Seq, TurnId};

/// The params of `turn.interrupt`.
///
/// Interrupting has two phases: the daemon records the request at once
/// (`turn_interrupt_requested`) and records `turn_interrupted` when the model's stream
/// has really stopped.
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
}

/// The result of `turn.interrupt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnInterruptResult {
    /// The turn that is being stopped.
    pub turn_id: TurnId,
    /// The sequence number of the `turn_interrupt_requested` event.
    pub seq: Seq,
}
