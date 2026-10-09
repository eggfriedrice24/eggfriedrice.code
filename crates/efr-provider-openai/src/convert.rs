//! Canonical requests to the Responses API, and Responses output items back.
//!
//! The request shape follows Codex (`codex-rs/core/src/client.rs`,
//! `build_responses_request`, and `codex-rs/codex-api/src/common.rs`,
//! `ResponsesApiRequest`): `stream: true`, `store: false`, the system prompt as
//! `instructions`, `tool_choice: "auto"`, function tools with `strict: false`, and
//! `include: ["reasoning.encrypted_content"]` so that a stateless conversation can send
//! its reasoning back. The item shapes are Codex's `ResponseItem`
//! (`codex-rs/protocol/src/models.rs`) and goose's `build_input_items`
//! (`crates/goose/src/providers/chatgpt_codex.rs`).
//!
//! A freeform tool goes to a model that takes freeform tools as a `custom` tool with
//! its grammar (`codex-rs/tools/src/responses_api.rs`, `FreeformTool`, and
//! `codex-rs/core/src/tools/handlers/apply_patch_spec.rs`); the model answers with a
//! `custom_tool_call` whose `input` is the raw text, and the result goes back as a
//! `custom_tool_call_output`. Every other model gets the tool's function form, and so
//! do the freeform calls of its history that have no raw items, such as after a
//! change of the model.
//!
//! An assistant message that carries `provider_raw` is sent as those items, verbatim:
//! they are the exact `reasoning`, `message`, `function_call` and `custom_tool_call`
//! items this provider produced, in order, and the encrypted reasoning only works when
//! it comes back unchanged. Without `provider_raw` the message is rebuilt from its
//! canonical content, and reasoning text is dropped, because a reasoning item without
//! its encrypted content is refused when `store` is false. So is a message whose raw
//! items hold a call of a tool that the request does not offer: its canonical content
//! shows that call as text (`efr_provider::unoffered_call_text`).

use std::collections::HashSet;

use efr_provider::{
    ContentBlock, FREEFORM_INPUT, Message, ModelInfo, Request, Role, ToolDefinition,
};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::config::{Backend, OpenAiConfig, ReasoningMode};
use crate::models::is_reasoning_model;

/// The item type of a call of a freeform tool.
pub(crate) const CUSTOM_CALL: &str = "custom_tool_call";

/// What the model is asked to return besides its output: the encrypted reasoning that
/// lets the next request continue the same chain of thought without server-side state.
const ENCRYPTED_REASONING: &str = "reasoning.encrypted_content";

