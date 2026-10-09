//! The stream of a Messages call to canonical provider events: a pure state machine
//! over the events that `efr_http::SseDecoder` parses.
//!
//! [`EventMapper`] reads the events of one answer in order, so the fixtures in
//! `fixtures/messages/` test it without a server.
//!
//! | Event | What efr does |
//! |---|---|
//! | `message_start` | keeps `message.usage` as the base, and logs each entry of `message.input_transformations` (a block that `drop_block` dropped) at `warn` |
//! | `content_block_start` `text` or `thinking` | starts the block |
//! | `content_block_start` `redacted_thinking` | keeps the block as it came for `provider_raw`, no event |
//! | `content_block_start` `tool_use` | `ToolCallStart { call_id, name }` |
//! | `content_block_delta` `text_delta` | `TextDelta` |
//! | `content_block_delta` `thinking_delta` | `ReasoningDelta` when it is not empty (a summary under `display: "summarized"`), after a blank line when a new thinking block follows another |
//! | `content_block_delta` `signature_delta` | kept for `provider_raw` only |
//! | `content_block_delta` `input_json_delta` | `ToolCallDelta`; the fragments join |
//! | `content_block_delta` `citations_delta` | nothing: efr sends no documents |
//! | `content_block_stop` of a `tool_use` | `ToolCallEnd` with the joined input; an empty input is the start's `input`, else `{}`; an input that is not a JSON object fails the stream |
//! | `message_delta` | overwrites each usage member that it sends (the counts are cumulative) and keeps `stop_reason` |
//! | `message_stop` | one `Usage` (see `usage`) and `Done { stop_reason, provider_raw }` |
//! | `ping` | nothing |
//! | `error` | the error of its `error.type` (see `failure`); the request is never sent again |
//! | any other event, block or delta type | `Raw`, never a failure: the API adds types within a version |
//!
//! `provider_raw` is the content array of the assistant message as one JSON string:
//! the exact text that efr built from the stream, with every `thinking`,
//! `redacted_thinking`, `text` and `tool_use` block, the empty ones too, so the next
//! request sends it back byte for byte. A `tool_use` keeps its input as the text that
//! the model wrote, and a `redacted_thinking` block, or a block of a type that efr does
//! not know, the text that the API sent. An answer without blocks has no
//! `provider_raw`.
//!
//! Stop reasons: `end_turn` and `stop_sequence` are `EndTurn`, `tool_use` is
//! `ToolUse`, `max_tokens` is `MaxTokens`, `refusal` is `ContentFilter` (the turn runs
//! none of its tools), `model_context_window_exceeded` is `MaxTokens` with a `warn`
//! line, and `pause_turn` (server tools only, which efr does not send) is `EndTurn` with
//! a `warn` line. A missing or unknown stop reason is `ToolUse` when the answer has a
//! tool call, else `EndTurn`.

use efr_http::SseEvent;
use efr_provider::{ProviderError, ProviderEvent, StopReason};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::failure::stream_error;
use crate::usage::StreamUsage;

/// The longest event or block type the debug log shows, so a hostile stream cannot
/// flood it.
const MAX_LOGGED_TYPE: usize = 128;

/// The input of a `tool_use` whose start and deltas gave none.
const EMPTY_INPUT: &str = "{}";

/// Maps the events of one answer, in order.
#[derive(Debug, Default)]
pub(crate) struct EventMapper {
    blocks: Vec<Block>,
    usage: StreamUsage,
    stop_reason: Option<String>,
    last: LastDelta,
    done: bool,
}

/// One content block of the answer, by its `index`.
#[derive(Debug)]
struct Block {
    index: u64,
    kind: Kind,
    stopped: bool,
}

#[derive(Debug)]
enum Kind {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        /// The `input` of `content_block_start`, as it came.
        start: Option<Box<RawValue>>,
        /// The joined `partial_json` fragments.
        joined: String,
        /// The input at `content_block_stop`, checked to be a JSON object.
        input: Option<Box<RawValue>>,
    },
    /// A `redacted_thinking` block, or a block of a type that efr does not know, kept
    /// as the API sent it.
    Verbatim(Box<RawValue>),
}

