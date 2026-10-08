//! A local stand-in for the OpenAI Responses API and the token endpoint of the OpenAI
//! login, so the daemon's real provider runs against wiremock instead of the network.
//!
//! Every `POST /v1/responses` gets the next queued answer, in order, and is kept for
//! the test to inspect. The daemon reaches it through the `openai-api` provider, or
//! the `openai-subscription` provider, whose base URL the test daemon points here.
//! Every `POST /oauth/token` gets the next queued token answer; the subscription
//! provider's token source refreshes there, with the server as its issuer. Every
//! `GET /v1/models`, the subscription's model catalog that efrd fetches in the
//! background, gets the answer of [`ResponsesServer::set_models`], else a 404.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

/// The path the provider posts to, below [`ResponsesServer::base_url`].
const RESPONSES_PATH: &str = "/v1/responses";

/// The token endpoint below [`ResponsesServer::issuer`].
const TOKEN_PATH: &str = "/oauth/token";

/// The model catalog below [`ResponsesServer::base_url`], which the subscription
/// provider fetches in the background.
const MODELS_PATH: &str = "/v1/models";

/// The status of a request that came after the last queued answer.
const EXHAUSTED_STATUS: u16 = 599;

/// One answer: an HTTP status and a body. A 2xx answer is sent as server-sent events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsesAnswer {
    /// The HTTP status.
    pub status: u16,
    /// The body.
    pub body: String,
}

