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
//! | `tools` | `{name, description, input_schema}` in the request's order; a freeform tool goes in its function form; left out when there is no tool |
//! | `tool_choice` | `{"type": "auto"}`, never `any` or `tool`; left out when there is no tool |
//! | `thinking` | `{"type": "adaptive", "display": "summarized", "block_binding": {"prefix_mismatch_behavior": "drop_block"}}` |
//! | `output_config` | `{"effort": ...}`: `Request::effort`, else the model's default effort (the catalog's [`DEFAULT_EFFORT`](crate::DEFAULT_EFFORT) when the model lists it); left out when neither gives one |
//! | `messages` | the canonical messages, by the rules below |
//!
//! Never sent: `temperature`, `top_p`, `top_k`, `stop_sequences`, `metadata`,
//! `service_tier`, `inference_geo`, and no key of `provider_options`. `block_binding`
//! needs the beta [`THINKING_BINDING_BETA`] in the `anthropic-beta` header.
//!
//! The messages:
//!
//! 1. An assistant message whose `provider_raw` this provider wrote (a JSON string that
//!    holds the content array, see `sse_events`) goes back as that exact text, through
//!    `serde_json::value::RawValue`. The conversation drops `provider_raw` when the
//!    model changes, so it only comes back to the model that wrote it.
//! 2. Any other assistant message is built from its text and tool calls; its reasoning
//!    is dropped, because another model's thinking cannot go back. A tool call whose
//!    input is not a JSON object, such as a freeform call's text, goes as
//!    `{"input": <value>}`.
//! 3. Adjacent messages of one role merge into one, so the bytes depend only on the
//!    canonical history. The elements of a merged raw message stay exact.
//! 4. In a user message, `tool_result` blocks come first, then any text and images. A
//!    result with an empty output has no `content`.
//! 5. A `tool_use` without a `tool_result` in the next message gets an `is_error`
//!    result with one fixed text. The conversation closes every call, so this is only a
//!    guard.
//! 6. Empty text blocks are dropped, also from a `provider_raw` (then the other
//!    blocks go back one by one, each exact), and so is a message that becomes empty.
//! 7. A tool id outside `[a-zA-Z0-9_-]` has each other character replaced by `_`.
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
    ContentBlock, FREEFORM_INPUT, Message, ModelInfo, ProviderError, Request, Role,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use self::breakpoints::{Layout, Shape, Target, Ttl, place_breakpoints, summary};
use crate::AnthropicConfig;

/// The beta that turns on `thinking.block_binding`. Every request sends it in the
/// `anthropic-beta` header, because every body asks for `drop_block`.
pub(crate) const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The output of the result that the conversion adds for a tool call without one. It is
/// one fixed text, so the bytes of the request stay the same from call to call.
const UNANSWERED_CALL: &str = "The tool call has no result.";

/// How many bytes of a message's JSON the estimate counts as one token.
const BYTES_PER_TOKEN: u64 = 4;

/// The JSON body of one `POST /messages`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MessagesBody {
    model: String,
    stream: bool,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<SystemBlock>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
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
    effort: String,
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
    let mut tools: Vec<Tool> = request
        .tools
        .iter()
        .map(|tool| Tool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            cache_control: None,
        })
        .collect();

    let mut shapes = Vec::with_capacity(request.messages.len());
    let mut messages = Vec::with_capacity(request.messages.len());
    for draft in drafts(&request.messages) {
        let role = draft.role;
        let opens_turn = role == Role::User && draft.answers.is_empty();
        let message = draft.into_message();
        shapes.push(Shape { role, opens_turn, tokens: estimate(&message) });
        messages.push(message);
    }

    let layout = Layout {
        system: !system.is_empty(),
        tools: !tools.is_empty(),
        messages: &shapes,
        side_call: request.side_call,
    };
    let marks = place_breakpoints(&layout, config.cache_ttl());
    let ttl = config.cache_ttl().as_str();
    let placed = summary(&marks);
    tracing::debug!(cache_ttl = ttl, markers = %placed, "cache_ttl={ttl} markers={placed}");
    for mark in marks {
        let control = Some(CacheControl::new(mark.ttl));
        let placed = match mark.target {
            Target::System => system.last_mut().map(|block| block.cache_control = control),
            Target::LastTool => tools.last_mut().map(|tool| tool.cache_control = control),
            Target::Message(index) => {
                messages.get_mut(index).and_then(|message| message.mark(control))
            }
        };
        if placed.is_none() {
            tracing::debug!(slot = ?mark.slot, "a prompt cache marker found no block to sit on");
        }
    }

    let tool_choice = (!tools.is_empty()).then_some(ToolChoice { kind: "auto" });
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
        output_config: effort.map(|effort| OutputConfig { effort }),
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
    /// The ids that the `tool_result` blocks of a user message answer.
    answers: Vec<String>,
}

