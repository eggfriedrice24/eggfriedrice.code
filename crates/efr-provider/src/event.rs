//! What a provider's stream yields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TokenUsage;

/// One event of a model's streamed answer, in canonical form.
///
/// The order a provider keeps:
///
/// - text, reasoning and tool call events come in the order the model produced them;
/// - a tool call is one `ToolCallStart`, any number of `ToolCallDelta`s with the same
///   call id, then one `ToolCallEnd` with the complete arguments (for a freeform call,
///   the complete text); calls may interleave, and a consumer that only needs the
///   result can ignore the deltas;
/// - `Usage` comes at most once, before `Done` (if it comes twice, the last counts);
/// - `Done` comes exactly once and is the last event;
/// - `Raw` may come anywhere.
///
/// A stream that yields an error ends there.
///
/// On the wire (replay transcripts, debug logs) an event is
/// `{"kind": "<snake_case variant>", "data": ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ProviderEvent {
    /// More of the assistant's text.
    TextDelta {
        /// The new text, to append.
        text: String,
    },

    /// More of the model's visible reasoning.
    ReasoningDelta {
        /// The new text, to append.
        text: String,
    },

    /// The model began a tool call.
    ToolCallStart {
        /// The provider's id of the call.
        call_id: String,
        /// The tool's name.
        name: String,
        /// True when the model calls a freeform tool in its freeform form: the
        /// arguments of the deltas and the end are then plain text, not JSON. False
        /// when absent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        freeform: bool,
    },

    /// More of a tool call's arguments, for showing a call as it is written.
    ToolCallDelta {
        /// The provider's id of the call.
        call_id: String,
        /// The next piece of the arguments text, or of a freeform call's text.
        arguments: String,
    },

    /// A tool call is complete.
    ToolCallEnd {
        /// The provider's id of the call.
        call_id: String,
        /// The complete arguments text, normally a JSON object; for a freeform call,
        /// the whole text the model wrote.
        arguments: String,
    },

    /// The tokens the call used.
    Usage(TokenUsage),

    /// The answer is complete.
    Done {
        /// Why the model stopped.
        stop_reason: StopReason,
        /// The provider's own items for the assistant message, which the conversation
        /// stores as [`Message::provider_raw`](crate::Message::provider_raw).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_raw: Option<Value>,
    },

    /// A provider event with no canonical form, passed on instead of being dropped
    /// silently. Consumers log it and otherwise ignore it.
    Raw(Value),
}

/// Why the model stopped producing output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopReason {
    /// The model finished its answer.
    EndTurn,
    /// The model wants the results of the tool calls it made.
    ToolUse,
    /// The answer reached the output token limit.
    MaxTokens,
    /// The provider stopped the answer for its content.
    ContentFilter,
}

#[cfg(test)]
mod tests;