impl ResponsesAnswer {
    /// An answer with `status` and `body`.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        ResponsesAnswer { status, body: body.into() }
    }

    /// A 200 answer that streams `text` as one assistant message: the Responses
    /// events from `response.created` to `response.completed`.
    pub fn text(text: &str) -> Self {
        let id = "0123456789abcdef0123456789abcdef";
        let item = serde_json::json!({
            "id": format!("msg_{id}"), "type": "message", "status": "completed", "role": "assistant",
            "content": [{ "type": "output_text", "annotations": [], "logprobs": [], "text": text }],
        });
        let part = serde_json::json!({ "type": "output_text", "annotations": [], "logprobs": [], "text": text });
        let message = |done: bool| {
            let mut started = item.clone();
            if !done {
                started["status"] = "in_progress".into();
                started["content"] = serde_json::json!([]);
            }
            started
        };
        let response = |status: &str, output: Value| {
            serde_json::json!({
                "id": format!("resp_{id}"), "object": "response", "created_at": 1_791_115_200,
                "status": status, "model": "gpt-5.5", "output": output,
                "usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 },
            })
        };
        let item_id = format!("msg_{id}");
        let events = [
            (
                "response.created",
                serde_json::json!({ "response": response("in_progress", serde_json::json!([])) }),
            ),
            (
                "response.output_item.added",
                serde_json::json!({ "output_index": 0, "item": message(false) }),
            ),
            (
                "response.content_part.added",
                serde_json::json!({ "item_id": item_id, "output_index": 0, "content_index": 0, "part": { "type": "output_text", "annotations": [], "logprobs": [], "text": "" } }),
            ),
            (
                "response.output_text.delta",
                serde_json::json!({ "item_id": item_id, "output_index": 0, "content_index": 0, "delta": text, "logprobs": [] }),
            ),
            (
                "response.output_text.done",
                serde_json::json!({ "item_id": item_id, "output_index": 0, "content_index": 0, "text": text, "logprobs": [] }),
            ),
            (
                "response.content_part.done",
                serde_json::json!({ "item_id": item_id, "output_index": 0, "content_index": 0, "part": part }),
            ),
            (
                "response.output_item.done",
                serde_json::json!({ "output_index": 0, "item": message(true) }),
            ),
            (
                "response.completed",
                serde_json::json!({ "response": response("completed", serde_json::json!([message(true)])) }),
            ),
        ];
        ResponsesAnswer::new(200, sse_body(events))
    }

    /// A 200 answer that asks for one function call of the tool `name` with
    /// `arguments` (a JSON object), sent whole in `response.output_item.done`, as a
    /// server may send an item without deltas.
    pub fn tool_call(call_id: &str, name: &str, arguments: &Value) -> Self {
        let id = "0123456789abcdef0123456789abcdef";
        let item = serde_json::json!({
            "id": format!("fc_{id}"), "type": "function_call", "status": "completed",
            "call_id": call_id, "name": name, "arguments": arguments.to_string(),
        });
        let response = |status: &str, output: Value| {
            serde_json::json!({
                "id": format!("resp_{id}"), "object": "response", "created_at": 1_791_115_200,
                "status": status, "model": "gpt-5.5", "output": output,
                "usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 },
            })
        };
        let events = [
            (
                "response.created",
                serde_json::json!({ "response": response("in_progress", serde_json::json!([])) }),
            ),
            ("response.output_item.done", serde_json::json!({ "output_index": 0, "item": item })),
            (
                "response.completed",
                serde_json::json!({ "response": response("completed", serde_json::json!([item])) }),
            ),
        ];
        ResponsesAnswer::new(200, sse_body(events))
    }

    /// A 200 answer that asks for one call of the freeform (`custom`) tool `name` with
    /// the text `input`, streamed as Codex's server streams it: the item added, the
    /// text in two deltas that name the item, the text done, the item done.
    pub fn custom_tool_call(call_id: &str, name: &str, input: &str) -> Self {
        let id = "0123456789abcdef0123456789abcdef";
        let item_id = format!("ctc_{id}");
        let item = |status: &str, input: &str| {
            serde_json::json!({
                "id": item_id, "type": "custom_tool_call", "status": status,
                "call_id": call_id, "name": name, "input": input,
            })
        };
        let response = |status: &str, output: Value| {
            serde_json::json!({
                "id": format!("resp_{id}"), "object": "response", "created_at": 1_791_115_200,
                "status": status, "model": "gpt-5.5", "output": output,
                "usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 },
            })
        };
        let middle = input.char_indices().nth(input.chars().count() / 2).map_or(0, |(at, _)| at);
        let (head, tail) = input.split_at(middle);
        let delta = |delta: &str| serde_json::json!({ "item_id": item_id, "output_index": 0, "delta": delta });
        let events = [
            (
                "response.created",
                serde_json::json!({ "response": response("in_progress", serde_json::json!([])) }),
            ),
            (
                "response.output_item.added",
                serde_json::json!({ "output_index": 0, "item": item("in_progress", "") }),
            ),
            ("response.custom_tool_call_input.delta", delta(head)),
            ("response.custom_tool_call_input.delta", delta(tail)),
            (
                "response.custom_tool_call_input.done",
                serde_json::json!({ "item_id": item_id, "output_index": 0, "input": input }),
            ),
            (
                "response.output_item.done",
                serde_json::json!({ "output_index": 0, "item": item("completed", input) }),
            ),
            (
                "response.completed",
                serde_json::json!({
                    "response": response("completed", serde_json::json!([item("completed", input)]))
                }),
            ),
        ];
        ResponsesAnswer::new(200, sse_body(events))
    }

    /// The answer a `provider_sse` text stands for. A first line `: status <code>`, a
    /// comment that any SSE reader skips, sets the status and is dropped; otherwise
    /// the status is 200.
    pub fn from_sse(text: &str) -> Self {
        let status = text
            .lines()
            .next()
            .and_then(|line| line.strip_prefix(": status "))
            .and_then(|code| code.trim().parse::<u16>().ok());
        match status {
            Some(status) => {
                let body = text.split_once('\n').map_or("", |(_, rest)| rest);
                ResponsesAnswer::new(status, body.trim_start_matches('\n'))
            }
            None => ResponsesAnswer::new(200, text),
        }
    }
}