/// The kind of the last canonical delta, to put a blank line between two thinking
/// blocks that would otherwise run together in one reasoning block.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum LastDelta {
    #[default]
    None,
    Other,
    Reasoning(u64),
}

/// A block of the content array of `provider_raw`, in the API's member order.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum RawBlock<'a> {
    Built(BuiltBlock<'a>),
    Verbatim(&'a RawValue),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BuiltBlock<'a> {
    Text { text: &'a str },
    Thinking { thinking: &'a str, signature: &'a str },
    ToolUse { id: &'a str, name: &'a str, input: &'a RawValue },
}

/// The `content_block` of a `content_block_start`, as the API sent it.
#[derive(Debug, Deserialize)]
struct BlockStart<'a> {
    #[serde(borrow)]
    content_block: &'a RawValue,
}

/// The `input` of a `tool_use` start, as the API sent it.
#[derive(Debug, Deserialize)]
struct ToolUseStart<'a> {
    #[serde(borrow, default)]
    input: Option<&'a RawValue>,
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
    /// API reported a failure, or sent data that does not fit the stream.
    pub(crate) fn map(&mut self, event: &SseEvent) -> Result<Vec<ProviderEvent>, ProviderError> {
        if self.done {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let result = self.map_event(event, &mut out);
        match result {
            Ok(()) => Ok(out),
            Err(error) => {
                self.done = true;
                Err(error)
            }
        }
    }

    fn map_event(
        &mut self,
        event: &SseEvent,
        out: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let value: Value =
            serde_json::from_str(&event.data).map_err(|source| ProviderError::Decode { source })?;
        let kind = value.get("type").and_then(Value::as_str).unwrap_or(event.event.as_str());
        match kind {
            "message_start" => self.message_start(&value),
            "content_block_start" => self.block_start(&event.data, &value, out)?,
            "content_block_delta" => self.block_delta(&value, out),
            "content_block_stop" => self.block_stop(&value, out)?,
            "message_delta" => {
                if let Some(reason) = value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = Some(reason.to_owned());
                }
                if let Some(usage) = value.get("usage") {
                    self.usage.update(usage);
                }
            }
            "message_stop" => self.message_stop(out)?,
            "ping" => tracing::trace!("a ping of the Messages stream"),
            "error" => return Err(stream_error(value.get("error"))),
            other => {
                tracing::debug!(event_type = %shown(other), "passed on an unknown Messages event");
                out.push(ProviderEvent::Raw(value.clone()));
            }
        }
        Ok(())
    }

    fn message_start(&mut self, event: &Value) {
        let message = event.get("message");
        if let Some(usage) = message.and_then(|message| message.get("usage")) {
            self.usage.start(usage);
        }
        let dropped = message
            .and_then(|message| message.get("input_transformations"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        // NOTE: with an append-only history, `drop_block` drops nothing outside a
        // pruning or a compaction, so every entry here may point to a bug.
        for entry in dropped {
            tracing::warn!(transformation = %entry, "the API dropped a block of the history");
        }
    }

    fn block_start(
        &mut self,
        data: &str,
        event: &Value,
        out: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let index = event_index(event)?;
        if self.blocks.iter().any(|block| block.index == index) {
            return Err(ProviderError::InvalidStream { problem: "a content block started twice" });
        }
        let raw = serde_json::from_str::<BlockStart<'_>>(data)
            .map_err(|source| ProviderError::Decode { source })?
            .content_block;
        let block = event.get("content_block").unwrap_or(&Value::Null);
        let text = |key: &str| block.get(key).and_then(Value::as_str);
        let kind = match text("type").unwrap_or_default() {
            "text" => {
                let text = text("text").unwrap_or_default().to_owned();
                self.push_text(&text, out);
                Kind::Text { text }
            }
            "thinking" => {
                let thinking = text("thinking").unwrap_or_default().to_owned();
                self.push_reasoning(index, &thinking, out);
                Kind::Thinking {
                    thinking,
                    signature: text("signature").unwrap_or_default().to_owned(),
                }
            }
            "tool_use" => {
                let (Some(id), Some(name)) = (text("id"), text("name")) else {
                    return Err(ProviderError::InvalidStream {
                        problem: "a tool call started without an id or a name",
                    });
                };
                let start = serde_json::from_str::<ToolUseStart<'_>>(raw.get())
                    .ok()
                    .and_then(|start| start.input)
                    .map(RawValue::to_owned);
                self.last = LastDelta::Other;
                out.push(ProviderEvent::ToolCallStart {
                    call_id: id.to_owned(),
                    name: name.to_owned(),
                    freeform: false,
                });
                Kind::ToolUse {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    start,
                    joined: String::new(),
                    input: None,
                }
            }
            "redacted_thinking" => Kind::Verbatim(raw.to_owned()),
            other => {
                tracing::debug!(block_type = %shown(other), "kept a content block of an unknown type as it came");
                out.push(ProviderEvent::Raw(block.clone()));
                Kind::Verbatim(raw.to_owned())
            }
        };
        self.blocks.push(Block { index, kind, stopped: false });
        Ok(())
    }

    fn block_delta(&mut self, event: &Value, out: &mut Vec<ProviderEvent>) {
        let Some(index) = event.get("index").and_then(Value::as_u64) else {
            tracing::debug!("a content block delta without an index; left out");
            return;
        };
        let delta = event.get("delta").unwrap_or(&Value::Null);
        let text = |key: &str| delta.get(key).and_then(Value::as_str).unwrap_or_default();
        let kind = delta.get("type").and_then(Value::as_str).unwrap_or_default();
        let Some(position) =
            self.blocks.iter().position(|block| block.index == index && !block.stopped)
        else {
            tracing::debug!(delta_type = %shown(kind), "a delta for a block that is not open; left out");
            return;
        };
        let mut reasoning = None;
        let mut new_text = None;
        match (kind, self.blocks.get_mut(position).map(|block| &mut block.kind)) {
            ("text_delta", Some(Kind::Text { text: block })) => {
                block.push_str(text("text"));
                new_text = Some(text("text"));
            }
            ("thinking_delta", Some(Kind::Thinking { thinking, .. })) => {
                thinking.push_str(text("thinking"));
                reasoning = Some(text("thinking"));
            }
            ("signature_delta", Some(Kind::Thinking { signature, .. })) => {
                signature.push_str(text("signature"));
            }
            ("input_json_delta", Some(Kind::ToolUse { id, joined, .. })) => {
                let fragment = text("partial_json");
                joined.push_str(fragment);
                if !fragment.is_empty() {
                    out.push(ProviderEvent::ToolCallDelta {
                        call_id: id.clone(),
                        arguments: fragment.to_owned(),
                    });
                }
            }
            ("citations_delta", _) => tracing::trace!("a citation; efr sends no documents"),
            (_, Some(Kind::Verbatim(_))) => {
                tracing::debug!(delta_type = %shown(kind), "a delta for a block kept as it came; left out");
            }
            _ => {
                tracing::debug!(delta_type = %shown(kind), "passed on an unknown content block delta");
                out.push(ProviderEvent::Raw(event.clone()));
            }
        }
        if let Some(text) = new_text {
            self.push_text(text, out);
        }
        if let Some(text) = reasoning {
            self.push_reasoning(index, text, out);
        }
    }

    fn push_text(&mut self, text: &str, out: &mut Vec<ProviderEvent>) {
        if !text.is_empty() {
            self.last = LastDelta::Other;
            out.push(ProviderEvent::TextDelta { text: text.to_owned() });
        }
    }

    /// Sends thinking text, after a blank line when it starts a new block right after
    /// another block's thinking.
    fn push_reasoning(&mut self, index: u64, text: &str, out: &mut Vec<ProviderEvent>) {
        if text.is_empty() {
            return;
        }
        let text = match self.last {
            LastDelta::Reasoning(last) if last != index => format!("\n\n{text}"),
            _ => text.to_owned(),
        };
        self.last = LastDelta::Reasoning(index);
        out.push(ProviderEvent::ReasoningDelta { text });
    }

    fn block_stop(
        &mut self,
        event: &Value,
        out: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let index = event_index(event)?;
        let Some(block) = self.blocks.iter_mut().find(|block| block.index == index) else {
            return Err(ProviderError::InvalidStream {
                problem: "a content block stopped that never started",
            });
        };
        if block.stopped {
            return Ok(());
        }
        block.stopped = true;
        if let Kind::ToolUse { id, start, joined, input, .. } = &mut block.kind {
            let text = if joined.is_empty() {
                start.as_ref().map_or(EMPTY_INPUT, |start| start.get())
            } else {
                joined.as_str()
            };
            // NOTE: a call whose input is not an object never runs: an answer cut off
            // by the output limit may stop inside it, and a prefix of a command can be
            // a different command.
            if serde_json::from_str::<Map<String, Value>>(text).is_err() {
                return Err(ProviderError::InvalidStream {
                    problem: "a tool call's input is not a JSON object",
                });
            }
            let raw = RawValue::from_string(text.to_owned())
                .map_err(|source| ProviderError::Decode { source })?;
            out.push(ProviderEvent::ToolCallEnd {
                call_id: id.clone(),
                arguments: text.to_owned(),
            });
            *input = Some(raw);
        }
        Ok(())
    }

    fn message_stop(&mut self, out: &mut Vec<ProviderEvent>) -> Result<(), ProviderError> {
        let calls = self.blocks.iter().filter(|block| matches!(block.kind, Kind::ToolUse { .. }));
        if calls.clone().any(|block| !block.stopped) {
            return Err(ProviderError::InvalidStream {
                problem: "the answer stopped inside a tool call",
            });
        }
        let has_calls = calls.count() > 0;
        let stop_reason = match self.stop_reason.as_deref() {
            Some("end_turn" | "stop_sequence") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("refusal") => StopReason::ContentFilter,
            Some("model_context_window_exceeded") => {
                tracing::warn!("the answer reached the end of the model's context window");
                StopReason::MaxTokens
            }
            Some("pause_turn") => {
                tracing::warn!("the API paused a turn, which only server tools do");
                StopReason::EndTurn
            }
            other => {
                tracing::warn!(stop_reason = ?other.map(shown), "an answer without a known stop reason");
                if has_calls { StopReason::ToolUse } else { StopReason::EndTurn }
            }
        };
        if !self.usage.is_empty() {
            out.push(ProviderEvent::Usage(self.usage.token_usage()));
        }
        let provider_raw = self.provider_raw()?;
        out.push(ProviderEvent::Done { stop_reason, provider_raw });
        self.done = true;
        Ok(())
    }

    /// The content array of the answer as one JSON string, or nothing for an answer
    /// without blocks.
    fn provider_raw(&self) -> Result<Option<Value>, ProviderError> {
        if self.blocks.is_empty() {
            return Ok(None);
        }
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            blocks.push(match &block.kind {
                Kind::Text { text } => RawBlock::Built(BuiltBlock::Text { text }),
                Kind::Thinking { thinking, signature } => {
                    RawBlock::Built(BuiltBlock::Thinking { thinking, signature })
                }
                Kind::ToolUse { id, name, input: Some(input), .. } => {
                    RawBlock::Built(BuiltBlock::ToolUse { id, name, input })
                }
                Kind::ToolUse { input: None, .. } => {
                    return Err(ProviderError::InvalidStream {
                        problem: "a tool call never ended",
                    });
                }
                Kind::Verbatim(raw) => RawBlock::Verbatim(raw),
            });
        }
        let text =
            serde_json::to_string(&blocks).map_err(|source| ProviderError::Decode { source })?;
        Ok(Some(Value::String(text)))
    }
}

/// The `index` of a block event.
fn event_index(event: &Value) -> Result<u64, ProviderError> {
    event
        .get("index")
        .and_then(Value::as_u64)
        .ok_or(ProviderError::InvalidStream { problem: "a content block event without an index" })
}

/// A type name from the stream, cut to [`MAX_LOGGED_TYPE`] characters for the log.
fn shown(kind: &str) -> String {
    kind.chars().take(MAX_LOGGED_TYPE).collect()
}

#[cfg(test)]
mod tests;
