//! Responses stream events to canonical [`ProviderEvent`]s.
//!
//! [`EventMapper`] is a pure state machine over the decoded server-sent events of one
//! response, so the fixtures in `fixtures/responses/` test it without a server. The
//! event names and shapes follow Codex's stream parser
//! (`codex-rs/codex-api/src/sse/responses.rs`, `process_responses_event`):
//!
//! - `response.output_text.delta` and `response.refusal.delta` are text;
//!   `response.reasoning_summary_text.delta` and `response.reasoning_text.delta` are
//!   reasoning, with a blank line between summary sections;
//! - `response.output_item.added` starts a function call,
//!   `response.function_call_arguments.delta` grows it and `response.output_item.done`
//!   ends it with the complete arguments; a `custom_tool_call` item is a freeform call,
//!   which `response.custom_tool_call_input.delta` grows with plain text and whose
//!   finished item holds the whole text as `input`;
//! - every `response.output_item.done` item is kept verbatim, and `response.completed`
//!   ends the answer with the usage and those items as the message's `provider_raw`;
//! - `response.incomplete` ends it at the output limit or the content filter;
//!   `response.failed` and `error` fail the stream with the provider's error;
//! - events that only repeat what other events carry (`response.created`,
//!   `response.output_text.done` and the like) are consumed; any other event type is
//!   passed on as [`ProviderEvent::Raw`] and logged at debug, never dropped silently.
//!
//! When a server sends an item only whole, without deltas, its text, reasoning summary
//! or call is taken from `response.output_item.done`, so the canonical message never
//! misses what the raw items hold.

use std::collections::HashSet;
use std::time::Duration;

use efr_http::SseEvent;
use efr_provider::{ProviderError, ProviderEvent, StopReason, TokenUsage};
use serde_json::Value;

use crate::convert::{OutputItem, provider_raw};

/// The longest event type the debug log shows, so a hostile stream cannot flood it.
const MAX_LOGGED_TYPE: usize = 128;

/// Error codes that mean the request was rate limited rather than refused.
const RATE_LIMIT_CODES: &[&str] = &["rate_limit_exceeded", "rate_limit_error"];

/// Maps the events of one response, in order.
#[derive(Debug, Default)]
pub(crate) struct EventMapper {
    calls: Vec<Call>,
    items: Vec<Value>,
    text: Seen,
    reasoning: Seen,
    last_delta: LastDelta,
    tool_use: bool,
    done: bool,
}

/// A tool call that has started; for a freeform call, `arguments` is its text.
#[derive(Debug)]
struct Call {
    call_id: String,
    item_id: Option<String>,
    output_index: Option<u64>,
    arguments: String,
    ended: bool,
}

/// The items that have streamed deltas of one kind, by item id and by output index,
/// because a delta and its finished item may not carry the same one.
#[derive(Debug, Default)]
struct Seen {
    ids: HashSet<String>,
    indices: HashSet<u64>,
    /// A delta came with neither, so it may belong to any item.
    unkeyed: bool,
}

impl Seen {
    fn mark(&mut self, at: &Location) {
        match (&at.item_id, at.output_index) {
            (None, None) => self.unkeyed = true,
            (id, index) => {
                self.ids.extend(id.clone());
                self.indices.extend(index);
            }
        }
    }

    fn contains(&self, at: &Location) -> bool {
        self.unkeyed
            || at.item_id.as_ref().is_some_and(|id| self.ids.contains(id))
            || at.output_index.is_some_and(|index| self.indices.contains(&index))
    }
}

/// Where in the response an event belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Location {
    item_id: Option<String>,
    output_index: Option<u64>,
}

/// The kind of the last delta sent, to put a blank line between two reasoning sections
/// that would otherwise run together in one block.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum LastDelta {
    #[default]
    None,
    Other,
    Reasoning(Location, &'static str, i64),
}

impl EventMapper {
    pub(crate) fn new() -> Self {
        EventMapper::default()
    }