/// Server-sent events of the Responses stream, each with its type and sequence number.
fn sse_body(events: impl IntoIterator<Item = (&'static str, Value)>) -> String {
    events
        .into_iter()
        .enumerate()
        .map(|(sequence, (kind, mut data))| {
            data["type"] = kind.into();
            data["sequence_number"] = sequence.into();
            format!("event: {kind}\ndata: {data}\n\n")
        })
        .collect()
}

/// A request the server got.
#[derive(Clone, PartialEq, Eq)]
pub struct ReceivedRequest {
    /// The body as JSON, or `null` when it was not JSON.
    pub body: Value,
    /// The `Authorization` header, when there was one.
    pub authorization: Option<String>,
    /// The `chatgpt-account-id` header, which the subscription provider sends.
    pub account_id: Option<String>,
}

impl fmt::Debug for ReceivedRequest {
    // The authorization header carries a key, even if only a test's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceivedRequest")
            .field("body", &self.body)
            .field("authorization", &self.authorization.as_ref().map(|_| "<redacted>"))
            .field("account_id", &self.account_id)
            .finish()
    }
}

/// How the server answers a fetch of the model catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsAnswer {
    /// The HTTP status.
    pub status: u16,
    /// The JSON body, when there is one.
    pub body: Option<String>,
    /// The `ETag` header, when there is one.
    pub etag: Option<String>,
    /// How long the server waits before it answers.
    pub delay: Duration,
}

impl ModelsAnswer {
    /// A 200 answer with the catalog `body` and its `etag`.
    pub fn catalog(body: &Value, etag: &str) -> Self {
        ModelsAnswer {
            status: 200,
            body: Some(body.to_string()),
            etag: Some(etag.to_owned()),
            delay: Duration::ZERO,
        }
    }

    /// An answer with `status` and no body, such as 304 or 503.
    pub fn status(status: u16) -> Self {
        ModelsAnswer { status, body: None, etag: None, delay: Duration::ZERO }
    }

    /// The same answer after `delay`, such as a backend that hangs.
    #[must_use]
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// A fetch of the model catalog that the server got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsRequest {
    /// The `client_version` query parameter.
    pub client_version: Option<String>,
    /// The `If-None-Match` header.
    pub if_none_match: Option<String>,
    /// The `originator` header.
    pub originator: Option<String>,
}

/// A request the token endpoint got: its form parameters. `Debug` leaves out the
/// values, which carry tokens.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenRequest {
    /// The form parameters, such as `grant_type` and `refresh_token`.
    pub form: BTreeMap<String, String>,
}

impl fmt::Debug for TokenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenRequest").field("keys", &self.form.keys().collect::<Vec<_>>()).finish()
    }
}

#[derive(Debug, Default)]
struct Queue {
    answers: VecDeque<ResponsesAnswer>,
    received: Vec<ReceivedRequest>,
    token_answers: VecDeque<ResponsesAnswer>,
    token_requests: Vec<TokenRequest>,
    models: Option<ModelsAnswer>,
    models_requests: Vec<ModelsRequest>,
}

/// A wiremock server that answers `POST /v1/responses` from a queue.
pub struct ResponsesServer {
    server: MockServer,
    queue: Arc<Mutex<Queue>>,
}

