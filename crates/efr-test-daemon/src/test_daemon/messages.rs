//! A local stand-in for Anthropic's Messages API, so the daemon's real Anthropic
//! provider runs against wiremock instead of the network.
//!
//! Every `POST /v1/messages` gets the next queued answer, in order, and is kept for the
//! test to inspect, with the headers that the provider sends. Every `GET /v1/models`
//! gets a page of the list of [`MessagesServer::set_models`], by its `limit` (20 when
//! absent, as the API does) and its `after_id`, else the refusal of
//! [`MessagesServer::refuse_models`], else a 404. A fetch of the catalog and a key check
//! both reach it.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

/// The path the provider posts to, below [`MessagesServer::base_url`].
const MESSAGES_PATH: &str = "/v1/messages";

/// The model list below [`MessagesServer::base_url`].
const MODELS_PATH: &str = "/v1/models";

/// The status of a request that came after the last queued answer.
const EXHAUSTED_STATUS: u16 = 599;

/// The page size of the API when a request names none.
const DEFAULT_LIMIT: usize = 20;

/// The model of the answers that the builders make.
const MODEL: &str = "claude-opus-5-5";

/// One answer: an HTTP status, a body and the wait that it asks for. A 2xx answer is
/// sent as server-sent events, any other as JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagesAnswer {
    /// The HTTP status.
    pub status: u16,
    /// The body.
    pub body: String,
    /// The `retry-after` header in seconds, when there is one.
    pub retry_after: Option<u64>,
}

impl MessagesAnswer {
    /// An answer with `status` and `body`.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        MessagesAnswer { status, body: body.into(), retry_after: None }
    }

    /// A 200 answer that streams `events`, each `(type, data)`, in order; the type is
    /// set in each data object, as the API does.
    pub fn events(events: &[(&str, Value)]) -> Self {
        let body = events
            .iter()
            .map(|(kind, data)| {
                let mut data = data.clone();
                data["type"] = json!(kind);
                format!("event: {kind}\ndata: {data}\n\n")
            })
            .collect::<String>();
        MessagesAnswer::new(200, body)
    }

    /// A 200 answer that streams `text` as one text block and ends the turn, from
    /// `message_start` to `message_stop`, with a `ping` and the usage.
    pub fn text(text: &str) -> Self {
        MessagesAnswer::events(&[
            ("message_start", message_start()),
            ("ping", json!({})),
            (
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            (
                "content_block_delta",
                json!({"index": 0, "delta": {"type": "text_delta", "text": text}}),
            ),
            ("content_block_stop", json!({"index": 0})),
            ("message_delta", message_delta("end_turn")),
            ("message_stop", json!({})),
        ])
    }

    /// A 200 answer that asks for one call of the tool `name` with `input` (a JSON
    /// object), whose input streams in two `input_json_delta` parts, and stops for the
    /// tool.
    pub fn tool_use(call_id: &str, name: &str, input: &Value) -> Self {
        let text = input.to_string();
        let middle = text.char_indices().nth(text.chars().count() / 2).map_or(0, |(at, _)| at);
        let (head, tail) = text.split_at(middle);
        let delta = |part: &str| json!({"index": 0, "delta": {"type": "input_json_delta", "partial_json": part}});
        MessagesAnswer::events(&[
            ("message_start", message_start()),
            (
                "content_block_start",
                json!({"index": 0, "content_block": {
                    "type": "tool_use", "id": call_id, "name": name, "input": {},
                }}),
            ),
            ("content_block_delta", delta(head)),
            ("content_block_delta", delta(tail)),
            ("content_block_stop", json!({"index": 0})),
            ("message_delta", message_delta("tool_use")),
            ("message_stop", json!({})),
        ])
    }

    /// An error answer with `status` and the API's error body of `kind` and `message`.
    pub fn error(status: u16, kind: &str, message: &str) -> Self {
        let body = json!({
            "type": "error",
            "error": {"type": kind, "message": message},
            "request_id": "req_test",
        });
        MessagesAnswer::new(status, body.to_string())
    }

    /// The same answer, asking the client to wait `seconds` before it tries again.
    #[must_use]
    pub fn with_retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    fn template(&self, request_id: &str) -> ResponseTemplate {
        let content_type = if (200..300).contains(&self.status) {
            "text/event-stream"
        } else {
            "application/json"
        };
        let mut template = ResponseTemplate::new(self.status)
            .insert_header("request-id", request_id)
            .set_body_raw(self.body.clone(), content_type);
        if let Some(seconds) = self.retry_after {
            template = template.insert_header("retry-after", seconds.to_string().as_str());
        }
        template
    }
}