/// The JSON body of one `POST /responses`, and of the `response.create` message of the
/// WebSocket transport.
///
/// Field order follows Codex's `ResponsesApiRequest` and `ResponseCreateWsRequest`,
/// routing fields first, because gateways may read a large body incrementally.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ResponsesBody {
    /// `response.create` on a WebSocket; absent from an HTTP body.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<&'static str>,
    model: String,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_tier: Option<String>,
    /// The answer this one continues on the same WebSocket; never sent over HTTP.
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<Value>,
    tools: Vec<Value>,
    tool_choice: &'static str,
    parallel_tool_calls: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
    store: bool,
    include: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<TextParam>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReasoningParam {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TextParam {
    verbosity: String,
}

/// The body for `request` under `config`. `model` is what the provider knows about the
/// request's model, from its catalog and the config; `None` for a model that neither
/// lists, which then gets the function form of each freeform tool and the request's
/// output limit.
///
/// The request's `effort` is the reasoning effort, else the config's default, else the
/// backend's default for the model.
///
/// `provider_options` keys this provider reads, each overriding the config:
/// `reasoning_summary` (a string, or `null` for no summary), `parallel_tool_calls`
/// (a boolean), `prompt_cache_key`, `service_tier` and `text_verbosity` (strings).
/// Other keys, and known keys with a value of the wrong type, are ignored.
pub(crate) fn request_body(
    request: &Request,
    config: &OpenAiConfig,
    model: Option<&ModelInfo>,
) -> ResponsesBody {
    let options = Options(&request.provider_options);
    let reasons = match config.reasoning() {
        ReasoningMode::Always => true,
        ReasoningMode::Never => false,
        ReasoningMode::ByModel => is_reasoning_model(&request.model),
    };
    let reasoning = reasons.then(|| ReasoningParam {
        effort: request.effort.clone().or_else(|| config.reasoning_effort().map(str::to_owned)),
        summary: options.nullable_string("reasoning_summary", config.reasoning_summary()),
    });
    // NOTE: the subscription backend refuses `max_output_tokens`; Codex never sends it
    // and opencode removes it on that path ("Match codex cli").
    let max_output_tokens = match config.backend() {
        Backend::Subscription => None,
        // NOTE: a model's own limit (an entry of `[openai] models`, else the built-in
        // table's) wins over the request's, which is `[model] max_output_tokens` for
        // every model.
        _ => model.and_then(|model| model.max_output_tokens).or(request.max_output_tokens),
    };
    let freeform = model.is_some_and(|model| model.freeform_tools);
    ResponsesBody {
        kind: None,
        model: request.model.clone(),
        stream: true,
        service_tier: options.string("service_tier"),
        previous_response_id: None,
        instructions: request.system.clone().filter(|system| !system.is_empty()),
        input: input_items(&request.messages, freeform, |name| request.offers(name)),
        tools: request.tools.iter().map(|tool| tool_definition(tool, freeform)).collect(),
        tool_choice: "auto",
        parallel_tool_calls: options.bool("parallel_tool_calls", config.parallel_tool_calls()),
        reasoning,
        store: false,
        include: if reasons { vec![ENCRYPTED_REASONING] } else { Vec::new() },
        max_output_tokens,
        prompt_cache_key: options.string("prompt_cache_key"),
        text: options.string("text_verbosity").map(|verbosity| TextParam { verbosity }),
    }
}

impl ResponsesBody {
    /// The key that sends the request to its prompt cache, when the request names one.
    pub(crate) fn prompt_cache_key(&self) -> Option<&str> {
        self.prompt_cache_key.as_deref()
    }

    /// The `input` items.
    pub(crate) fn input(&self) -> &[Value] {
        &self.input
    }

    /// Every field but `input`: what must not change for a request to continue the
    /// previous answer of a WebSocket with only its new items.
    pub(crate) fn settings(&self) -> ResponsesBody {
        ResponsesBody { input: Vec::new(), ..self.clone() }
    }

    /// The `response.create` message of the WebSocket transport for this body: the same
    /// fields with `input` replaced by `input`, continuing the answer `previous` when
    /// one is given.
    pub(crate) fn websocket_create(
        &self,
        previous: Option<&str>,
        input: &[Value],
    ) -> ResponsesBody {
        ResponsesBody {
            kind: Some("response.create"),
            previous_response_id: previous.map(str::to_owned),
            input: input.to_vec(),
            ..self.settings()
        }
    }
}

/// The request's `provider_options`, read leniently: a value of the wrong type counts as
/// absent.
struct Options<'a>(&'a Map<String, Value>);

impl Options<'_> {
    fn string(&self, key: &str) -> Option<String> {
        match self.0.get(key) {
            Some(Value::String(value)) => Some(value.clone()),
            Some(other) => {
                ignored(key, other);
                None
            }
            None => None,
        }
    }

    /// A string option whose explicit `null` turns the default off.
    fn nullable_string(&self, key: &str, default: Option<&str>) -> Option<String> {
        match self.0.get(key) {
            Some(Value::String(value)) => Some(value.clone()),
            Some(Value::Null) => None,
            Some(other) => {
                ignored(key, other);
                default.map(str::to_owned)
            }
            None => default.map(str::to_owned),
        }
    }

    fn bool(&self, key: &str, default: bool) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(value)) => *value,
            Some(other) => {
                ignored(key, other);
                default
            }
            None => default,
        }
    }
}

