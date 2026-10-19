//! `approval.respond`: answer an approval request.
//!
//! `allow_always` (phase E) runs the call and saves the rule that the question offered
//! in `exit.always`; a client sends it only when that member is set. Only a surface of
//! the user's own machine may send it, never a phone. A daemon or a turn that does not
//! know the answer denies the call.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ApprovalDecision, CallId, CommandId, ConversationId, Seq};

/// The params of `approval.respond`. The daemon answers `not_found` for a call that has
/// no pending approval, which includes one that expired at a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRespond {
    /// Makes the answer idempotent.
    pub command_id: CommandId,
    /// The conversation of the waiting turn.
    pub conversation_id: ConversationId,
    /// The tool call that waits for approval.
    pub call_id: CallId,
    /// The answer.
    pub decision: ApprovalDecision,
}

/// The result of `approval.respond`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRespondResult {
    /// The sequence number of the `approval_resolved` event.
    pub seq: Seq,
}