/// The `message_start` of the builders: an empty assistant message with its usage.
fn message_start() -> Value {
    json!({"message": {
        "id": "msg_test", "type": "message", "role": "assistant", "content": [],
        "model": MODEL, "stop_reason": null, "stop_sequence": null,
        "usage": {
            "input_tokens": 10, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0, "output_tokens": 1,
        },
    }})
}

/// The `message_delta` of the builders, which stops for `stop_reason`.
fn message_delta(stop_reason: &str) -> Value {
    json!({
        "delta": {"stop_reason": stop_reason, "stop_sequence": null},
        "usage": {"output_tokens": 5},
    })
}

/// A model call that the server got. `Debug` hides the credential headers.
#[derive(Clone, PartialEq, Eq)]
pub struct MessagesRequest {
    /// The body as JSON, or `null` when it was not JSON.
    pub body: Value,
    /// The `Authorization` header, when there was one.
    pub authorization: Option<String>,
    /// The `x-api-key` header, when there was one; the provider never sends it.
    pub api_key: Option<String>,
    /// The `anthropic-version` header.
    pub version: Option<String>,
    /// The `anthropic-beta` header.
    pub beta: Option<String>,
    /// The `anthropic-workspace-id` header.
    pub workspace_id: Option<String>,
}

impl fmt::Debug for MessagesRequest {
    // The credential headers carry a key, even if only a test's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessagesRequest")
            .field("body", &self.body)
            .field("authorization", &self.authorization.as_ref().map(|_| "<redacted>"))
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("version", &self.version)
            .field("beta", &self.beta)
            .field("workspace_id", &self.workspace_id)
            .finish()
    }
}

/// A request for a page of the model list that the server got. `Debug` hides the
/// credential header.
#[derive(Clone, PartialEq, Eq)]
pub struct ModelsPageRequest {
    /// The `limit` query parameter.
    pub limit: Option<String>,
    /// The `after_id` query parameter.
    pub after_id: Option<String>,
    /// The `Authorization` header, when there was one.
    pub authorization: Option<String>,
    /// The `anthropic-version` header.
    pub version: Option<String>,
    /// The `anthropic-workspace-id` header.
    pub workspace_id: Option<String>,
}

impl fmt::Debug for ModelsPageRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelsPageRequest")
            .field("limit", &self.limit)
            .field("after_id", &self.after_id)
            .field("authorization", &self.authorization.as_ref().map(|_| "<redacted>"))
            .field("version", &self.version)
            .field("workspace_id", &self.workspace_id)
            .finish()
    }
}

#[derive(Debug, Default)]
struct Queue {
    answers: VecDeque<MessagesAnswer>,
    received: Vec<MessagesRequest>,
    answered: usize,
    models: Option<Vec<Value>>,
    models_refusal: Option<MessagesAnswer>,
    models_requests: Vec<ModelsPageRequest>,
}

/// A wiremock server that answers `POST /v1/messages` from a queue and
/// `GET /v1/models` from a list.
pub struct MessagesServer {
    server: MockServer,
    queue: Arc<Mutex<Queue>>,
}