impl ResponsesServer {
    /// Starts the server on a free local port with an empty queue.
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let queue = Arc::new(Mutex::new(Queue::default()));
        Mock::given(method("POST"))
            .and(path(RESPONSES_PATH))
            .respond_with(Answers { queue: Arc::clone(&queue) })
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(TOKEN_PATH))
            .respond_with(TokenAnswers { queue: Arc::clone(&queue) })
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .respond_with(ModelsAnswers { queue: Arc::clone(&queue) })
            .mount(&server)
            .await;
        ResponsesServer { server, queue }
    }

    /// The base URL the provider is configured with: the server and `/v1`.
    pub fn base_url(&self) -> String {
        format!("{}/v1", self.server.uri())
    }

    /// The authorization server the login's token source is configured with: the
    /// server itself, whose `/oauth/token` answers from the token queue.
    pub fn issuer(&self) -> String {
        self.server.uri()
    }

    /// Queues `answer` for the next request.
    pub fn push(&self, answer: ResponsesAnswer) {
        lock(&self.queue).answers.push_back(answer);
    }

    /// Queues `answer`, sent as JSON, for the next request to the token endpoint.
    pub fn push_token(&self, answer: ResponsesAnswer) {
        lock(&self.queue).token_answers.push_back(answer);
    }

    /// Every request so far, in order.
    pub fn received(&self) -> Vec<ReceivedRequest> {
        lock(&self.queue).received.clone()
    }

    /// Every request to the token endpoint so far, in order.
    pub fn token_requests(&self) -> Vec<TokenRequest> {
        lock(&self.queue).token_requests.clone()
    }

    /// The answers no request has taken yet.
    pub fn remaining(&self) -> usize {
        lock(&self.queue).answers.len()
    }

    /// Answers every later fetch of the model catalog with `answer`. Until a test sets
    /// one, the catalog is not found (404), so the daemon keeps the list it has.
    pub fn set_models(&self, answer: ModelsAnswer) {
        lock(&self.queue).models = Some(answer);
    }

    /// Every fetch of the model catalog so far, in order.
    pub fn models_requests(&self) -> Vec<ModelsRequest> {
        lock(&self.queue).models_requests.clone()
    }
}

impl fmt::Debug for ResponsesServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let queue = lock(&self.queue);
        f.debug_struct("ResponsesServer")
            .field("uri", &self.server.uri())
            .field("received", &queue.received.len())
            .field("remaining", &queue.answers.len())
            .finish()
    }
}

/// The responder behind the mock.
struct Answers {
    queue: Arc<Mutex<Queue>>,
}

impl Respond for Answers {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let header = |name: &str| {
            request.headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
        };
        let authorization = header("authorization");
        let account_id = header("chatgpt-account-id");
        let mut queue = lock(&self.queue);
        queue.received.push(ReceivedRequest { body, authorization, account_id });
        match queue.answers.pop_front() {
            Some(answer) if (200..300).contains(&answer.status) => {
                ResponseTemplate::new(answer.status)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(answer.body)
            }
            Some(answer) => ResponseTemplate::new(answer.status)
                .insert_header("content-type", "application/json")
                .set_body_string(answer.body),
            None => ResponseTemplate::new(EXHAUSTED_STATUS)
                .set_body_string("the transcript has no answer left for this request"),
        }
    }
}

/// The responder behind the model catalog's mock.
struct ModelsAnswers {
    queue: Arc<Mutex<Queue>>,
}

impl Respond for ModelsAnswers {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let header = |name: &str| {
            request.headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
        };
        let client_version = request
            .url
            .query_pairs()
            .find(|(key, _)| key == "client_version")
            .map(|(_, value)| value.into_owned());
        let mut queue = lock(&self.queue);
        queue.models_requests.push(ModelsRequest {
            client_version,
            if_none_match: header("if-none-match"),
            originator: header("originator"),
        });
        let Some(answer) = queue.models.clone() else {
            return ResponseTemplate::new(404).set_body_string("no catalog here");
        };
        let mut template = ResponseTemplate::new(answer.status).set_delay(answer.delay);
        if let Some(etag) = &answer.etag {
            template = template.insert_header("etag", etag.as_str());
        }
        match answer.body {
            Some(body) => template.set_body_raw(body, "application/json"),
            None => template,
        }
    }
}

/// The responder behind the token endpoint's mock.
struct TokenAnswers {
    queue: Arc<Mutex<Queue>>,
}

impl Respond for TokenAnswers {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let form = url::form_urlencoded::parse(&request.body).into_owned().collect();
        let mut queue = lock(&self.queue);
        queue.token_requests.push(TokenRequest { form });
        match queue.token_answers.pop_front() {
            Some(answer) => ResponseTemplate::new(answer.status)
                .insert_header("content-type", "application/json")
                .set_body_string(answer.body),
            None => ResponseTemplate::new(EXHAUSTED_STATUS)
                .set_body_string("the test queued no answer for this token request"),
        }
    }
}

fn lock(queue: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    // Every critical section leaves the queue whole, so a poisoned lock is usable.
    queue.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
