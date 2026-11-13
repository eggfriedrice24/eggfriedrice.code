//! Canonical requests to the body of `POST /messages`, as pure functions.
//!
//! The body, routing members first:
//!
//! | Member | Value |
//! |---|---|
//! | `model` | `Request::model` |
//! | `stream` | `true` |
//! | `max_tokens` | `Request::max_output_tokens`, at most the model's `max_tokens` from the catalog, else that `max_tokens`; the API needs it |
//! | `system` | one `text` block with `Request::system`, left out when it is absent or empty |
//! | `tools` | `{name, description, input_schema}` in the request's order; a freeform tool goes in its function form; then, for a `HostedTool::WebSearch` of `Request::hosted`, the server tools `web_search_20250305` and `web_fetch_20250910` (see below); left out when there is no tool |
//! | `tool_choice` | `{"type": "auto"}`, never `any` or `tool`; left out when there is no tool |
//! | `thinking` | `{"type": "adaptive", "display": "summarized", "block_binding": {"prefix_mismatch_behavior": "drop_block"}}` |
//! | `output_config` | `{"effort": ..., "format": ...}`: `effort` is `Request::effort`, else the model's default effort (the catalog's [`DEFAULT_EFFORT`](crate::anthropic::DEFAULT_EFFORT) when the model lists it); `format` is `{"type": "json_schema", "schema": ...}` from `Request::output`; each is left out when nothing gives it, and the member when both are |
//! | `messages` | the canonical messages, by the rules below |
//!
//! Never sent: `temperature`, `top_p`, `top_k`, `stop_sequences`, `metadata`,
//! `service_tier`, `inference_geo`, and no key of `provider_options`. `block_binding`
//! needs the beta [`THINKING_BINDING_BETA`] in the `anthropic-beta` header.
//!
//! The web tools, after the client tools: `{"type": "web_search_20250305", "name":
//! "web_search", "max_uses": n}` and `{"type": "web_fetch_20250910", "name":
//! "web_fetch", "max_uses": n, "max_content_tokens": 25000, "url_sources":
//! {"client_tool_results": {"type": "none"}}}`, with `n` from `max_uses` (left out when
//! it is `None`). `url_sources` is left out when `urls_from_tool_results` is true: by
//! default a fetch then may not take a URL that only the output of one of efr's tools
//! holds. The access of the tool does not matter: the API has only the live web. Both
//! versions call the tool directly (no code execution) and need no beta. The request
//! has no other server tool, and a request without `hosted` has none at all.
//!
//! The messages:
//!
//! 1. An assistant message whose `provider_raw` this provider wrote (a JSON string that
//!    holds the content array, see `sse_events`) goes back as that exact text, through
//!    `serde_json::value::RawValue`. The conversation drops `provider_raw` when the
//!    model changes, so it only comes back to the model that wrote it. A raw `tool_use`
//!    of a tool that the request does not offer goes as a text block with
//!    [`unoffered_call_text`], the text that the conversation put in the canonical
//!    content, and the other blocks, the thinking blocks too, go back one by one, each
//!    exact. So the last assistant message of a tool loop keeps its thinking. A raw
//!    message that holds a step of the web tools (a `server_tool_use`, a
//!    `web_search_tool_result` or `web_fetch_tool_result` block, or a `text` block with
//!    `citations`) goes back only in a request that carries the web tools; any other
//!    request sends its canonical content, because the API refuses a waiting server
//!    tool call without its tool and the docs do not say that it takes the other blocks.
//! 2. Any other assistant message is built from its text and tool calls; its reasoning
//!    is dropped, because another model's thinking cannot go back. A tool call whose
//!    input is not a JSON object, such as a freeform call's text, goes as
//!    `{"input": <value>}`.
//! 3. Adjacent assistant messages merge into one, so the bytes depend only on the
//!    canonical history. The elements of a merged raw message stay exact. Adjacent
//!    user messages stay apart, such as a new prompt after a turn that ended on its
//!    tool results or on its prompt: the API joins them itself, and the user message
//!    that ended the request before keeps its bytes.
//! 4. In a user message, `tool_result` blocks come first, then any text and images. A
//!    result with an empty output has no `content`.
//! 5. A `tool_use` without a `tool_result` in the next message gets an `is_error`
//!    result with one fixed text. The conversation closes every call, so this is only a
//!    guard.
//! 6. Empty text blocks are dropped, also from a `provider_raw` (then the other
//!    blocks go back one by one, each exact), and so is a message that becomes empty.
//! 7. A tool id outside `[a-zA-Z0-9_-]` has each other character replaced by `_`.
//! 8. A `server_tool_use` block without its result block stays only where its call can
//!    still end: in the last message of the request (the continuation after a
//!    `pause_turn`), or when one user message of only tool results follows it and ends
//!    the request or comes before the assistant message that starts with the result (a
//!    search that waited for client tools). Anywhere else, such as a turn that an
//!    interrupt ended inside a search, the block is left out, the other blocks of its
//!    message go one by one, each exact, and a message that becomes empty is left out.
//! 9. A raw message with a result block of a server tool whose `server_tool_use` no
//!    earlier part of the request holds goes as its canonical content, such as the
//!    first answer of the tail that a compaction kept after a search that waited for
//!    client tools or a `pause_turn`: its call was in the answer before, which the
//!    summary replaced. The docs show only that the API refuses a call without its
//!    result, so efr sends no result without its call either.
//!
//! A request can end with an assistant message: the continuation of a `pause_turn`. The
//! marker of the tail then sits on the last user message, as in the request before.
//!
//! The prompt cache markers come from `breakpoints`, a pure function of the shape of
//! the request; the conversion puts each marker as the last member of the block that it
//! names. Two following requests of one conversation give the same bytes for `tools`,
//! `system` and every earlier message, apart from the markers.
//!
//! The provider calls [`request_body`] once per call and sends the body's
//! [`betas`](MessagesBody::betas) as the `anthropic-beta` header.