impl MessagesServer {
    /// Starts the server on a free local port with an empty queue and no model list.
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let queue = Arc::new(Mutex::new(Queue::default()));
        Mock::given(method("POST"))
            .and(path(MESSAGES_PATH))
            .respond_with(Answers { queue: Arc::clone(&queue) })
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .respond_with(Pages { queue: Arc::clone(&queue) })
            .mount(&server)
            .await;
        MessagesServer { server, queue }
    }

    /// The base URL the provider is configured with: the server and `/v1`.
    pub fn base_url(&self) -> String {
        format!("{}/v1", self.server.uri())
    }

    /// An entry of the model list in the API's form: `lifecycle` `active`, a window of
    /// 1M tokens, 128K output tokens and the efforts `low` to `max`.
    pub fn model(id: &str) -> Value {
        let supported = json!({"supported": true});
        json!({
            "type": "model",
            "id": id,
            "display_name": id,
            "created_at": "2026-09-01T00:00:00Z",
            "max_input_tokens": 1_000_000,
            "max_tokens": 128_000,
            "lifecycle": "active",
            "capabilities": {
                "effort": {
                    "supported": true,
                    "low": supported, "medium": supported, "high": supported,
                    "xhigh": supported, "max": supported,
                },
                "thinking": {"supported": true, "types": {"adaptive": supported}},
            },
        })
    }

    /// Queues `answer` for the next model call.
    pub fn push(&self, answer: MessagesAnswer) {
        lock(&self.queue).answers.push_back(answer);
    }

    /// Every model call so far, in order.
    pub fn received(&self) -> Vec<MessagesRequest> {
        lock(&self.queue).received.clone()
    }

    /// The answers no model call has taken yet.
    pub fn remaining(&self) -> usize {
        lock(&self.queue).answers.len()
    }

    /// Answers every later request for the model list with pages of `models`, the
    /// entries in the API's form (see [`MessagesServer::model`]), newest first.
    pub fn set_models(&self, models: Vec<Value>) {
        let mut queue = lock(&self.queue);
        queue.models = Some(models);
        queue.models_refusal = None;
    }

    /// Answers every later request for the model list with `answer`, such as a 401
    /// for a refused key or a 503.
    pub fn refuse_models(&self, answer: MessagesAnswer) {
        lock(&self.queue).models_refusal = Some(answer);
    }

    /// Every request for a page of the model list so far, in order.
    pub fn models_requests(&self) -> Vec<ModelsPageRequest> {
        lock(&self.queue).models_requests.clone()
    }
}

impl fmt::Debug for MessagesServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let queue = lock(&self.queue);
        f.debug_struct("MessagesServer")
            .field("uri", &self.server.uri())
            .field("received", &queue.received.len())
            .field("remaining", &queue.answers.len())
            .finish()
    }
}

/// The value of the header `name` of `request` as text.
fn header(request: &wiremock::Request, name: &str) -> Option<String> {
    request.headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
}

/// The query parameter `name` of `request`.
fn query(request: &wiremock::Request, name: &str) -> Option<String> {
    request.url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

/// The responder behind the model calls' mock.
struct Answers {
    queue: Arc<Mutex<Queue>>,
}

impl Respond for Answers {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let mut queue = lock(&self.queue);
        queue.received.push(MessagesRequest {
            body,
            authorization: header(request, "authorization"),
            api_key: header(request, "x-api-key"),
            version: header(request, "anthropic-version"),
            beta: header(request, "anthropic-beta"),
            workspace_id: header(request, "anthropic-workspace-id"),
        });
        queue.answered += 1;
        let request_id = format!("req_test_{}", queue.answered);
        match queue.answers.pop_front() {
            Some(answer) => answer.template(&request_id),
            None => ResponseTemplate::new(EXHAUSTED_STATUS)
                .set_body_string("the test queued no answer for this model call"),
        }
    }
}

/// The responder behind the model list's mock.
struct Pages {
    queue: Arc<Mutex<Queue>>,
}

impl Respond for Pages {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let limit = query(request, "limit");
        let after_id = query(request, "after_id");
        let mut queue = lock(&self.queue);
        queue.models_requests.push(ModelsPageRequest {
            limit: limit.clone(),
            after_id: after_id.clone(),
            authorization: header(request, "authorization"),
            version: header(request, "anthropic-version"),
            workspace_id: header(request, "anthropic-workspace-id"),
        });
        if let Some(refusal) = &queue.models_refusal {
            return refusal.template("req_test_models");
        }
        let Some(models) = &queue.models else {
            return MessagesAnswer::error(404, "not_found_error", "no model list here")
                .template("req_test_models");
        };
        let limit = limit.and_then(|limit| limit.parse().ok()).unwrap_or(DEFAULT_LIMIT);
        let start = match &after_id {
            Some(after) => models
                .iter()
                .position(|model| model["id"].as_str() == Some(after.as_str()))
                .map_or(models.len(), |at| at + 1),
            None => 0,
        };
        let page: Vec<Value> = models.iter().skip(start).take(limit).cloned().collect();
        let id = |model: Option<&Value>| model.map_or(Value::Null, |model| model["id"].clone());
        let body = json!({
            "data": page,
            "has_more": start + page.len() < models.len(),
            "first_id": id(page.first()),
            "last_id": id(page.last()),
        });
        ResponseTemplate::new(200)
            .insert_header("request-id", "req_test_models")
            .set_body_json(body)
    }
}

fn lock(queue: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    // Every critical section leaves the queue whole, so a poisoned lock is usable.
    queue.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
