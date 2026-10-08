//! `turn.steer`: add guidance to the running turn.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, Seq, TurnId};

/// The params of `turn.steer` (`,!` in the shell). Unlike `prompt.send`, it does not
/// queue: the text joins the running turn. A turn that has made its last model call
/// takes no more guidance, and the daemon answers `conflict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnSteer {
    /// Makes the request idempotent.
    pub command_id: CommandId,
    /// The conversation whose running turn to steer.
    pub conversation_id: ConversationId,
    /// The turn the client means. When it is given and is not the running turn, the
    /// daemon answers `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// The guidance.
    pub text: String,
}

/// The result of `turn.steer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnSteerResult {
    /// The turn that took the guidance.
    pub turn_id: TurnId,
    /// The sequence number of the `turn_steered` event.
    pub seq: Seq,
}