impl Draft {
    fn new(role: Role) -> Self {
        Draft {
            role,
            whole: None,
            results: Vec::new(),
            rest: Vec::new(),
            calls: Vec::new(),
            answers: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.results.is_empty() && self.rest.is_empty()
    }

    /// Adds the blocks of `next`, a message of the same role that follows this one.
    fn absorb(&mut self, next: Draft) {
        self.whole = None;
        self.results.extend(next.results);
        self.rest.extend(next.rest);
        self.calls.extend(next.calls);
        self.answers.extend(next.answers);
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
/// ones of one role merged, and every open tool call answered.
fn drafts(messages: &[Message]) -> Vec<Draft> {
    let mut drafts: Vec<Draft> = Vec::with_capacity(messages.len());
    for message in messages {
        let draft = match message.role {
            Role::Assistant => raw_draft(message).unwrap_or_else(|| assistant_draft(message)),
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
            Some(last) if last.role == draft.role => last.absorb(draft),
            _ => drafts.push(draft),
        }
    }
    answer_open_calls(&mut drafts);
    drafts
}

/// The start of one raw block: enough to check that it is a block, to read the id of a
/// `tool_use` and to find an empty `text` block.
#[derive(Debug, Deserialize)]
struct RawHead {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

impl RawHead {
    /// True for a `text` block without text, which the API refuses.
    fn is_empty_text(&self) -> bool {
        self.kind == "text" && self.text.as_deref().is_none_or(str::is_empty)
    }
}

/// The draft of an assistant message from its `provider_raw`, when that holds the
/// content array that this provider wrote: a JSON string of a non-empty array of
/// blocks. Anything else, such as another provider's items, falls back to the
/// canonical content and costs the signed thinking, not the request.
fn raw_draft(message: &Message) -> Option<Draft> {
    let text = match message.provider_raw.as_ref()? {
        Value::String(text) => text,
        _ => {
            tracing::debug!(
                "provider_raw is not a Messages content text; sending the canonical content"
            );
            return None;
        }
    };
    let read = || -> Option<Draft> {
        let blocks: Vec<Box<RawValue>> = serde_json::from_str(text).ok()?;
        if blocks.is_empty() {
            return None;
        }
        let mut draft = Draft::new(Role::Assistant);
        let mut dropped = false;
        for block in blocks {
            let head: RawHead = serde_json::from_str(block.get()).ok()?;
            if head.is_empty_text() {
                dropped = true;
                continue;
            }
            if head.kind == "tool_use" {
                draft.calls.push(head.id?);
            }
            draft.rest.push(Part::Raw(block));
        }
        if !dropped {
            draft.whole = Some(RawValue::from_string(text.clone()).ok()?);
        }
        Some(draft)
    };
    let draft = read();
    if draft.is_none() {
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

/// The id of a tool call as the API takes it: every character outside
/// `[a-zA-Z0-9_-]` becomes `_`, so an id that another provider wrote still matches its
/// result.
fn tool_id(id: &str) -> String {
    if id.is_empty() {
        return "_".to_owned();
    }
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
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