fn ignored(key: &str, value: &Value) {
    tracing::debug!(
        option = key,
        kind = json_kind(value),
        "ignored a provider option of the wrong type"
    );
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A tool as the Responses API takes it, for a model that takes freeform tools when
/// `freeform` is true.
///
/// A freeform tool for such a model is a `custom` tool with its grammar as `format`.
/// Every other tool is a function tool: `strict` is off as in Codex, because strict
/// mode requires every property to be listed as required, which tool schemas with
/// optional inputs are not. A freeform tool's function form is its `input_schema`.
pub(crate) fn tool_definition(tool: &ToolDefinition, freeform: bool) -> Value {
    match &tool.grammar {
        Some(grammar) if freeform => json!({
            "type": "custom",
            "name": tool.name,
            "description": tool.description,
            "format": {
                "type": "grammar",
                "syntax": grammar.syntax,
                "definition": grammar.definition,
            },
        }),
        _ => json!({
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "strict": false,
            "parameters": tool.input_schema,
        }),
    }
}

/// The Responses `input` for a conversation, oldest first, for a model that takes
/// freeform tools when `takes_freeform` is true.
///
/// A tool result answers a `custom_tool_call` with a `custom_tool_call_output` and
/// any other call with a `function_call_output`, so the calls of the conversation are
/// read first, from the raw items and from the canonical content. A canonical
/// freeform call goes to a model that does not take freeform tools in the function
/// form that the tool has for it, so a conversation that changes the model keeps one
/// shape for each tool. `offers` says whether the request offers a tool: raw items
/// with a call of a tool that it does not offer are not sent.
pub(crate) fn input_items(
    messages: &[Message],
    takes_freeform: bool,
    offers: impl Fn(&str) -> bool,
) -> Vec<Value> {
    let freeform = freeform_calls(messages, takes_freeform, &offers);
    let mut items = Vec::new();
    for message in messages {
        match raw_items(message, &offers) {
            Some(raw) => items.extend(raw.iter().cloned()),
            None => push_canonical(&mut items, message, &freeform, takes_freeform),
        }
    }
    items
}

/// The provider ids of the calls in `messages` that go as `custom_tool_call` items:
/// those of their raw items, and the canonical freeform calls when `takes_freeform`
/// is true.
fn freeform_calls<'m>(
    messages: &'m [Message],
    takes_freeform: bool,
    offers: &impl Fn(&str) -> bool,
) -> HashSet<&'m str> {
    let mut calls = HashSet::new();
    for message in messages {
        if let Some(raw) = raw_items(message, offers) {
            calls.extend(
                raw.iter()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some(CUSTOM_CALL))
                    .filter_map(|item| item.get("call_id").and_then(Value::as_str)),
            );
        }
        calls.extend(message.content.iter().filter_map(|block| match block {
            ContentBlock::ToolCall { call_id, freeform: true, .. } if takes_freeform => {
                Some(call_id.as_str())
            }
            _ => None,
        }));
    }
    calls
}

/// The `provider_raw` of an assistant message whose response produced `items`: the
/// items themselves, verbatim and in order, as one JSON array. [`raw_items`] reads it
/// back.
pub(crate) fn provider_raw(items: Vec<Value>) -> Option<Value> {
    (!items.is_empty()).then_some(Value::Array(items))
}

/// The items of an assistant message's `provider_raw`, when it holds a list of items
/// and each call among them is of a tool that the request `offers`. Anything else falls
/// back to the canonical content, so a damaged or foreign value costs the encrypted
/// reasoning, not the request, and a call of a tool that the request does not offer
/// goes as the text of the canonical content.
fn raw_items<'m>(message: &'m Message, offers: &impl Fn(&str) -> bool) -> Option<&'m Vec<Value>> {
    if message.role != Role::Assistant {
        return None;
    }
    let raw = message.provider_raw.as_ref()?;
    match raw {
        Value::Array(items)
            if !items.is_empty()
                && items.iter().all(|item| item.get("type").is_some_and(Value::is_string)) =>
        {
            let unoffered = items.iter().any(|item| {
                let call = matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call" | CUSTOM_CALL)
                );
                call && !item.get("name").and_then(Value::as_str).is_some_and(offers)
            });
            if unoffered {
                tracing::debug!(
                    "provider_raw holds a call of a tool that the request does not offer; sending the canonical content"
                );
                return None;
            }
            Some(items)
        }
        _ => {
            tracing::debug!(
                kind = json_kind(raw),
                "provider_raw is not a list of Responses items; sending the canonical content"
            );
            None
        }
    }
}