mod breakpoints;

use std::io;

use efr_provider::{
    ContentBlock, FREEFORM_INPUT, HostedTool, Message, ModelInfo, ProviderError, Request, Role,
    unoffered_call_text,
};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use self::breakpoints::{Layout, Shape, Target, Ttl, place_breakpoints, summary};
use crate::anthropic::AnthropicConfig;

/// The beta that turns on `thinking.block_binding`. Every request sends it in the
/// `anthropic-beta` header, because every body asks for `drop_block`.
pub(crate) const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The output of the result that the conversion adds for a tool call without one. It is
/// one fixed text, so the bytes of the request stay the same from call to call.
const UNANSWERED_CALL: &str = "The tool call has no result.";

/// How many bytes of a message's JSON the estimate counts as one token.
const BYTES_PER_TOKEN: u64 = 4;

/// The most tokens of text that one web fetch puts into the context. A fetched PDF is
/// not bound by it (the API applies it to text only).
pub(crate) const FETCH_MAX_CONTENT_TOKENS: u32 = 25_000;

/// The type and name of the web search tool (the basic version: direct calls only).
const WEB_SEARCH_TYPE: &str = "web_search_20250305";

/// The type and name of the web fetch tool.
const WEB_FETCH_TYPE: &str = "web_fetch_20250910";

/// The JSON body of one `POST /messages`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MessagesBody {
    model: String,
    stream: bool,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<SystemBlock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
    thinking: Thinking,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig>,
    messages: Vec<BodyMessage>,
}

impl MessagesBody {
    /// The values of the `anthropic-beta` header that the body needs, in order.
    pub(crate) fn betas(&self) -> &'static [&'static str] {
        &[THINKING_BINDING_BETA]
    }

    /// True when the body carries the web tools, so the stream holds back the trailing
    /// punctuation of a text block for the mark of a citation.
    pub(crate) fn carries_web_tools(&self) -> bool {
        self.tools.iter().any(|tool| matches!(tool, ToolEntry::Hosted(_)))
    }
}

