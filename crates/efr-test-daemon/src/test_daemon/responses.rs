//! A local stand-in for the OpenAI Responses API and the token endpoint of the OpenAI
//! login, so the daemon's real provider runs against wiremock instead of the network.
//!
//! Every `POST /v1/responses` gets the next queued answer, in order, and is kept for
//! the test to inspect. The daemon reaches it through the `openai-api` provider, or
//! the `openai-subscription` provider, whose base URL the test daemon points here.
//! Every `POST /oauth/token` gets the next queued token answer; the subscription
//! provider's token source refreshes there, with the server as its issuer.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

/// The path the provider posts to, below [`ResponsesServer::base_url`].
const RESPONSES_PATH: &str = "/v1/responses";

/// The token endpoint below [`ResponsesServer::issuer`].
const TOKEN_PATH: &str = "/oauth/token";

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
        let body = events
            .into_iter()
            .enumerate()
            .map(|(sequence, (kind, mut data))| {
                data["type"] = kind.into();
                data["sequence_number"] = sequence.into();
                format!("event: {kind}\ndata: {data}\n\n")
            })
            .collect::<String>();
        ResponsesAnswer::new(200, body)
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
