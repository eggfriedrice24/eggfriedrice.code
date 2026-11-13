//! The stream of a Messages call to canonical provider events: a pure state machine
//! over the events that `efr_http::SseDecoder` parses.
//!
//! [`EventMapper`] reads the events of one answer in order, so the fixtures in
//! `fixtures/anthropic/messages/` test it without a server.
//!
//! | Event | What efr does |
//! |---|---|
//! | `message_start` | keeps `message.usage` as the base, and logs each entry of `message.input_transformations` (a block that `drop_block` dropped) at `warn` |
//! | `content_block_start` `text` or `thinking` | starts the block |
//! | `content_block_start` `redacted_thinking` | keeps the block as it came for `provider_raw`, no event |
//! | `content_block_start` `tool_use` | `ToolCallStart { call_id, name }` |
//! | `content_block_start` `server_tool_use` of `web_search` or `web_fetch` | starts the block, no event: the input comes in deltas, and the turn never runs such a call |
//! | `content_block_start` `web_search_tool_result` | keeps the block as it came; `HostedCallEnd` for its `tool_use_id`: a list of results is `Done` with the `url` and `title` of each (never `encrypted_content`), an error object is `Failed` (see below) |
//! | `content_block_start` `web_fetch_tool_result` | keeps the block as it came; `HostedCallEnd` with `Open`: `Done` without results, or `Failed` |
//! | `content_block_delta` `text_delta` | `TextDelta`; in a request with the web tools, the trailing run of white space and `. , ; : ! ?` waits for the next delta of the block or for its stop |
//! | `content_block_delta` `thinking_delta` | `ReasoningDelta` when it is not empty (a summary under `display: "summarized"`), after a blank line when a new thinking block follows another |
//! | `content_block_delta` `signature_delta` | kept for `provider_raw` only |
//! | `content_block_delta` `input_json_delta` | `ToolCallDelta` for a `tool_use`; the fragments join; for a `server_tool_use` the fragments only join |
//! | `content_block_delta` `citations_delta` | the citation, as it came, joins the `citations` of its text block |
//! | `content_block_stop` of a `tool_use` | `ToolCallEnd` with the joined input; an empty input is the start's `input`, else `{}`; an input that is not a JSON object fails the stream |
//! | `content_block_stop` of a `server_tool_use` | `HostedCallStart` with `Search { query }` (`web_search`, from `input.query`) or `Open { url }` (`web_fetch`, from `input.url`); an input that is not a JSON object gives the action without its subject and never fails the stream |
//! | `content_block_stop` of a `text` block | one `Citation { placed: true }` for each `web_search_result_location` of the block, in order, then the held text |
//! | `message_delta` | overwrites each usage member that it sends (the counts are cumulative) and keeps `stop_reason` |
//! | `message_stop` | stops each text and server tool block that is still open, then one `Usage` (see `usage`), a `debug` line with `usage.server_tool_use`, and `Done { stop_reason, provider_raw }` |
//! | `ping` | nothing |
//! | `error` | the error of its `error.type` (see `failure`); the request is never sent again |
//! | any other event, block or delta type | `Raw`, never a failure: the API adds types within a version |
//!
//! The error codes of a result: `too_many_requests` is `TooManyRequests`,
//! `max_uses_exceeded` `LimitReached`, `unavailable` `Unavailable`, `invalid_tool_input`,
//! `query_too_long`, `request_too_large` and `url_too_long` `Invalid`, `url_not_allowed`
//! and `url_not_in_prior_context` `NotAllowed`, `url_not_accessible` `NotAccessible`,
//! `unsupported_content_type` and `content_too_large` `Unsupported`, any other `Other`.
//! A result whose call started in an earlier answer (a search that waited for the
//! results of client tools) still gives its `HostedCallEnd`, with `Search` without a
//! query or `Open` with the result's `url`. An answer that ends with a `server_tool_use`
//! without its result (`pause_turn`, or `tool_use` together with a client call) is no
//! error.
//!
//! `provider_raw` is the content array of the assistant message as one JSON string:
//! the text that efr built from the stream, with every `thinking`, `redacted_thinking`,
//! `text`, `tool_use`, `server_tool_use` and result block, so the next request sends it
//! back byte for byte. efr writes the `text`, `thinking`, `tool_use` and
//! `server_tool_use` blocks itself, in the API's member order; a `tool_use` and a
//! `server_tool_use` keep their input as the text that the model wrote (a
//! `server_tool_use` whose input is not a JSON object gets `{}`), a `text` block keeps
//! each citation as the API sent it in `citations` (left out when none came), and a
//! `redacted_thinking` block, a result block, or a block of a type that efr does not
//! know, the text that the API sent. The held text of a block is never left out of
//! `provider_raw`: only its event waits. An empty `thinking` block stays, because its
//! signature counts. An empty `text` block is left out: the API refuses one in a
//! request, and the block would go back in every later request of the conversation. An
//! answer without blocks has no `provider_raw`.
//!
//! Stop reasons: `end_turn` and `stop_sequence` are `EndTurn`, `tool_use` is
//! `ToolUse`, `max_tokens` is `MaxTokens`, `refusal` is `ContentFilter` (the turn runs
//! none of its tools), `model_context_window_exceeded` is `MaxTokens` with a `warn`
//! line, and `pause_turn` (the server ran its tools for as long as it may in one
//! request) is `PauseTurn`, which the turn continues. A missing or unknown stop reason
//! is `ToolUse` when the answer has a tool call, else `EndTurn`. A web step never makes
//! an answer `ToolUse`.