/// `{"type": "ephemeral", "ttl": ...}`: a prompt cache marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
    ttl: &'static str,
}

impl CacheControl {
    fn new(ttl: Ttl) -> Self {
        CacheControl { kind: "ephemeral", ttl: ttl.as_str() }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize)]
struct Tool {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// One entry of `tools`: a tool that efr runs, or a server tool that the API runs.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum ToolEntry {
    Client(Tool),
    Hosted(ServerTool),
}

/// A server tool: the web search or the web fetch tool.
#[derive(Debug, Clone, Serialize)]
struct ServerTool {
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_uses: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_content_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url_sources: Option<UrlSources>,
}

/// `{"client_tool_results": {"type": "none"}}`: a fetch takes no URL that only the
/// result of a client tool holds.
#[derive(Debug, Clone, Copy, Serialize)]
struct UrlSources {
    client_tool_results: SourceFilter,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct SourceFilter {
    #[serde(rename = "type")]
    kind: &'static str,
}

/// The server tools for `hosted`, in order: the web search and the web fetch tool for
/// the first `HostedTool::WebSearch`, and nothing for any other entry.
fn server_tools(hosted: &[HostedTool]) -> Vec<ServerTool> {
    let Some((max_uses, urls_from_tool_results)) = hosted.iter().find_map(|tool| match tool {
        HostedTool::WebSearch { max_uses, urls_from_tool_results, .. } => {
            Some((*max_uses, *urls_from_tool_results))
        }
        _ => None,
    }) else {
        return Vec::new();
    };
    let url_sources = (!urls_from_tool_results)
        .then_some(UrlSources { client_tool_results: SourceFilter { kind: "none" } });
    vec![
        ServerTool {
            kind: WEB_SEARCH_TYPE,
            name: "web_search",
            max_uses,
            max_content_tokens: None,
            url_sources: None,
        },
        ServerTool {
            kind: WEB_FETCH_TYPE,
            name: "web_fetch",
            max_uses,
            max_content_tokens: Some(FETCH_MAX_CONTENT_TOKENS),
            url_sources,
        },
    ]
}

#[derive(Debug, Clone, Copy, Serialize)]
struct ToolChoice {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct Thinking {
    #[serde(rename = "type")]
    kind: &'static str,
    display: &'static str,
    block_binding: BlockBinding,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct BlockBinding {
    prefix_mismatch_behavior: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<OutputFormat>,
}

/// `{"type": "json_schema", "schema": ...}`: the answer is one JSON object that the
/// schema describes. The API takes no length keywords and needs
/// `additionalProperties: false` on every object, so the caller's schema must follow
/// both rules.
#[derive(Debug, Clone, Serialize)]
struct OutputFormat {
    #[serde(rename = "type")]
    kind: &'static str,
    schema: Value,
}

#[derive(Debug, Clone, Serialize)]
struct BodyMessage {
    role: &'static str,
    content: Content,
}

/// The content of one body message.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum Content {
    /// The exact content array of one assistant message that this provider wrote.
    Raw(Box<RawValue>),
    /// Blocks built or kept one by one.
    Parts(Vec<Part>),
}

/// One block of a body message.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum Part {
    /// One block of an assistant message that this provider wrote, exactly as it was.
    Raw(Box<RawValue>),
    /// A block built from the canonical content.
    Block(Block),
}

/// A block built from the canonical content. A marker is always its last member.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Debug, Clone, Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    kind: &'static str,
    media_type: String,
    data: String,
}