    /// True once the answer has ended, with `Done` or an error. Nothing after that
    /// counts.
    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    /// The canonical events for one server-sent event. An error ends the answer: the
    /// provider reported a failure, or sent data that is not JSON.
    pub(crate) fn map(&mut self, event: &SseEvent) -> Result<Vec<ProviderEvent>, ProviderError> {
        if self.done {
            return Ok(Vec::new());
        }
        let data = event.data.trim();
        // NOTE: the Responses API ends a stream by closing it; some proxies in front of
        // it add the Chat Completions sentinel, which carries nothing.
        if data == "[DONE]" {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(data).map_err(|source| {
            self.done = true;
            ProviderError::Decode { source }
        })?;
        let kind =
            value.get("type").and_then(Value::as_str).unwrap_or(event.event.as_str()).to_owned();
        let result = self.map_value(&kind, &value);
        if result.is_err() {
            self.done = true;
        }
        result
    }

    /// The canonical events for one message of the WebSocket transport: the same JSON
    /// event as the data of a server-sent event, named by its `type`.
    pub(crate) fn map_message(
        &mut self,
        event: &Value,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        if self.done {
            return Ok(Vec::new());
        }
        let kind = event.get("type").and_then(Value::as_str).unwrap_or_default();
        let result = self.map_value(kind, event);
        if result.is_err() {
            self.done = true;
        }
        result
    }

    fn map_value(
        &mut self,
        kind: &str,
        event: &Value,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let mut out = Vec::new();
        match kind {
            "response.output_text.delta" | "response.refusal.delta" => {
                if let Some(delta) = text(event, "delta").filter(|delta| !delta.is_empty()) {
                    self.text.mark(&location(event));
                    self.last_delta = LastDelta::Other;
                    out.push(ProviderEvent::TextDelta { text: delta.to_owned() });
                }
            }
            "response.reasoning_summary_text.delta" => {
                self.reasoning_delta(event, "summary_index", &mut out);
            }
            "response.reasoning_text.delta" => {
                self.reasoning_delta(event, "content_index", &mut out)
            }
            "response.output_item.added" => {
                if let Some(item) = event.get("item") {
                    self.item_added(item, &location_of_item(event, item), &mut out);
                }
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                self.arguments_delta(event, &mut out)
            }
            "response.function_call_arguments.done" | "response.custom_tool_call_input.done" => {
                let at = location(event);
                let key = if kind == "response.function_call_arguments.done" {
                    "arguments"
                } else {
                    "input"
                };
                if let (Some(call), Some(arguments)) =
                    (self.open_call_at(&at, text(event, "call_id")), text(event, key))
                {
                    arguments.clone_into(&mut call.arguments);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    self.item_done(item, &location_of_item(event, item), &mut out);
                }
            }
            "response.completed" => self.completed(event, &mut out),
            "response.incomplete" => self.incomplete(event, &mut out)?,
            "response.failed" => {
                let error = event.get("response").and_then(|response| response.get("error"));
                return Err(provider_error(error, ErrorShape::Nested, "the response failed"));
            }
            "error" => {
                // NOTE: the documented shape has `code` and `message` at the top level;
                // older streams nest them under `error`.
                let fallback = "the provider sent an error event";
                return Err(match event.get("error").filter(|error| error.is_object()) {
                    Some(error) => provider_error(Some(error), ErrorShape::Nested, fallback),
                    None => provider_error(Some(event), ErrorShape::TopLevel, fallback),
                });
            }
            "response.created"
            | "response.in_progress"
            | "response.queued"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.refusal.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.done" => {
                tracing::trace!(event_type = kind, "consumed a Responses event");
            }
            _ => {
                let shown: String = kind.chars().take(MAX_LOGGED_TYPE).collect();
                tracing::debug!(event_type = %shown, "passed on an unknown Responses event");
                out.push(ProviderEvent::Raw(event.clone()));
            }
        }
        Ok(out)
    }

    fn reasoning_delta(
        &mut self,
        event: &Value,
        index_key: &'static str,
        out: &mut Vec<ProviderEvent>,
    ) {
        let Some(delta) = text(event, "delta").filter(|delta| !delta.is_empty()) else {
            return;
        };
        let at = location(event);
        let index = event.get(index_key).and_then(Value::as_i64).unwrap_or(0);
        let section = LastDelta::Reasoning(at.clone(), index_key, index);
        self.reasoning.mark(&at);
        self.push_reasoning(section, delta, out);
    }

    /// Sends reasoning text, after a blank line when it starts a new section right
    /// after another one.
    fn push_reasoning(&mut self, section: LastDelta, text: &str, out: &mut Vec<ProviderEvent>) {
        let text =
            if matches!(self.last_delta, LastDelta::Reasoning(..)) && self.last_delta != section {
                format!("\n\n{text}")
            } else {
                text.to_owned()
            };
        self.last_delta = section;
        out.push(ProviderEvent::ReasoningDelta { text });
    }

    fn item_added(&mut self, item: &Value, at: &Location, out: &mut Vec<ProviderEvent>) {
        if let OutputItem::FunctionCall { call_id, name, freeform, .. } = OutputItem::parse(item) {
            self.start_call(call_id, name, freeform, at, out);
        }
    }