use std::borrow::Cow;
use std::mem;

use efr_http::SseEvent;
use efr_provider::{
    HostedAction, HostedError, HostedOutcome, ProviderError, ProviderEvent, StopReason, WebResult,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::anthropic::failure;
use crate::anthropic::usage::StreamUsage;

/// The longest event or block type the debug log shows, so a hostile stream cannot
/// flood it.
const MAX_LOGGED_TYPE: usize = 128;

/// The input of a `tool_use` whose start and deltas gave none.
const EMPTY_INPUT: &str = "{}";

/// The citation type of a web search result.
const WEB_SEARCH_CITATION: &str = "web_search_result_location";

/// Maps the events of one answer, in order.
#[derive(Debug, Default)]
pub(crate) struct EventMapper {
    blocks: Vec<Block>,
    usage: StreamUsage,
    stop_reason: Option<String>,
    last: LastDelta,
    done: bool,
    /// True when the request carries the web tools: the trailing punctuation of a text
    /// block then waits, so the mark of a citation can stand before it.
    hold: bool,
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
        /// The citations of the block, each as the API sent it.
        citations: Vec<Box<RawValue>>,
        /// The end of `text` that no `TextDelta` has sent yet.
        held: String,
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
    /// A `server_tool_use` of the web search or the web fetch tool.
    ServerTool {
        id: String,
        tool: WebTool,
        /// The `input` of `content_block_start`, as it came.
        start: Option<Box<RawValue>>,
        /// The joined `partial_json` fragments.
        joined: String,
        /// The input and the action at `content_block_stop`.
        stopped: Option<(Box<RawValue>, HostedAction)>,
        /// The members of the start block other than `type`, `id`, `name` and `input`,
        /// such as `caller`, each as the API sent it: the block goes back with them.
        extra: Vec<(String, Box<RawValue>)>,
    },
    /// A `redacted_thinking` block, a result block of a server tool, or a block of a
    /// type that efr does not know, kept as the API sent it.
    Verbatim(Box<RawValue>),
}

/// The web tools of the API that efr sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebTool {
    /// `web_search`.
    Search,
    /// `web_fetch`.
    Fetch,
}

impl WebTool {
    fn of(name: &str) -> Option<Self> {
        match name {
            "web_search" => Some(WebTool::Search),
            "web_fetch" => Some(WebTool::Fetch),
            _ => None,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            WebTool::Search => "web_search",
            WebTool::Fetch => "web_fetch",
        }
    }
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
    /// A block that efr wrote whole, such as a `server_tool_use` with members beyond
    /// the four that `BuiltBlock` knows.
    Owned(Box<RawValue>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BuiltBlock<'a> {
    Text {
        text: &'a str,
        #[serde(skip_serializing_if = "no_citations")]
        citations: &'a [Box<RawValue>],
    },
    Thinking {
        thinking: &'a str,
        signature: &'a str,
    },
    ToolUse {
        id: &'a str,
        name: &'a str,
        input: &'a RawValue,
    },
    ServerToolUse {
        id: &'a str,
        name: &'a str,
        input: &'a RawValue,
    },
}

/// The members of the `content_block` `raw` of a `server_tool_use` start other than the
/// four that efr writes itself, each as it came, in the API's order. The docs say to
/// send the blocks back exactly as they came; efr writes `input` from the joined
/// deltas, so it keeps the rest here.
fn extra_members(raw: &RawValue) -> Vec<(String, Box<RawValue>)> {
    let members = serde_json::from_str::<Members>(raw.get()).map(|members| members.0);
    members
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| !matches!(key.as_str(), "type" | "id" | "name" | "input"))
        .collect()
}