/// The body for `request` under `config`. `model` is what the catalog and the config
/// know about the request's model, `None` for a model that neither lists.
///
/// Fails with [`ProviderError::UnknownModel`] when neither the request nor the model
/// gives an output limit: the API needs `max_tokens`, and efr guesses no model fact.
pub(crate) fn request_body(
    request: &Request,
    config: &AnthropicConfig,
    model: Option<&ModelInfo>,
) -> Result<MessagesBody, ProviderError> {
    let cap = model.and_then(|model| model.max_output_tokens);
    let max_tokens = match (request.max_output_tokens, cap) {
        (Some(asked), Some(cap)) => asked.min(cap),
        (Some(limit), None) | (None, Some(limit)) => limit,
        (None, None) => return Err(ProviderError::UnknownModel { model: request.model.clone() }),
    };
    // NOTE: a model without efforts, such as one that the API lists with
    // `capabilities.effort.supported` false or one only the config names, gets no
    // `output_config`: the API refuses an effort that the model does not take.
    let effort =
        request.effort.clone().or_else(|| model.and_then(|model| model.default_effort.clone()));
    let mut system: Vec<SystemBlock> = request
        .system
        .iter()
        .filter(|system| !system.is_empty())
        .map(|system| SystemBlock { kind: "text", text: system.clone(), cache_control: None })
        .collect();
    let mut tools: Vec<ToolEntry> = request
        .tools
        .iter()
        .map(|tool| {
            ToolEntry::Client(Tool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
                cache_control: None,
            })
        })
        .collect();
    let client_tools = tools.len();
    tools.extend(server_tools(&request.hosted).into_iter().map(ToolEntry::Hosted));

    let mut shapes = Vec::with_capacity(request.messages.len());
    let mut messages = Vec::with_capacity(request.messages.len());
    let mut after_calls = false;
    for draft in drafts(request) {
        let role = draft.role;
        // NOTE: a user message right after the calls of an assistant message answers
        // them, also when it holds their results as text, so it opens no turn.
        let opens_turn = role == Role::User && draft.answers.is_empty() && !after_calls;
        after_calls = role == Role::Assistant && draft.called;
        let message = draft.into_message();
        shapes.push(Shape { role, opens_turn, tokens: estimate(&message) });
        messages.push(message);
    }

    let layout = Layout {
        system: !system.is_empty(),
        tools: client_tools > 0,
        messages: &shapes,
        side_call: request.side_call,
        one_shot: request.one_shot,
    };
    let marks = place_breakpoints(&layout, config.cache_ttl());
    let ttl = config.cache_ttl().as_str();
    let placed = summary(&marks);
    tracing::debug!(cache_ttl = ttl, markers = %placed, "cache_ttl={ttl} markers={placed}");
    for mark in marks {
        let control = Some(CacheControl::new(mark.ttl));
        let placed = match mark.target {
            Target::System => system.last_mut().map(|block| block.cache_control = control),
            // NOTE: the marker stays on the last client tool: the marker of the system
            // block covers the server tools after it, and the docs do not show a marker
            // on a server tool.
            Target::LastTool => tools.iter_mut().rev().find_map(|tool| match tool {
                ToolEntry::Client(tool) => {
                    tool.cache_control = control;
                    Some(())
                }
                ToolEntry::Hosted(_) => None,
            }),
            Target::Message(index) => {
                messages.get_mut(index).and_then(|message| message.mark(control))
            }
        };
        if placed.is_none() {
            tracing::debug!(slot = ?mark.slot, "a prompt cache marker found no block to sit on");
        }
    }

    let tool_choice = (!tools.is_empty()).then_some(ToolChoice { kind: "auto" });
    let format = request
        .output
        .as_ref()
        .map(|output| OutputFormat { kind: "json_schema", schema: output.schema.clone() });
    Ok(MessagesBody {
        model: request.model.clone(),
        stream: true,
        max_tokens,
        system,
        tools,
        tool_choice,
        thinking: Thinking {
            kind: "adaptive",
            display: "summarized",
            block_binding: BlockBinding { prefix_mismatch_behavior: "drop_block" },
        },
        output_config: (effort.is_some() || format.is_some())
            .then_some(OutputConfig { effort, format }),
        messages,
    })
}