    fn start_call(
        &mut self,
        call_id: String,
        name: String,
        freeform: bool,
        at: &Location,
        out: &mut Vec<ProviderEvent>,
    ) {
        if self.calls.iter().any(|call| call.call_id == call_id) {
            return;
        }
        out.push(ProviderEvent::ToolCallStart { call_id: call_id.clone(), name, freeform });
        self.last_delta = LastDelta::Other;
        self.calls.push(Call {
            call_id,
            item_id: at.item_id.clone(),
            output_index: at.output_index,
            arguments: String::new(),
            ended: false,
        });
    }

    fn arguments_delta(&mut self, event: &Value, out: &mut Vec<ProviderEvent>) {
        let at = location(event);
        let Some(delta) = text(event, "delta").filter(|delta| !delta.is_empty()) else {
            return;
        };
        let Some(call) = self.open_call_at(&at, text(event, "call_id")) else {
            tracing::debug!("arguments for a tool call that has not started; left out");
            return;
        };
        call.arguments.push_str(delta);
        out.push(ProviderEvent::ToolCallDelta {
            call_id: call.call_id.clone(),
            arguments: delta.to_owned(),
        });
    }

    /// The started, not yet ended call at `at`, found by its item id, else by its
    /// output index, else by the `call_id` that a freeform call's input events may
    /// carry instead.
    fn open_call_at(&mut self, at: &Location, call_id: Option<&str>) -> Option<&mut Call> {
        let by_id = |call: &Call| at.item_id.is_some() && call.item_id == at.item_id;
        let by_index =
            |call: &Call| at.output_index.is_some() && call.output_index == at.output_index;
        let by_call = |call: &Call| call_id.is_some_and(|id| call.call_id == id);
        let position = self
            .calls
            .iter()
            .position(|call| !call.ended && by_id(call))
            .or_else(|| self.calls.iter().position(|call| !call.ended && by_index(call)))
            .or_else(|| self.calls.iter().position(|call| !call.ended && by_call(call)))?;
        self.calls.get_mut(position)
    }

    fn item_done(&mut self, item: &Value, at: &Location, out: &mut Vec<ProviderEvent>) {
        self.items.push(item.clone());
        match OutputItem::parse(item) {
            OutputItem::Message { text } => {
                if !text.is_empty() && !self.text.contains(at) {
                    self.last_delta = LastDelta::Other;
                    out.push(ProviderEvent::TextDelta { text });
                }
            }
            OutputItem::Reasoning { summary } => {
                if !summary.is_empty() && !self.reasoning.contains(at) {
                    let section = LastDelta::Reasoning(at.clone(), "item", 0);
                    self.push_reasoning(section, &summary, out);
                }
            }
            OutputItem::FunctionCall { call_id, name, arguments, freeform } => {
                self.start_call(call_id.clone(), name, freeform, at, out);
                if let Some(call) = self.calls.iter_mut().find(|call| call.call_id == call_id)
                    && !call.ended
                {
                    call.ended = true;
                    self.tool_use = true;
                    out.push(ProviderEvent::ToolCallEnd { call_id, arguments });
                }
            }
            OutputItem::Other { kind } => {
                tracing::debug!(item_type = %kind, "passed on an output item with no canonical form");
                out.push(ProviderEvent::Raw(item.clone()));
            }
        }
    }

    fn completed(&mut self, event: &Value, out: &mut Vec<ProviderEvent>) {
        let response = event.get("response");
        // A call whose item never finished still ends with the arguments that streamed,
        // because the server says the whole response is complete.
        for call in self.calls.iter_mut().filter(|call| !call.ended) {
            call.ended = true;
            self.tool_use = true;
            out.push(ProviderEvent::ToolCallEnd {
                call_id: call.call_id.clone(),
                arguments: call.arguments.clone(),
            });
        }
        let stop_reason = if self.tool_use { StopReason::ToolUse } else { StopReason::EndTurn };
        self.finish(response, stop_reason, out);
    }