/// The members of a JSON object in their order, each value as it came.
struct Members(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Members {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = Members;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Members, A::Error> {
                let mut members = Vec::new();
                while let Some(member) = map.next_entry::<String, Box<RawValue>>()? {
                    members.push(member);
                }
                Ok(Members(members))
            }
        }
        deserializer.deserialize_map(Visit)
    }
}

/// The `server_tool_use` block `{type, id, name, input, ...extra}` as JSON text.
fn server_tool_use(
    id: &str,
    name: &str,
    input: &RawValue,
    extra: &[(String, Box<RawValue>)],
) -> Result<Box<RawValue>, ProviderError> {
    let head = serde_json::to_string(&BuiltBlock::ServerToolUse { id, name, input })
        .map_err(|source| ProviderError::Decode { source })?;
    let mut text = head.strip_suffix('}').unwrap_or(&head).to_owned();
    for (key, value) in extra {
        let key = serde_json::to_string(key).map_err(|source| ProviderError::Decode { source })?;
        text.push(',');
        text.push_str(&key);
        text.push(':');
        text.push_str(value.get());
    }
    text.push('}');
    RawValue::from_string(text).map_err(|source| ProviderError::Decode { source })
}

/// True for a text block without citations, which goes back as it did before web search.
fn no_citations(citations: &&[Box<RawValue>]) -> bool {
    citations.is_empty()
}

/// The `content_block` of a `content_block_start`, as the API sent it.
#[derive(Debug, Deserialize)]
struct BlockStart<'a> {
    #[serde(borrow)]
    content_block: &'a RawValue,
}

/// The `input` of a `tool_use` or `server_tool_use` start, as the API sent it.
#[derive(Debug, Deserialize)]
struct ToolUseStart<'a> {
    #[serde(borrow, default)]
    input: Option<&'a RawValue>,
}

/// The citation of a `citations_delta`, as the API sent it.
#[derive(Debug, Deserialize)]
struct CitationDelta<'a> {
    #[serde(borrow)]
    delta: CitationOf<'a>,
}

#[derive(Debug, Deserialize)]
struct CitationOf<'a> {
    #[serde(borrow)]
    citation: &'a RawValue,
}

/// The members of a citation that the turn needs.
#[derive(Debug, Deserialize)]
struct CitationHead {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

impl EventMapper {
    pub(crate) fn new() -> Self {
        EventMapper::default()
    }