/// One message of the body while the conversion merges.
#[derive(Debug)]
struct Draft {
    role: Role,
    /// The `provider_raw` text of the one assistant message that this draft holds,
    /// which goes back whole while nothing merges into it.
    whole: Option<Box<RawValue>>,
    /// The `tool_result` blocks of a user message, which come first.
    results: Vec<Part>,
    /// Every other block, in order.
    rest: Vec<Part>,
    /// The ids of the `tool_use` blocks of an assistant message.
    calls: Vec<String>,
    /// True for an assistant message that called a tool: a `tool_use` block, or a raw
    /// one that goes as text because the request does not offer its tool.
    called: bool,
    /// The ids that the `tool_result` blocks of a user message answer.
    answers: Vec<String>,
    /// The ids of the `server_tool_use` blocks of a raw assistant message.
    server_calls: Vec<String>,
    /// The `tool_use_id`s of the result blocks of server tools in a raw assistant
    /// message.
    server_results: Vec<String>,
}

impl Draft {
    fn new(role: Role) -> Self {
        Draft {
            role,
            whole: None,
            results: Vec::new(),
            rest: Vec::new(),
            calls: Vec::new(),
            called: false,
            answers: Vec::new(),
            server_calls: Vec::new(),
            server_results: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.results.is_empty() && self.rest.is_empty()
    }

    /// True for a user message that holds only `tool_result` blocks: it continues the
    /// assistant turn before it, and the API runs a waiting server tool call then.
    fn only_results(&self) -> bool {
        self.role == Role::User && !self.results.is_empty() && self.rest.is_empty()
    }

    /// The ids of the server tool calls of this message without a result block in it.
    fn open_server_calls(&self) -> impl Iterator<Item = &String> {
        self.server_calls.iter().filter(|id| !self.server_results.contains(id))
    }

    /// Leaves out the `server_tool_use` blocks of `ids`; the other blocks then go one by
    /// one, each exact.
    fn drop_server_calls(&mut self, ids: &[String]) {
        self.whole = None;
        self.rest.retain(|part| {
            let Part::Raw(raw) = part else {
                return true;
            };
            let Ok(head) = serde_json::from_str::<RawHead>(raw.get()) else {
                return true;
            };
            !(head.kind == "server_tool_use" && head.id.is_some_and(|id| ids.contains(&id)))
        });
        self.server_calls.retain(|id| !ids.contains(id));
    }

    /// Adds the blocks of `next`, a message of the same role that follows this one.
    fn absorb(&mut self, next: Draft) {
        self.whole = None;
        self.results.extend(next.results);
        self.rest.extend(next.rest);
        self.calls.extend(next.calls);
        self.called |= next.called;
        self.answers.extend(next.answers);
        self.server_calls.extend(next.server_calls);
        self.server_results.extend(next.server_results);
    }

    fn into_message(self) -> BodyMessage {
        let role = match self.role {
            Role::Assistant => "assistant",
            _ => "user",
        };
        let content = match self.whole {
            Some(whole) => Content::Raw(whole),
            None => Content::Parts(self.results.into_iter().chain(self.rest).collect()),
        };
        BodyMessage { role, content }
    }
}

impl BodyMessage {
    /// Puts `control` on the last block, when that block takes a marker.
    fn mark(&mut self, control: Option<CacheControl>) -> Option<()> {
        let Content::Parts(parts) = &mut self.content else {
            return None;
        };
        match parts.last_mut()? {
            Part::Block(
                Block::Text { cache_control, .. }
                | Block::Image { cache_control, .. }
                | Block::ToolResult { cache_control, .. },
            ) => {
                *cache_control = control;
                Some(())
            }
            Part::Block(Block::ToolUse { .. }) | Part::Raw(_) => None,
        }
    }
}

/// The body messages of `messages`: each converted, the empty ones left out, adjacent
/// assistant messages merged, and every open tool call answered. Adjacent user
/// messages stay apart: the API joins them, and a user message that ended the request
/// before keeps its bytes when a new one follows it.
fn drafts(request: &Request) -> Vec<Draft> {
    let messages = &request.messages;
    let mut drafts: Vec<Draft> = Vec::with_capacity(messages.len());
    let mut server_calls: Vec<String> = Vec::new();
    for message in messages {
        let draft = match message.role {
            Role::Assistant => match raw_draft(message, request) {
                Some(raw) if orphan_result(&raw, &server_calls) => {
                    tracing::debug!(
                        "a server tool result without its call in the request; sending the canonical content"
                    );
                    assistant_draft(message)
                }
                Some(raw) => {
                    server_calls.extend(raw.server_calls.iter().cloned());
                    raw
                }
                None => assistant_draft(message),
            },
            Role::User => user_draft(message),
            other => {
                tracing::debug!(role = ?other, "a role the Messages API does not know; sent as user");
                user_draft(message)
            }
        };
        if draft.is_empty() {
            continue;
        }
        match drafts.last_mut() {
            Some(last) if last.role == Role::Assistant && draft.role == Role::Assistant => {
                last.absorb(draft);
            }
            _ => drafts.push(draft),
        }
    }
    drop_stale_server_calls(&mut drafts);
    answer_open_calls(&mut drafts);
    drafts
}

/// True when a result block of a server tool in `draft` answers a call that neither
/// `draft` nor an earlier message of the request (`calls`) holds (rule 9 of the module
/// doc), such as the start of the tail that a compaction kept: the call was in the
/// answer before, which the summary replaced.
fn orphan_result(draft: &Draft, calls: &[String]) -> bool {
    draft.server_results.iter().any(|id| !draft.server_calls.contains(id) && !calls.contains(id))
}

/// Leaves out each `server_tool_use` block whose call can no longer end (rule 8 of the
/// module doc), and a message that becomes empty.
fn drop_stale_server_calls(drafts: &mut Vec<Draft>) {
    let mut emptied = false;
    for index in 0..drafts.len() {
        let stale: Vec<String> = match drafts.get(index) {
            Some(draft) if draft.role == Role::Assistant => draft
                .open_server_calls()
                .filter(|id| !can_end(drafts, index, id))
                .cloned()
                .collect(),
            _ => continue,
        };
        if stale.is_empty() {
            continue;
        }
        tracing::debug!(calls = stale.len(), "server tool calls that can no longer end; left out");
        if let Some(draft) = drafts.get_mut(index) {
            draft.drop_server_calls(&stale);
            emptied |= draft.is_empty();
        }
    }
    if emptied {
        drafts.retain(|draft| !draft.is_empty());
    }
}

/// True when the server tool call `id` of the assistant message at `index` can still
/// end: the message is the last one, or one user message of only tool results follows
/// it and either ends the request or comes before the assistant message with the
/// call's result.
fn can_end(drafts: &[Draft], index: usize, id: &str) -> bool {
    let Some(next) = drafts.get(index + 1) else {
        return true;
    };
    if !next.only_results() {
        return false;
    }
    drafts.get(index + 2).is_none_or(|after| {
        after.role == Role::Assistant && after.server_results.iter().any(|result| result == id)
    })
}

/// The start of one raw block: enough to check that it is a block, to read the id, the
/// tool and the input of a `tool_use`, to find an empty `text` block and to find the
/// blocks of the web tools.
#[derive(Debug, Deserialize)]
struct RawHead {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    citations: Option<IgnoredAny>,
}

impl RawHead {
    /// True for a `text` block without text, which the API refuses.
    fn is_empty_text(&self) -> bool {
        self.kind == "text" && self.text.as_deref().is_none_or(str::is_empty)
    }