    fn incomplete(
        &mut self,
        event: &Value,
        out: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let response = event.get("response");
        let reason = response
            .and_then(|response| response.get("incomplete_details"))
            .and_then(|details| details.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let stop_reason = match reason {
            "max_output_tokens" => StopReason::MaxTokens,
            "content_filter" => StopReason::ContentFilter,
            other => {
                return Err(ProviderError::Api {
                    status: None,
                    code: Some(other.to_owned()),
                    message: format!("the response stopped early: {other}"),
                });
            }
        };
        // NOTE: a call cut off by the limit must never run: its arguments may be a
        // prefix of a different command.
        if self.calls.iter().any(|call| !call.ended) {
            return Err(ProviderError::InvalidStream {
                problem: "the response stopped inside a tool call",
            });
        }
        self.finish(response, stop_reason, out);
        Ok(())
    }

    fn finish(
        &mut self,
        response: Option<&Value>,
        stop_reason: StopReason,
        out: &mut Vec<ProviderEvent>,
    ) {
        if let Some(usage) = response.and_then(|response| response.get("usage")).and_then(usage) {
            out.push(ProviderEvent::Usage(usage));
        }
        let mut items = std::mem::take(&mut self.items);
        if items.is_empty() {
            // A server that sent no finished items still lists them on the response.
            items = response
                .and_then(|response| response.get("output"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
        }
        let provider_raw = provider_raw(items);
        out.push(ProviderEvent::Done { stop_reason, provider_raw });
        self.done = true;
    }
}

fn text<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event.get(key).and_then(Value::as_str)
}

fn location(event: &Value) -> Location {
    Location {
        item_id: text(event, "item_id").map(str::to_owned),
        output_index: event.get("output_index").and_then(Value::as_u64),
    }
}

/// The location of an item event: the item's own id, else the event's.
fn location_of_item(event: &Value, item: &Value) -> Location {
    let mut at = location(event);
    if let Some(id) = text(item, "id") {
        at.item_id = Some(id.to_owned());
    }
    at
}

/// The usage of a finished response. Missing or `null` parts count as zero. OpenAI's
/// `input_tokens` holds the cached and the written tokens already, so the parts map
/// one to one; the API has no cache time to live to choose, so no write is for an hour.
fn usage(value: &Value) -> Option<TokenUsage> {
    if !value.is_object() {
        return None;
    }
    let count = |path: &[&str]| {
        path.iter().try_fold(value, |at, key| at.get(key)).and_then(Value::as_u64).unwrap_or(0)
    };
    Some(TokenUsage {
        input_tokens: count(&["input_tokens"]),
        output_tokens: count(&["output_tokens"]),
        cached_input_tokens: count(&["input_tokens_details", "cached_tokens"]),
        reasoning_tokens: count(&["output_tokens_details", "reasoning_tokens"]),
        cache_write_tokens: count(&["input_tokens_details", "cache_write_tokens"]),
        cache_write_1h_tokens: 0,
    })
}

/// Where a stream's error object sits, which decides what its `type` member means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorShape {
    /// An object of its own, whose `type` is the error's class, such as
    /// `invalid_request_error`.
    Nested,
    /// The event itself, whose `type` is the event's name.
    TopLevel,
}

/// The provider error for an error object of a stream, `{code, message}` with an
/// optional `type`.
fn provider_error(error: Option<&Value>, shape: ErrorShape, fallback: &str) -> ProviderError {
    let field = |key: &str| error.and_then(|error| error.get(key)).and_then(Value::as_str);
    let class = if shape == ErrorShape::Nested { field("type") } else { None };
    let code = field("code").or(class).map(str::to_owned);
    let message = field("message").filter(|message| !message.trim().is_empty()).unwrap_or(fallback);
    if code.as_deref().is_some_and(|code| RATE_LIMIT_CODES.contains(&code)) {
        return ProviderError::RateLimited { retry_after: retry_hint(message) };
    }
    ProviderError::api(None, code, message.to_owned())
}

/// The wait a rate-limit message asks for, as in "Please try again in 11.054s" or
/// "try again in 20ms".
pub(crate) fn retry_hint(message: &str) -> Option<Duration> {
    const MARKER: &str = "try again in ";
    let lower = message.to_ascii_lowercase();
    let start = lower.find(MARKER)? + MARKER.len();
    let token = lower.get(start..)?.split_whitespace().next()?;
    let token = token.trim_end_matches(['.', ',', ';']);
    let seconds = if let Some(millis) = token.strip_suffix("ms") {
        millis.parse::<f64>().ok()? / 1000.0
    } else if let Some((minutes, rest)) = token.split_once('m') {
        let seconds = match rest.strip_suffix('s') {
            Some(seconds) => seconds.parse::<f64>().ok()?,
            None if rest.is_empty() => 0.0,
            None => return None,
        };
        minutes.parse::<f64>().ok()? * 60.0 + seconds
    } else {
        token.strip_suffix('s')?.parse::<f64>().ok()?
    };
    Duration::try_from_secs_f64(seconds).ok()
}

#[cfg(test)]
mod tests;
