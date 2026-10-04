//! `approval.respond`: answer an approval request.

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
