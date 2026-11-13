//! Canonical requests to the Responses API, and Responses output items back.
//!
//! The request shape follows Codex (`codex-rs/core/src/client.rs`,
//! `build_responses_request`, and `codex-rs/codex-api/src/common.rs`,
//! `ResponsesApiRequest`): `stream: true`, `store: false`, the system prompt as
//! `instructions`, `tool_choice: "auto"`, function tools with `strict: false`, and
//! `include: ["reasoning.encrypted_content"]` so that a stateless conversation can send
//! its reasoning back. A request without tools, such as a review by the classifier,
//! sends neither `tool_choice` nor `parallel_tool_calls`: the API's defaults fit it, and
//! a member that is not sent cannot be refused. A request with an output schema
//! (`Request::output`) asks for it as `text.format`, a `json_schema` in strict mode.
//! The item shapes are Codex's `ResponseItem`
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
//!
//! The web tool (`HostedTool::WebSearch` in `Request::hosted`) goes after the function
//! tools as `{"type": "web_search", "external_web_access": <live>}`, always with the
//! member, as Codex sends it (`codex-rs/core/src/tools/hosted_spec.rs`,
//! `create_web_search_tool`): `false` reads the provider's stored copy of the web,
//! `true` the live web (developers.openai.com/api/docs/guides/tools-web-search). A
//! request whose reasoning effort is `minimal` gets no web tool: the guide says that web
//! search needs an effort above it. A `web_search_call` item is a step of that tool; it
//! has no canonical form, so it goes back only in its raw items, and only in a request
//! that carries the web tool. In any other request its message falls back to the
//! canonical content, because the docs do not say that the API takes the item without
//! the tool.

use std::collections::HashSet;

use efr_provider::{
    ContentBlock, FREEFORM_INPUT, HostedAction, HostedTool, Message, ModelInfo, Request, Role,
    ToolDefinition, WebAccess,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use crate::openai::config::{Backend, OpenAiConfig, ReasoningMode};
use crate::openai::models::is_reasoning_model;

/// The item type of a call of a freeform tool.
pub(crate) const CUSTOM_CALL: &str = "custom_tool_call";

/// The item type of a step of the hosted web tool.
pub(crate) const WEB_CALL: &str = "web_search_call";

/// The reasoning effort at which the web tool is not sent: the guide says that web
/// search needs an effort above it (on `gpt-5`).
const NO_WEB_EFFORT: &str = "minimal";

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
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
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

/// The `text` member: how long the answer is, and the form that it must take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TextParam {
    #[serde(skip_serializing_if = "Option::is_none")]
    verbosity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<TextFormat>,
}