    /// The mapper of an answer to a request that carries the web tools when `web` is
    /// true: the trailing white space and punctuation of each text block then waits for
    /// the next text of the block or for its stop, so a citation at the stop comes
    /// before it.
    pub(crate) fn with_web_tools(mut self, web: bool) -> Self {
        self.hold = web;
        self
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

    /// The text that every block still holds back, as `TextDelta`s, once: the provider
    /// sends it before the error of an answer that failed or ended early, so every
    /// character of the stream reaches the turn exactly once.
    pub(crate) fn take_held(&mut self) -> Vec<ProviderEvent> {
        let mut out = Vec::new();
        for block in &mut self.blocks {
            if let Kind::Text { held, .. } = &mut block.kind
                && !held.is_empty()
            {
                out.push(ProviderEvent::TextDelta { text: mem::take(held) });
            }
        }
        out
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
            "content_block_delta" => self.block_delta(&event.data, &value, out),
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
            "error" => return Err(failure::event(&value)),
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
        let start_input = || {
            serde_json::from_str::<ToolUseStart<'_>>(raw.get())
                .ok()
                .and_then(|start| start.input)
                .map(RawValue::to_owned)
        };
        let kind = match text("type").unwrap_or_default() {
            "text" => {
                let mut held = String::new();
                let first = text("text").unwrap_or_default();
                let now = release(self.hold, &mut held, first);
                self.push_text(now, out);
                Kind::Text { text: first.to_owned(), citations: Vec::new(), held }
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
                self.last = LastDelta::Other;
                out.push(ProviderEvent::ToolCallStart {
                    call_id: id.to_owned(),
                    name: name.to_owned(),
                    freeform: false,
                });
                Kind::ToolUse {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    start: start_input(),
                    joined: String::new(),
                    input: None,
                }
            }
            "server_tool_use" => match (text("id"), text("name").and_then(WebTool::of)) {
                (Some(id), Some(tool)) => Kind::ServerTool {
                    id: id.to_owned(),
                    tool,
                    start: start_input(),
                    joined: String::new(),
                    stopped: None,
                    extra: extra_members(raw),
                },
                _ => {
                    tracing::debug!("kept a server tool call of another tool as it came");
                    out.push(ProviderEvent::Raw(block.clone()));
                    Kind::Verbatim(raw.to_owned())
                }
            },
            kind @ ("web_search_tool_result" | "web_fetch_tool_result") => {
                let tool =
                    if kind == "web_search_tool_result" { WebTool::Search } else { WebTool::Fetch };
                match self.hosted_end(tool, block) {
                    Some(end) => {
                        self.last = LastDelta::Other;
                        out.push(end);
                    }
                    None => tracing::debug!("a result of a server tool without a tool_use_id"),
                }
                Kind::Verbatim(raw.to_owned())
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

    /// The `HostedCallEnd` of a result block of `tool`, or nothing for a block without
    /// a `tool_use_id`. The action is the complete one of the call's start in this
    /// answer, else the one that the result tells.
    fn hosted_end(&self, tool: WebTool, block: &Value) -> Option<ProviderEvent> {
        let call_id = block.get("tool_use_id").and_then(Value::as_str)?;
        let content = block.get("content").unwrap_or(&Value::Null);
        let page = content.get("url").and_then(Value::as_str).map(str::to_owned);
        let started = self.blocks.iter().find_map(|block| match &block.kind {
            Kind::ServerTool { id, stopped: Some((_, action)), .. } if id == call_id => {
                Some(action.clone())
            }
            _ => None,
        });
        let action = match (started, tool) {
            (Some(HostedAction::Open { url: None }), WebTool::Fetch) => {
                HostedAction::Open { url: page }
            }
            (Some(action), _) => action,
            (None, WebTool::Search) => HostedAction::Search { query: None },
            (None, WebTool::Fetch) => HostedAction::Open { url: page },
        };
        let error = content.get("error_code").and_then(Value::as_str);
        let outcome = match (error, tool, content.as_array()) {
            (Some(code), tool, _) => HostedOutcome::Failed { error: error_of(tool, code) },
            (None, WebTool::Search, Some(entries)) => HostedOutcome::Done {
                results: Some(
                    entries
                        .iter()
                        .filter_map(|entry| {
                            let url = entry.get("url").and_then(Value::as_str)?;
                            let title = entry.get("title").and_then(Value::as_str);
                            Some(WebResult::new(url, title.map(str::to_owned)))
                        })
                        .collect(),
                ),
            },
            (None, _, _) => HostedOutcome::Done { results: None },
        };
        Some(ProviderEvent::HostedCallEnd { call_id: call_id.to_owned(), action, outcome })
    }

    fn block_delta(&mut self, data: &str, event: &Value, out: &mut Vec<ProviderEvent>) {
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
        let hold = self.hold;
        let mut reasoning = None;
        let mut new_text = None;
        match (kind, self.blocks.get_mut(position).map(|block| &mut block.kind)) {
            ("text_delta", Some(Kind::Text { text: block, held, .. })) => {
                let fragment = text("text");
                block.push_str(fragment);
                new_text = Some(release(hold, held, fragment));
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
            ("input_json_delta", Some(Kind::ServerTool { joined, .. })) => {
                joined.push_str(text("partial_json"));
            }
            ("citations_delta", Some(Kind::Text { citations, .. })) => {
                match serde_json::from_str::<CitationDelta<'_>>(data) {
                    Ok(cited) => citations.push(cited.delta.citation.to_owned()),
                    Err(_) => tracing::debug!("a citation delta without a citation; left out"),
                }
            }
            ("citations_delta", _) => tracing::debug!("a citation outside a text block; left out"),
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

    fn push_text(&mut self, text: Cow<'_, str>, out: &mut Vec<ProviderEvent>) {
        if !text.is_empty() {
            self.last = LastDelta::Other;
            out.push(ProviderEvent::TextDelta { text: text.into_owned() });
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
        let Some(position) = self.blocks.iter().position(|block| block.index == index) else {
            return Err(ProviderError::InvalidStream {
                problem: "a content block stopped that never started",
            });
        };
        self.stop_block(position, out)
    }

    /// Stops the block at `position` of `blocks`, once.
    fn stop_block(
        &mut self,
        position: usize,
        out: &mut Vec<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        let Some(block) = self.blocks.get_mut(position) else {
            return Ok(());
        };
        if block.stopped {
            return Ok(());
        }
        block.stopped = true;
        let mut events = Vec::new();
        match &mut block.kind {
            Kind::ToolUse { id, start, joined, input, .. } => {
                let text = if joined.is_empty() {
                    start.as_ref().map_or(EMPTY_INPUT, |start| start.get())
                } else {
                    joined.as_str()
                };
                // NOTE: a call whose input is not an object never runs: an answer cut
                // off by the output limit may stop inside it, and a prefix of a command
                // can be a different command.
                if serde_json::from_str::<Map<String, Value>>(text).is_err() {
                    return Err(ProviderError::InvalidStream {
                        problem: "a tool call's input is not a JSON object",
                    });
                }
                let raw = RawValue::from_string(text.to_owned())
                    .map_err(|source| ProviderError::Decode { source })?;
                events.push(ProviderEvent::ToolCallEnd {
                    call_id: id.clone(),
                    arguments: text.to_owned(),
                });
                *input = Some(raw);
            }
            Kind::ServerTool { id, tool, start, joined, stopped, .. } => {
                let text = if joined.is_empty() {
                    start.as_ref().map_or(EMPTY_INPUT, |start| start.get())
                } else {
                    joined.as_str()
                };
                // NOTE: the API runs the call, not efr, so an input that does not read
                // as an object only loses the subject of the row; `provider_raw` then
                // holds `{}`, and the replay drops such a call when it never ran.
                let object = serde_json::from_str::<Map<String, Value>>(text).ok();
                // NOTE: a blank query or URL names nothing, so the step has no subject
                // and the row says `web search` (or `web open`).
                let subject = |key: &str| {
                    object
                        .as_ref()
                        .and_then(|object| object.get(key))
                        .and_then(Value::as_str)
                        .filter(|subject| !subject.trim().is_empty())
                        .map(str::to_owned)
                };
                let action = match tool {
                    WebTool::Search => HostedAction::Search { query: subject("query") },
                    WebTool::Fetch => HostedAction::Open { url: subject("url") },
                };
                let kept = if object.is_some() { text } else { EMPTY_INPUT };
                let raw = RawValue::from_string(kept.to_owned())
                    .map_err(|source| ProviderError::Decode { source })?;
                events.push(ProviderEvent::HostedCallStart {
                    call_id: id.clone(),
                    action: action.clone(),
                });
                *stopped = Some((raw, action));
            }
            Kind::Text { citations, held, .. } => {
                for citation in citations.iter() {
                    if let Some(event) = citation_event(citation) {
                        events.push(event);
                    }
                }
                if !held.is_empty() {
                    events.push(ProviderEvent::TextDelta { text: mem::take(held) });
                }
            }
            Kind::Thinking { .. } | Kind::Verbatim(_) => {}
        }
        if !events.is_empty() {
            self.last = LastDelta::Other;
            out.extend(events);
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
        // NOTE: a text or server tool block without its stop still gives its citations,
        // its held text and its web step: the API's `message_stop` ends them all.
        for position in 0..self.blocks.len() {
            self.stop_block(position, out)?;
        }
        let stop_reason = match self.stop_reason.as_deref() {
            Some("end_turn" | "stop_sequence") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some("refusal") => StopReason::Refusal,
            Some("model_context_window_exceeded") => {
                tracing::warn!("the answer reached the end of the model's context window");
                StopReason::MaxTokens
            }
            Some("pause_turn") => {
                tracing::debug!("the API paused the turn of its server tools");
                StopReason::PauseTurn
            }
            other => {
                tracing::warn!(stop_reason = ?other.map(shown), "an answer without a known stop reason");
                if has_calls { StopReason::ToolUse } else { StopReason::EndTurn }
            }
        };
        if let Some((searches, fetches)) = self.usage.server_tools() {
            tracing::debug!(
                web_search_requests = searches,
                web_fetch_requests = fetches,
                "the server tools of the answer"
            );
        }
        if !self.usage.is_empty() {
            out.push(ProviderEvent::Usage(self.usage.token_usage()));
        }
        let provider_raw = self.provider_raw()?;
        out.push(ProviderEvent::Done { stop_reason, provider_raw });
        self.done = true;
        Ok(())
    }

    /// The content array of the answer as one JSON string, or nothing for an answer
    /// without blocks. An empty text block is left out.
    fn provider_raw(&self) -> Result<Option<Value>, ProviderError> {
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            blocks.push(match &block.kind {
                Kind::Text { text, .. } if text.is_empty() => continue,
                Kind::Text { text, citations, .. } => {
                    RawBlock::Built(BuiltBlock::Text { text, citations })
                }
                Kind::Thinking { thinking, signature } => {
                    RawBlock::Built(BuiltBlock::Thinking { thinking, signature })
                }
                Kind::ToolUse { id, name, input: Some(input), .. } => {
                    RawBlock::Built(BuiltBlock::ToolUse { id, name, input })
                }
                Kind::ToolUse { input: None, .. } | Kind::ServerTool { stopped: None, .. } => {
                    return Err(ProviderError::InvalidStream {
                        problem: "a tool call never ended",
                    });
                }
                Kind::ServerTool { id, tool, stopped: Some((input, _)), extra, .. }
                    if extra.is_empty() =>
                {
                    RawBlock::Built(BuiltBlock::ServerToolUse { id, name: tool.name(), input })
                }
                Kind::ServerTool { id, tool, stopped: Some((input, _)), extra, .. } => {
                    RawBlock::Owned(server_tool_use(id, tool.name(), input, extra)?)
                }
                Kind::Verbatim(raw) => RawBlock::Verbatim(raw),
            });
        }
        if blocks.is_empty() {
            return Ok(None);
        }
        let text =
            serde_json::to_string(&blocks).map_err(|source| ProviderError::Decode { source })?;
        Ok(Some(Value::String(text)))
    }
}

/// `fragment` with the held text before it, less the trailing run of white space and
/// `. , ; : ! ?`, which stays in `held` when `hold` is true. Without `hold`, all of it.
fn release<'a>(hold: bool, held: &mut String, fragment: &'a str) -> Cow<'a, str> {
    if !hold {
        return Cow::Borrowed(fragment);
    }
    held.push_str(fragment);
    let cut = held.trim_end_matches(is_held).len();
    let rest = held.split_off(cut);
    Cow::Owned(mem::replace(held, rest))
}

/// True for a character that waits at the end of a text block: a mark of a citation
/// stands before it.
fn is_held(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | ',' | ';' | ':' | '!' | '?')
}

/// The `Citation` of a citation of a text block, for a web search result only: a
/// citation of another type has no page to list.
fn citation_event(citation: &RawValue) -> Option<ProviderEvent> {
    let head = serde_json::from_str::<CitationHead>(citation.get()).ok()?;
    if head.kind != WEB_SEARCH_CITATION {
        tracing::debug!(citation_type = %shown(&head.kind), "a citation of another type; no event");
        return None;
    }
    let url = head.url?;
    Some(ProviderEvent::Citation { url, title: head.title, placed: true })
}

/// efr's reason for the `error_code` of a result block of `tool`.
fn error_of(tool: WebTool, code: &str) -> HostedError {
    match (tool, code) {
        (_, "too_many_requests") => HostedError::TooManyRequests,
        (_, "max_uses_exceeded") => HostedError::LimitReached,
        (_, "unavailable") => HostedError::Unavailable,
        (_, "invalid_tool_input")
        | (WebTool::Search, "query_too_long" | "request_too_large")
        | (WebTool::Fetch, "url_too_long") => HostedError::Invalid,
        (WebTool::Fetch, "url_not_allowed" | "url_not_in_prior_context") => HostedError::NotAllowed,
        (WebTool::Fetch, "url_not_accessible") => HostedError::NotAccessible,
        (WebTool::Fetch, "unsupported_content_type" | "content_too_large") => {
            HostedError::Unsupported
        }
        (_, other) => {
            tracing::debug!(error_code = %shown(other), "a server tool error of another code");
            HostedError::Other
        }
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
