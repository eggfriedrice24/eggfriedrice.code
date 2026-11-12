//! What a provider's stream yields.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{HostedAction, HostedOutcome, TokenUsage};

/// One event of a model's streamed answer, in canonical form.
///
/// The order a provider keeps:
///
/// - text, reasoning and tool call events come in the order the model produced them;
/// - a tool call is one `ToolCallStart`, any number of `ToolCallDelta`s with the same
///   call id, then one `ToolCallEnd` with the complete arguments (for a freeform call,
///   the complete text); calls may interleave, and a consumer that only needs the
///   result can ignore the deltas;
/// - a step of a hosted tool is one `HostedCallStart`, then at most one `HostedCallEnd`
///   with the same call id; the end carries the complete action (the start can lack the
///   query). An end may come without a start in the same answer: its start was in an
///   earlier answer of the same turn, such as before a `pause_turn`;
/// - a `Citation` with `placed` comes after the last character of the text that it
///   backs and before any later `TextDelta`; one without `placed` may come anywhere
///   before `Done`;
/// - no encrypted content of the provider is in these events: it stays in
///   `provider_raw`;
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

    /// The provider began a step of a hosted tool, such as a web search
    /// ([`HostedTool`](crate::HostedTool)). efr does not run it and never asks before
    /// it. It is a border of the text: the next `TextDelta` starts a new text block, so
    /// the turn can end the assistant message before the row of the step.
    HostedCallStart {
        /// The provider's id of the step.
        call_id: String,
        /// What the model asked for, as far as the provider says it now.
        action: HostedAction,
    },

    /// A step of a hosted tool ended. Its start can be in an earlier answer of the same
    /// turn; then this answer has no start for it.
    HostedCallEnd {
        /// The provider's id of the step.
        call_id: String,
        /// The complete action.
        action: HostedAction,
        /// How it ended.
        outcome: HostedOutcome,
    },

    /// The answer cites a page. The turn gives each new URL of its turn the next number
    /// and lists the cited pages after the answer.
    Citation {
        /// The page.
        url: String,
        /// Its title, when the provider gives one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// True when the citation backs the text that the stream gave right before it,
        /// and no later text came yet: the turn then puts the mark `[n]` there. False
        /// when absent: the citation only gets its number and its entry in the list.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        placed: bool,
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
    /// The model refused to answer, such as Anthropic's `refusal`. A reader that
    /// does not tell the two apart treats it as [`ContentFilter`](Self::ContentFilter).
    Refusal,
    /// The API paused a long turn, such as Anthropic's `pause_turn`, which only server
    /// tools cause, such as a long web search. efr sends the paused answer back as it
    /// is and the model goes on in the same turn; the reason shows only when the turn
    /// stopped after its most continuations.
    PauseTurn,
}

#[cfg(test)]
mod tests;