/// Appends `message` rebuilt from its canonical blocks. Text and images of one run form
/// one message item; a tool call or a tool result ends the run and becomes an item of
/// its own, so the order of the blocks is kept. A result of one of the `freeform`
/// calls is a `custom_tool_call_output`. A freeform call is a `custom_tool_call` when
/// `takes_freeform` is true, else a `function_call` with the text in
/// [`FREEFORM_INPUT`].
fn push_canonical(
    items: &mut Vec<Value>,
    message: &Message,
    freeform: &HashSet<&str>,
    takes_freeform: bool,
) {
    let role = match message.role {
        Role::Assistant => "assistant",
        Role::User => "user",
        other => {
            tracing::debug!(role = ?other, "a role the Responses API does not know; sent as user");
            "user"
        }
    };
    let assistant = role == "assistant";
    let mut content: Vec<Value> = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text } => {
                if !text.is_empty() {
                    let kind = if assistant { "output_text" } else { "input_text" };
                    content.push(json!({"type": kind, "text": text}));
                }
            }
            ContentBlock::Image { media_type, data } => {
                if assistant {
                    tracing::debug!(
                        "an image in an assistant message; the Responses API takes none"
                    );
                    continue;
                }
                // The canonical form already serialises as standard base64.
                match serde_json::to_value(data) {
                    Ok(Value::String(base64)) => content.push(json!({
                        "type": "input_image",
                        "image_url": format!("data:{media_type};base64,{base64}"),
                    })),
                    _ => tracing::debug!("an image whose bytes did not encode; left out"),
                }
            }
            ContentBlock::ToolCall { call_id, name, input, freeform: true } if takes_freeform => {
                flush(items, role, &mut content);
                items.push(json!({
                    "type": CUSTOM_CALL,
                    "call_id": call_id,
                    "name": name,
                    "input": arguments_text(input),
                }));
            }
            ContentBlock::ToolCall { call_id, name, input, freeform } => {
                flush(items, role, &mut content);
                let arguments = match input {
                    Value::String(text) if *freeform => json!({ FREEFORM_INPUT: text }).to_string(),
                    other => arguments_text(other),
                };
                items.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments,
                }));
            }
            ContentBlock::ToolResult { call_id, output, is_error } => {
                flush(items, role, &mut content);
                // NOTE: the Responses API has no error flag on a tool result; goose
                // marks a failed call in the output text the same way.
                let output = if *is_error { format!("Error: {output}") } else { output.clone() };
                let kind = if freeform.contains(call_id.as_str()) {
                    "custom_tool_call_output"
                } else {
                    "function_call_output"
                };
                items.push(json!({
                    "type": kind,
                    "call_id": call_id,
                    "output": output,
                }));
            }
            ContentBlock::Reasoning { .. } => {}
            _ => tracing::debug!("a content block the Responses API cannot carry; left out"),
        }
    }
    flush(items, role, &mut content);
}

fn flush(items: &mut Vec<Value>, role: &str, content: &mut Vec<Value>) {
    if !content.is_empty() {
        items.push(json!({"type": "message", "role": role, "content": std::mem::take(content)}));
    }
}

/// The arguments text of a canonical tool call. Arguments that were not JSON were kept
/// as a JSON string, and so is the text of a freeform call; they go back as the text
/// the model wrote.
fn arguments_text(input: &Value) -> String {
    match input {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A Responses output item, read for what the canonical form needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutputItem {
    /// An assistant message: its `output_text` and `refusal` parts, concatenated.
    Message { text: String },
    /// A reasoning item: its summary parts, joined by a blank line.
    Reasoning { summary: String },
    /// A function call, or a freeform (`custom_tool_call`) call whose `arguments` are
    /// its raw text `input`.
    FunctionCall { call_id: String, name: String, arguments: String, freeform: bool },
    /// Any other item, such as a hosted tool call, which has no canonical form.
    Other { kind: String },
}

impl OutputItem {
    /// Reads `item`. A call without a `call_id` or a `name` is `Other`, since nothing
    /// could answer it.
    pub(crate) fn parse(item: &Value) -> OutputItem {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            // NOTE: the separators match what the streamed deltas add up to: the parts of
            // a message continue each other, the parts of a summary are sections.
            "message" => OutputItem::Message {
                text: joined(item, "content", "", &["output_text", "refusal"]),
            },
            "reasoning" => OutputItem::Reasoning {
                summary: joined(item, "summary", "\n\n", &["summary_text"]),
            },
            "function_call" | CUSTOM_CALL => {
                let freeform = kind == CUSTOM_CALL;
                let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
                let arguments = if freeform { "input" } else { "arguments" };
                match (text("call_id"), text("name")) {
                    (Some(call_id), Some(name)) => OutputItem::FunctionCall {
                        call_id,
                        name,
                        arguments: text(arguments).unwrap_or_default(),
                        freeform,
                    },
                    _ => OutputItem::Other { kind: kind.to_owned() },
                }
            }
            other => OutputItem::Other { kind: other.to_owned() },
        }
    }
}

/// The text of the parts of `item[list]` whose `type` is one of `kinds`, joined by
/// `separator`. A `refusal` part keeps its text under `refusal`.
fn joined(item: &Value, list: &str, separator: &str, kinds: &[&str]) -> String {
    let Some(parts) = item.get(list).and_then(Value::as_array) else {
        return String::new();
    };
    let texts: Vec<&str> = parts
        .iter()
        .filter(|part| {
            part.get("type").and_then(Value::as_str).is_some_and(|kind| kinds.contains(&kind))
        })
        .filter_map(|part| part.get("text").or_else(|| part.get("refusal")).and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect();
    texts.join(separator)
}

#[cfg(test)]
mod tests;