    /// True for a result block of a server tool, such as `web_search_tool_result`.
    fn is_server_result(&self) -> bool {
        self.kind.ends_with("_tool_result") && self.tool_use_id.is_some()
    }

    /// True for a block of a step of the web tools: a server tool call, its result, or
    /// a text block with citations.
    fn is_web(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "server_tool_use" | "web_search_tool_result" | "web_fetch_tool_result"
        ) || (self.kind == "text" && self.citations.is_some())
    }
}

/// The draft of an assistant message from its `provider_raw`, when that holds the
/// content array that this provider wrote: a JSON string of a non-empty array of
/// blocks. Anything else, such as another provider's items, falls back to the
/// canonical content and costs the signed thinking, not the request. So does a message
/// with a step of the web tools in a request without them. A `tool_use` of a tool that
/// `request` does not offer goes as its text.
fn raw_draft(message: &Message, request: &Request) -> Option<Draft> {
    let text = match message.provider_raw.as_ref()? {
        Value::String(text) => text,
        _ => {
            tracing::debug!(
                "provider_raw is not a Messages content text; sending the canonical content"
            );
            return None;
        }
    };
    let web = request.has_web_search();
    let mut without_web = false;
    let mut read = || -> Option<Draft> {
        let blocks: Vec<Box<RawValue>> = serde_json::from_str(text).ok()?;
        if blocks.is_empty() {
            return None;
        }
        let mut draft = Draft::new(Role::Assistant);
        let mut changed = false;
        for block in blocks {
            let head: RawHead = serde_json::from_str(block.get()).ok()?;
            if head.is_empty_text() {
                changed = true;
                continue;
            }
            if !web && head.is_web() {
                without_web = true;
                return None;
            }
            if head.kind == "server_tool_use" {
                draft.server_calls.push(head.id.clone()?);
            } else if head.is_server_result() {
                draft.server_results.extend(head.tool_use_id.clone());
            }
            if head.kind == "tool_use" {
                let id = head.id?;
                draft.called = true;
                let name = head.name.unwrap_or_default();
                if !request.offers(&name) {
                    let input = head.input.unwrap_or(Value::Null);
                    let text = unoffered_call_text(&id, &name, &input);
                    draft.rest.push(Part::Block(Block::Text { text, cache_control: None }));
                    changed = true;
                    continue;
                }
                draft.calls.push(id);
            }
            draft.rest.push(Part::Raw(block));
        }
        if !changed {
            draft.whole = Some(RawValue::from_string(text.clone()).ok()?);
        }
        Some(draft)
    };
    let draft = read();
    if without_web {
        tracing::debug!(
            "provider_raw holds a step of the web tools, which this request does not carry; sending the canonical content"
        );
    } else if draft.is_none() {
        tracing::debug!(
            "provider_raw is not a list of Messages blocks; sending the canonical content"
        );
    }
    draft
}

