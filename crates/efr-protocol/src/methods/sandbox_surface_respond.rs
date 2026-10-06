//! `sandbox.surface_respond`: answer the question about a git setting or another file
//! that runs code, which a call changed and the launcher put in quarantine.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, QuestionId, Seq};

/// The params of `sandbox.surface_respond`. The daemon refuses it from a phone and
/// from a process that the model's commands started, and answers `not_found` for a
/// question that is not pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxSurfaceRespond {
    /// Makes the answer idempotent.
    pub command_id: CommandId,
    /// The conversation of the waiting turn.
    pub conversation_id: ConversationId,
    /// The question.
    pub question_id: QuestionId,
    /// True moves the changes back from quarantine; false leaves them there.
    pub keep: bool,
}

/// The result of `sandbox.surface_respond`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxSurfaceRespondResult {
    /// The sequence number of the `surface_question_answered` event.
    pub seq: Seq,
}
