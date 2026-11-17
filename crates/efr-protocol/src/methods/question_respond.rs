//! `question.respond`: a person's answer to the model's question (the tool `ask_user`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandId, ConversationId, QuestionId, QuestionReply, Seq};

/// The params of `question.respond`, a unary method with the scope `operate`. efrd
/// announces it with the capability `user_questions`.
///
/// efrd answers `not_found` when no such question waits, and `invalid` for an `index`
/// outside the options, for a `text` that is empty, longer than
/// [`QuestionReply::MAX_TEXT_BYTES`] or holds a control character, and for a reply of
/// the kind `unknown`. A retried command answers from its receipt. The answer approves
/// nothing: it is information for the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionRespond {
    /// Makes the answer idempotent.
    pub command_id: CommandId,
    /// The conversation of the waiting turn.
    pub conversation_id: ConversationId,
    /// The question, from `user_question_requested`.
    pub question_id: QuestionId,
    /// The answer.
    pub answer: QuestionReply,
}

/// The result of `question.respond`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionRespondResult {
    /// The sequence number of the `user_question_answered` event.
    pub seq: Seq,
}