fn assistant_draft(message: &Message) -> Draft {
    let mut draft = Draft::new(Role::Assistant);
    for block in &message.content {
        match block {
            ContentBlock::Text { text } => push_text(&mut draft, text),
            ContentBlock::ToolCall { call_id, name, input, .. } => {
                let id = tool_id(call_id);
                draft.called = true;
                draft.calls.push(id.clone());
                draft.rest.push(Part::Block(Block::ToolUse {
                    id,
                    name: name.clone(),
                    input: tool_input(input),
                }));
            }
            ContentBlock::Reasoning { .. } => {}
            _ => tracing::debug!("a block that an assistant message cannot carry; left out"),
        }
    }
    draft
}

fn user_draft(message: &Message) -> Draft {
    let mut draft = Draft::new(Role::User);
    for block in &message.content {
        match block {
            ContentBlock::ToolResult { call_id, output, is_error } => {
                let id = tool_id(call_id);
                draft.answers.push(id.clone());
                draft.results.push(Part::Block(Block::ToolResult {
                    tool_use_id: id,
                    content: (!output.is_empty()).then(|| output.clone()),
                    is_error: *is_error,
                    cache_control: None,
                }));
            }
            ContentBlock::Text { text } => push_text(&mut draft, text),
            ContentBlock::Image { media_type, data } => {
                // The canonical form already serialises as standard base64.
                match serde_json::to_value(data) {
                    Ok(Value::String(data)) => draft.rest.push(Part::Block(Block::Image {
                        source: ImageSource {
                            kind: "base64",
                            media_type: media_type.clone(),
                            data,
                        },
                        cache_control: None,
                    })),
                    _ => tracing::debug!("an image whose bytes did not encode; left out"),
                }
            }
            _ => tracing::debug!("a block that a user message cannot carry; left out"),
        }
    }
    draft
}