/// `{"type": "json_schema", name, schema, "strict": true}`: the answer is one JSON
/// object that the schema describes. Strict mode needs every property in `required`
/// and `additionalProperties: false`, so the caller's schema must give both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TextFormat {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    schema: Value,
    strict: bool,
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
    let minimal = reasoning
        .as_ref()
        .is_some_and(|reasoning| reasoning.effort.as_deref() == Some(NO_WEB_EFFORT));
    let web = web_tool(&request.hosted, minimal);
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
    let mut tools: Vec<Value> =
        request.tools.iter().map(|tool| tool_definition(tool, freeform)).collect();
    let carries_web = web.is_some();
    tools.extend(web);
    let calls_tools = !tools.is_empty();
    let format = request.output.as_ref().map(|output| TextFormat {
        kind: "json_schema",
        name: output.name.clone(),
        schema: output.schema.clone(),
        strict: true,
    });
    let verbosity = options.string("text_verbosity");
    let text = (verbosity.is_some() || format.is_some()).then_some(TextParam { verbosity, format });
    ResponsesBody {
        kind: None,
        model: request.model.clone(),
        stream: true,
        service_tier: options.string("service_tier"),
        previous_response_id: None,
        instructions: request.system.clone().filter(|system| !system.is_empty()),
        input: input_items(&request.messages, freeform, carries_web, |name| request.offers(name)),
        tools,
        tool_choice: calls_tools.then_some("auto"),
        parallel_tool_calls: calls_tools
            .then(|| options.bool("parallel_tool_calls", config.parallel_tool_calls())),
        reasoning,
        store: false,
        include: if reasons { vec![ENCRYPTED_REASONING] } else { Vec::new() },
        max_output_tokens,
        prompt_cache_key: options.string("prompt_cache_key"),
        text,
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
        // NOTE: every field is named, with no `..`, so the input is never copied only
        // to be dropped, and a new field cannot be left out of the comparison.
        let ResponsesBody {
            kind,
            model,
            stream,
            service_tier,
            previous_response_id,
            instructions,
            input: _,
            tools,
            tool_choice,
            parallel_tool_calls,
            reasoning,
            store,
            include,
            max_output_tokens,
            prompt_cache_key,
            text,
        } = self;
        ResponsesBody {
            kind: *kind,
            model: model.clone(),
            stream: *stream,
            service_tier: service_tier.clone(),
            previous_response_id: previous_response_id.clone(),
            instructions: instructions.clone(),
            input: Vec::new(),
            tools: tools.clone(),
            tool_choice: *tool_choice,
            parallel_tool_calls: *parallel_tool_calls,
            reasoning: reasoning.clone(),
            store: *store,
            include: include.clone(),
            max_output_tokens: *max_output_tokens,
            prompt_cache_key: prompt_cache_key.clone(),
            text: text.clone(),
        }
    }

    /// The `response.create` message of the WebSocket transport for this body: the same
    /// fields with `input` replaced by `input`, continuing the answer `previous` when
    /// one is given. Only `input` is copied of the items.
    pub(crate) fn websocket_create(
        &self,
        previous: Option<&str>,
        input: &[Value],
    ) -> ResponsesBody {
        let mut create = self.settings();
        create.kind = Some("response.create");
        create.previous_response_id = previous.map(str::to_owned);
        create.input = input.to_vec();
        create
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

/// The web tool of a request whose hosted tools are `hosted`, or `None` when it has none
/// or when the effort is `minimal`. A second web entry adds nothing: the API takes one.
pub(crate) fn web_tool(hosted: &[HostedTool], minimal: bool) -> Option<Value> {
    let access = hosted.iter().find_map(|tool| match tool {
        HostedTool::WebSearch { access, .. } => Some(*access),
        _ => None,
    })?;
    if minimal {
        tracing::debug!("the reasoning effort is minimal; the request carries no web tool");
        return None;
    }
    Some(json!({"type": "web_search", "external_web_access": access == WebAccess::Live}))
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
/// with a call of a tool that it does not offer are not sent. `web` says whether the
/// request carries the web tool: raw items with a step of it are not sent without it.
pub(crate) fn input_items(
    messages: &[Message],
    takes_freeform: bool,
    web: bool,
    offers: impl Fn(&str) -> bool,
) -> Vec<Value> {
    let offered = Offered { web, tools: offers };
    let freeform = freeform_calls(messages, takes_freeform, &offered);
    let mut items = Vec::new();
    for message in messages {
        match raw_items(message, &offered) {
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
    offered: &Offered<impl Fn(&str) -> bool>,
) -> HashSet<&'m str> {
    let mut calls = HashSet::new();
    for message in messages {
        if let Some(raw) = raw_items(message, offered) {
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

/// What a request offers the model: its tools by name, and whether it carries the web
/// tool.
struct Offered<F> {
    web: bool,
    tools: F,
}

/// The items of an assistant message's `provider_raw`, when it holds a list of items,
/// each call among them is of a tool that the request offers, and a step of the web
/// tool is among them only when the request carries that tool. Anything else falls
/// back to the canonical content, so a damaged or foreign value costs the encrypted
/// reasoning, not the request, a call of a tool that the request does not offer goes as
/// the text of the canonical content, and a web search of an earlier turn reaches a
/// request without the web tool (manual mode, `web.search = "off"`) only as the answer
/// that the model wrote.
fn raw_items<'m>(
    message: &'m Message,
    offered: &Offered<impl Fn(&str) -> bool>,
) -> Option<&'m Vec<Value>> {
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
                call && !item.get("name").and_then(Value::as_str).is_some_and(&offered.tools)
            });
            if unoffered {
                tracing::debug!(
                    "provider_raw holds a call of a tool that the request does not offer; sending the canonical content"
                );
                return None;
            }
            let web =
                items.iter().any(|item| item.get("type").and_then(Value::as_str) == Some(WEB_CALL));
            if web && !offered.web {
                tracing::debug!(
                    "provider_raw holds a web search and the request carries no web tool; sending the canonical content"
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
                    "call_id": call_id_of(call_id),
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
                    "call_id": call_id_of(call_id),
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
                    "call_id": call_id_of(call_id),
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

/// The longest `call_id` that the code keeps as it is.
const MAX_CALL_ID: usize = 64;

/// The `call_id` of a canonical tool call as the Responses API gets it. An id in the
/// form of OpenAI's own (`call_`, then only `[a-zA-Z0-9_-]`, at most 64 characters)
/// goes as it is. Another id, such as Claude's `toolu_...` after a switch of provider,
/// becomes `call_` and 24 hex digits of its SHA-256: the same each time, so the call
/// and its result (both go through here) still match, and the bytes of the request
/// stay the same for the prompt cache.
fn call_id_of(id: &str) -> String {
    let own = id.len() <= MAX_CALL_ID
        && id.strip_prefix("call_").is_some_and(|rest| {
            !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if own {
        return id.to_owned();
    }
    let digest = Sha256::digest(id.as_bytes());
    let hex: String = digest.iter().take(12).map(|byte| format!("{byte:02x}")).collect();
    format!("call_{hex}")
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
    /// A step of the hosted web tool (`web_search_call`). It has no canonical form: the
    /// stream reports it as hosted call events, and it goes back only in the raw items.
    WebCall {
        /// The item's id, when it has one.
        id: Option<String>,
        /// The item's `status`: `in_progress`, `searching`, `completed`, `failed` or
        /// `incomplete` by the reference.
        status: Option<String>,
        /// What the step did. A search with several `queries` is one search, as the
        /// guide bills it ("Search actions incur a tool call cost").
        action: HostedAction,
    },
    /// Any other item, such as a hosted tool call of another tool, which has no
    /// canonical form.
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
            WEB_CALL => OutputItem::WebCall {
                id: string(item, "id"),
                status: string(item, "status"),
                action: web_action(item.get("action")),
            },
            other => OutputItem::Other { kind: other.to_owned() },
        }
    }
}

/// The member `key` of `value` when it is a string that is not empty.
fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// The step of a `web_search_call` whose `action` is `action`, by the reference
/// (developers.openai.com/api/reference/resources/responses/streaming-events,
/// `WebSearchCall`): a `search` with its `queries` joined by `; ` when the list is not
/// empty, else with its `query`; an `open_page` with its `url`; a `find_in_page` with its
/// `pattern` and `url`. One item is one step: the guide bills one tool call for each
/// search action, also when it lists several queries. No action, or an action of another
/// type, is a search without a query: the guide says that a search "will usually (but
/// not always)" name its queries.
fn web_action(action: Option<&Value>) -> HostedAction {
    let Some(action) = action.filter(|action| action.is_object()) else {
        return HostedAction::Search { query: None };
    };
    match action.get("type").and_then(Value::as_str) {
        Some("search") => {
            let mut queries: Vec<&str> = Vec::new();
            for query in action
                .get("queries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|query| !query.trim().is_empty())
            {
                if !queries.contains(&query) {
                    queries.push(query);
                }
            }
            let query = if queries.is_empty() {
                string(action, "query").filter(|query| !query.trim().is_empty())
            } else {
                Some(queries.join("; "))
            };
            HostedAction::Search { query }
        }
        Some("open_page") => HostedAction::Open { url: string(action, "url") },
        Some("find_in_page") => HostedAction::Find {
            pattern: action.get("pattern").and_then(Value::as_str).unwrap_or_default().to_owned(),
            url: string(action, "url"),
        },
        other => {
            tracing::debug!(action = ?other.map(|kind| kind.chars().take(64).collect::<String>()), "a web search step of an unknown action; shown as a search");
            HostedAction::Search { query: None }
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