/// Adds a text block, unless the text is empty: the API refuses an empty text block.
fn push_text(draft: &mut Draft, text: &str) {
    if !text.is_empty() {
        draft.rest.push(Part::Block(Block::Text { text: text.to_owned(), cache_control: None }));
    }
}

/// Adds an `is_error` result for each call of an assistant message that the next
/// message does not answer, and a user message for them when none follows.
fn answer_open_calls(drafts: &mut Vec<Draft>) {
    let mut index = 0;
    while index < drafts.len() {
        let next = index + 1;
        let open: Vec<String> = match drafts.get(index) {
            Some(draft) if draft.role == Role::Assistant => {
                let answered = drafts
                    .get(next)
                    .filter(|next| next.role == Role::User)
                    .map_or(&[][..], |next| next.answers.as_slice());
                draft.calls.iter().filter(|call| !answered.contains(call)).cloned().collect()
            }
            _ => Vec::new(),
        };
        if !open.is_empty() {
            tracing::debug!(calls = open.len(), "tool calls without a result; answered as failed");
            if drafts.get(next).is_none_or(|next| next.role != Role::User) {
                drafts.insert(next, Draft::new(Role::User));
            }
            if let Some(answer) = drafts.get_mut(next) {
                for id in open {
                    answer.answers.push(id.clone());
                    answer.results.push(Part::Block(Block::ToolResult {
                        tool_use_id: id,
                        content: Some(UNANSWERED_CALL.to_owned()),
                        is_error: true,
                        cache_control: None,
                    }));
                }
            }
        }
        index = next;
    }
}

/// The id of a tool call as the API takes it (`^[a-zA-Z0-9_-]+$`). A valid id goes as it
/// is. In another id, such as one that another provider wrote, every other character
/// becomes `_`, and `_` and 8 hex digits of the SHA-256 of the whole id follow, so two
/// ids that differ only in those characters stay apart. The call and its result go
/// through this one function, so they still match.
fn tool_id(id: &str) -> String {
    let valid = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if !id.is_empty() && id.chars().all(valid) {
        return id.to_owned();
    }
    let cleaned: String = id.chars().map(|c| if valid(c) { c } else { '_' }).collect();
    let digest = Sha256::digest(id.as_bytes());
    let suffix: String = digest.iter().take(4).map(|byte| format!("{byte:02x}")).collect();
    if cleaned.is_empty() { suffix } else { format!("{cleaned}_{suffix}") }
}

/// The input of a canonical tool call as a `tool_use` takes it, which is always an
/// object. Any other value, such as a freeform call's text or arguments that were not
/// JSON, goes in the one member that a freeform tool's function form has.
fn tool_input(input: &Value) -> Value {
    match input {
        Value::Object(_) => input.clone(),
        other => {
            let mut wrapped = Map::new();
            wrapped.insert(FREEFORM_INPUT.to_owned(), other.clone());
            Value::Object(wrapped)
        }
    }
}

/// The estimate of a message's tokens: its JSON bytes over [`BYTES_PER_TOKEN`], rounded
/// up. It is the same for the same bytes, which the anchors of the cache markers need.
fn estimate(message: &BodyMessage) -> u64 {
    let mut bytes = ByteCount(0);
    if serde_json::to_writer(&mut bytes, message).is_err() {
        return 0;
    }
    bytes.0.div_ceil(BYTES_PER_TOKEN)
}

/// A writer that only counts the bytes written to it.
#[derive(Debug)]
struct ByteCount(u64);

impl io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(u64::try_from(buf.len()).unwrap_or(u64::MAX));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
