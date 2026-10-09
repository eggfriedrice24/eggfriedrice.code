//! The Responses API client.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_http::{
    ByteStream, HeaderMap, HeaderName, HeaderValue, HttpClient, HttpError, HttpRequest,
    HttpResponse, Outcome, Retryable, SseStream, StatusCode, WebSocket, header,
};
use efr_provider::{
    AccessToken, ModelInfo, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream,
    Request, TokenSource,
};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use futures::{StreamExt as _, stream};
use jiff::Timestamp;
use serde_json::Value;
use tracing::Instrument as _;

use crate::OpenAiConfig;
use crate::catalog::{Catalog, ModelCatalog, with_extra};
use crate::config::Backend;
use crate::convert::{ResponsesBody, request_body};
use crate::sse_events::{EventMapper, retry_hint};
use crate::timing::Timing;
use crate::websocket::{Attempt, CONNECT_TIMEOUT, ConnectError, Health, Rejection, Sockets};

/// The header that routes a subscription request to the account the token belongs to
/// (goose `chatgpt_codex.rs`, `post_streaming`; Codex sends it as `ChatGPT-Account-ID`,
/// and header names are case-insensitive).
const ACCOUNT_HEADER: &str = "chatgpt-account-id";

/// The header that names the organization that an API request bills
/// (<https://developers.openai.com/api/reference/overview>).
const ORGANIZATION_HEADER: &str = "openai-organization";

/// The header that names the project that an API request bills.
const PROJECT_HEADER: &str = "openai-project";

/// The header that names the client to the subscription backend (Codex
/// `codex-rs/login/src/auth/default_client.rs`, `add_originator_header`).
const ORIGINATOR_HEADER: &str = "originator";

/// The header that gives the subscription backend the request's prompt cache key. The
/// backend routes a request to its prompt cache by it (Codex
/// `codex-rs/core/src/client.rs`, `responses_session_id`, and
/// `codex-rs/codex-api/src/requests/headers.rs`, `build_session_headers`).
const SESSION_HEADER: &str = "session-id";

/// The header that asks for the Responses WebSocket protocol that Codex speaks
/// (`codex-rs/core/src/client.rs`, `RESPONSES_WEBSOCKETS_V2_BETA_HEADER_VALUE`).
const BETA_HEADER: &str = "openai-beta";

/// The value of [`BETA_HEADER`] on a WebSocket handshake.
const WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";

/// The response header that carries the server's id of the request, for the log.
const REQUEST_ID_HEADER: &str = "x-request-id";

/// The longest error message kept from a response body, in characters.
const MAX_ERROR_MESSAGE: usize = 1000;

/// Error codes of a 429 that mean money or plan, not pace: waiting does not help, so
/// they are reported as API errors rather than as rate limiting, and such a 429 is
/// never sent again (the list Codex maps to `QuotaExceeded` and `UsageNotIncluded` in
/// `codex-rs/codex-api/src/api_bridge.rs`).
const QUOTA_CODES: &[&str] = &[
    "insufficient_quota",
    "usage_not_included",
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
];

/// A [`Provider`] over OpenAI's Responses API, on the ChatGPT subscription backend or
/// the public API (see [`OpenAiConfig`]).
///
/// Each [`Provider::stream`] posts `<base_url>/responses` with `stream: true` and
/// turns the server-sent events into [`ProviderEvent`]s. A call for a model that uses
/// the WebSocket transport (see [`WebSocketMode`](crate::WebSocketMode)) goes as a
/// `response.create` message on the conversation's WebSocket instead, and over HTTP
/// whenever the socket cannot serve it. The token comes from the
/// [`TokenSource`] for every request; the provider never sees a refresh token. When the
/// server answers 401, the provider invalidates the token, fetches a new one and sends
/// the request once more; a second 401 is [`ProviderError::Unauthorized`], and so is
/// the first one for a token source that cannot refresh, such as an API key. Other
/// retries follow the config's `efr_http::RetryPolicy`, which sends a `POST` again only
/// when the server certainly did not act on it, so a model call never runs twice. The
/// body of a 429 is read before the policy decides: a 429 for money or plan, such as
/// `insufficient_quota`, is never sent again.
///
/// The `Done` event carries the response's output items verbatim as `provider_raw`,
/// and a later request sends them back unchanged, so encrypted reasoning survives a
/// stateless (`store: false`) conversation.
///
/// Which models take freeform tools, and the output limit that the API path sends,
/// come from the [`ModelCatalog`], read from memory for each request, with the models
/// of the config laid over it.
#[derive(Debug)]
pub struct OpenAiProvider {
    id: ProviderId,
    config: OpenAiConfig,
    catalog: ModelCatalog,
    http: HttpClient,
    tokens: Arc<dyn TokenSource>,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    sockets: Sockets,
}

impl OpenAiProvider {
    /// A provider named `id` that sends requests through `http` with tokens from
    /// `tokens`. `clock` dates the wait of a rate limit that names the time it resets
    /// and waits between two tries; `rng` spreads those waits. Its catalog is the table
    /// built into efr until [`with_catalog`](Self::with_catalog) gives it a shared one.
    pub fn new(
        id: ProviderId,
        config: OpenAiConfig,
        http: HttpClient,
        tokens: Arc<dyn TokenSource>,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Self {
        let catalog = ModelCatalog::new(Catalog::builtin(config.backend()));
        let sockets = Sockets::new(Arc::clone(&clock));
        OpenAiProvider { id, config, catalog, http, tokens, clock, rng, sockets }
    }

    /// The same provider reading its models from `catalog`, which a fetch in the
    /// background can replace while the provider runs.
    #[must_use]
    pub fn with_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// The provider's WebSocket connections.
    #[cfg(test)]
    pub(crate) fn sockets(&self) -> &Sockets {
        &self.sockets
    }

    /// The provider's settings.
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// The request for `request` without its credentials: built once, then cloned for
    /// each attempt, since only the token may change between attempts.
    fn unsigned(&self, body: &ResponsesBody) -> Result<HttpRequest, HttpError> {
        let mut unsigned = HttpRequest::post(&self.config.responses_url())?
            .json(body)?
            .header(header::ACCEPT, HeaderValue::from_static("text/event-stream"));
        if self.config.backend() == Backend::Subscription
            && let Some(key) = body.prompt_cache_key()
        {
            unsigned = unsigned.header_text(HeaderName::from_static(SESSION_HEADER), key)?;
        }
        Ok(unsigned.recorded())
    }

    /// Opens a WebSocket to the Responses endpoint for the conversation of `body`, with
    /// the headers of the HTTP path and the WebSocket beta header.
    async fn connect(&self, body: &ResponsesBody) -> Result<WebSocket, ConnectError> {
        let token = self.tokens.access_token().await.map_err(|error| ConnectError {
            reason: format!("no access token: {error}"),
            health: Health::Fine,
        })?;
        let request = self.websocket_request(body, &token).map_err(|error| ConnectError {
            reason: format!("the handshake could not be built: {error}"),
            health: Health::Broken,
        })?;
        self.http.websocket(&request).await.map_err(|error| {
            let unauthorized = matches!(
                &error,
                HttpError::UpgradeRefused { status, .. } if *status == StatusCode::UNAUTHORIZED
            );
            let health = if unauthorized { Health::Unauthorized } else { Health::Broken };
            ConnectError { reason: error.to_string(), health }
        })
    }

    fn websocket_request(
        &self,
        body: &ResponsesBody,
        token: &AccessToken,
    ) -> Result<HttpRequest, HttpError> {
        let mut request = HttpRequest::get(&self.config.responses_url())?
            .header(HeaderName::from_static(BETA_HEADER), HeaderValue::from_static(WEBSOCKET_BETA))
            .timeout(CONNECT_TIMEOUT);
        if self.config.backend() == Backend::Subscription
            && let Some(key) = body.prompt_cache_key()
        {
            request = request.header_text(HeaderName::from_static(SESSION_HEADER), key)?;
        }
        sign(&request, &self.config, token)
    }

    /// Sends the request and returns the successful response, with one token refresh on
    /// a 401 when the token source can refresh.
    async fn post(
        &self,
        unsigned: &HttpRequest,
        model: &str,
    ) -> Result<HttpResponse, ProviderError> {
        let mut refreshed = false;
        loop {
            let token = self.tokens.access_token().await?;
            let request = sign(unsigned, &self.config, &token).map_err(transport)?;
            let response = match self.send(&request).await? {
                Answer::Response(response) => response,
                Answer::TooManyRequests { headers, body, .. } => {
                    let header_wait = efr_http::retry_after(&headers, self.clock.now());
                    let status = StatusCode::TOO_MANY_REQUESTS;
                    return Err(self.answer_error(status, header_wait, &body, model));
                }
            };
            if let Some(id) =
                response.headers().get(REQUEST_ID_HEADER).and_then(|value| value.to_str().ok())
            {
                tracing::Span::current().record("request_id", id);
            }
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            if status != StatusCode::UNAUTHORIZED {
                return Err(self.status_error(response, model).await);
            }
            if refreshed || !self.tokens.refreshable() {
                return Err(unauthorized(response).await);
            }
            tracing::warn!("the provider refused the access token; refreshing it once");
            drop(response);
            self.tokens.invalidate().await;
            refreshed = true;
        }
    }

    /// Sends `request` while the config's retry policy says so and returns the last
    /// answer. A 429 is read before the policy decides, so a 429 for money or plan
    /// ends the tries at once; any other status decides as
    /// `efr_http::HttpClient::send_with_retry` does.
    async fn send(&self, request: &HttpRequest) -> Result<Answer, ProviderError> {
        let idempotent = request.is_idempotent();
        let sent: Result<Sent, Unanswered> = self
            .config
            .retry()
            .run(&*self.clock, &*self.rng, |_attempt| async move {
                let response = self
                    .http
                    .send(request)
                    .await
                    .map_err(|error| Unanswered { error, idempotent })?;
                if response.status() != StatusCode::TOO_MANY_REQUESTS {
                    return Ok(Sent { answer: Answer::Response(response), idempotent });
                }
                let headers = response.headers().clone();
                // NOTE: a body that breaks off came after the server took the request,
                // so it is never a reason to send the request again.
                let body = response
                    .text()
                    .await
                    .map_err(|error| Unanswered { error, idempotent: false })?;
                let quota = ErrorDetails::parse(&body).is_quota();
                Ok(Sent { answer: Answer::TooManyRequests { headers, body, quota }, idempotent })
            })
            .await;
        match sent {
            Ok(sent) => Ok(sent.answer),
            Err(unanswered) => Err(transport(unanswered.error)),
        }
    }

    /// The error for a response that is neither a success nor a 401.
    async fn status_error(&self, response: HttpResponse, model: &str) -> ProviderError {
        let status = response.status();
        let header_wait = efr_http::retry_after(response.headers(), self.clock.now());
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => return transport(error),
        };
        self.answer_error(status, header_wait, &body, model)
    }

    /// The error for an `error` event with an HTTP error status on the WebSocket: the
    /// same as for an HTTP answer with that status, with the event as its body and its
    /// `headers` member as the headers.
    fn rejection_error(&self, rejection: &Rejection, model: &str) -> ProviderError {
        let Rejection { status, event } = rejection;
        let mut headers = HeaderMap::new();
        if let Some(members) = event.get("headers").and_then(Value::as_object) {
            for (name, value) in members {
                let text = match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                if let (Ok(name), Ok(value)) =
                    (HeaderName::try_from(name.as_str()), HeaderValue::from_str(&text))
                {
                    headers.insert(name, value);
                }
            }
        }
        let header_wait = efr_http::retry_after(&headers, self.clock.now());
        let status = StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_REQUEST);
        self.answer_error(status, header_wait, &event.to_string(), model)
    }

    /// The error for an error answer with `status` and `body`; `header_wait` is the
    /// wait that its headers ask for.
    fn answer_error(
        &self,
        status: StatusCode,
        header_wait: Option<Duration>,
        body: &str,
        model: &str,
    ) -> ProviderError {
        let now = self.clock.now();
        let details = ErrorDetails::parse(body);
        if status == StatusCode::TOO_MANY_REQUESTS && !details.is_quota() {
            let reset_wait = details.resets_at.map(|at| {
                let seconds = at.saturating_sub(now.as_second());
                Duration::from_secs(u64::try_from(seconds).unwrap_or(0))
            });
            let retry_after = header_wait
                .or(reset_wait)
                .or(details.resets_in_seconds.map(Duration::from_secs))
                .or_else(|| details.message.as_deref().and_then(retry_hint));
            return ProviderError::RateLimited { retry_after };
        }
        if details.names_unknown_model(status) {
            return ProviderError::UnknownModel { model: model.to_owned() };
        }
        let message = details.message.clone().unwrap_or_else(|| {
            let shown: String = body.trim().chars().take(MAX_ERROR_MESSAGE).collect();
            if shown.is_empty() {
                status.canonical_reason().unwrap_or("no message").to_owned()
            } else {
                shown
            }
        });
        let code = details.code.or(details.kind);
        ProviderError::api(Some(status.as_u16()), code, message)
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        with_extra(self.catalog.current().models(), self.config.models())
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let span = tracing::info_span!(
            "provider_request",
            provider = %self.id,
            model = %request.model,
            request_id = tracing::field::Empty,
        );
        let timing = Timing::start();
        let model = self.models().into_iter().find(|model| model.id == request.model);
        let body = request_body(&request, &self.config, model.as_ref());
        if self.config.uses_websocket(model.as_ref())
            && let Some(key) = body.prompt_cache_key()
        {
            let connect = || self.connect(&body);
            let attempt = self
                .sockets
                .stream(key, &body, connect, timing.clone(), span.clone())
                .instrument(span.clone())
                .await;
            match attempt {
                Attempt::Answered(stream) => return Ok(stream),
                Attempt::Rejected(rejection) => {
                    span.in_scope(|| {
                        tracing::debug!(status = rejection.status, "the server refused the model call on the websocket");
                    });
                    return Err(self.rejection_error(&rejection, &request.model));
                }
                Attempt::Http(reason) => span.in_scope(|| {
                    tracing::warn!(reason = %reason, "the websocket could not serve the model call; it goes over HTTP");
                }),
                Attempt::Skipped(reason) => span.in_scope(|| {
                    tracing::debug!(reason = %reason, "the model call goes over HTTP");
                }),
            }
        }
        let unsigned = self.unsigned(&body).map_err(transport)?;
        let response = self.post(&unsigned, &request.model).instrument(span.clone()).await?;
        span.in_scope(|| {
            tracing::debug!(status = response.status().as_u16(), "the response is streaming")
        });
        Ok(events(response, span, timing))
    }
}

/// One answer of the server to a model call.
enum Answer {
    /// Any status but 429; its body is not read yet.
    Response(HttpResponse),
    /// A 429, with its body read to tell a quota from a rate limit.
    TooManyRequests {
        headers: HeaderMap,
        body: String,
        /// The body names a quota or a plan limit, so another try cannot pass.
        quota: bool,
    },
}

/// An answer, as the retry policy sees it.
struct Sent {
    answer: Answer,
    idempotent: bool,
}

impl Retryable for Sent {
    fn outcome(&self, now: Timestamp) -> Outcome {
        match &self.answer {
            Answer::Response(response)
                if efr_http::is_retryable_status(response.status(), self.idempotent) =>
            {
                Outcome::Transient { retry_after: efr_http::retry_after(response.headers(), now) }
            }
            Answer::Response(_) | Answer::TooManyRequests { quota: true, .. } => Outcome::Final,
            Answer::TooManyRequests { headers, .. } => {
                Outcome::Transient { retry_after: efr_http::retry_after(headers, now) }
            }
        }
    }
}

/// A call that got no answer, or whose 429 body could not be read.
struct Unanswered {
    error: HttpError,
    idempotent: bool,
}

impl Retryable for Unanswered {
    fn outcome(&self, _now: Timestamp) -> Outcome {
        if self.error.is_retryable(self.idempotent) {
            Outcome::Transient { retry_after: None }
        } else {
            Outcome::Final
        }
    }
}

/// The error for a 401 that no refresh can fix, with the message of its body.
async fn unauthorized(response: HttpResponse) -> ProviderError {
    let message = match response.text().await {
        Ok(body) => ErrorDetails::parse(&body).message,
        Err(_) => None,
    };
    ProviderError::Unauthorized { message }
}

/// The error body of a failed response, in the shapes OpenAI's servers use:
/// `{"error": {"message", "type", "code", ...}}` from the API and
/// `{"detail": ...}` from the subscription backend.
#[derive(Debug, Default)]
struct ErrorDetails {
    code: Option<String>,
    kind: Option<String>,
    message: Option<String>,
    resets_at: Option<i64>,
    resets_in_seconds: Option<u64>,
}

impl ErrorDetails {
    fn parse(body: &str) -> ErrorDetails {
        let Ok(value) = serde_json::from_str::<Value>(body) else {
            return ErrorDetails::default();
        };
        let error = match value.get("error") {
            Some(Value::String(message)) => {
                return ErrorDetails { message: Some(clip(message)), ..ErrorDetails::default() };
            }
            Some(error @ Value::Object(_)) => error,
            _ => match value.get("detail") {
                Some(Value::String(message)) => {
                    return ErrorDetails {
                        message: Some(clip(message)),
                        ..ErrorDetails::default()
                    };
                }
                Some(detail @ Value::Object(_)) => detail,
                _ => &value,
            },
        };
        let text = |key: &str| error.get(key).and_then(Value::as_str);
        ErrorDetails {
            code: text("code").map(str::to_owned),
            kind: text("type").map(str::to_owned),
            message: text("message").filter(|message| !message.trim().is_empty()).map(clip),
            resets_at: error.get("resets_at").and_then(Value::as_i64),
            resets_in_seconds: error.get("resets_in_seconds").and_then(Value::as_u64),
        }
    }

    fn codes(&self) -> impl Iterator<Item = &str> {
        self.code.as_deref().into_iter().chain(self.kind.as_deref())
    }

    /// True when the error is about money or plan, which waiting does not fix.
    fn is_quota(&self) -> bool {
        self.codes().any(|code| QUOTA_CODES.contains(&code))
    }

    /// True when the server says it does not serve the requested model.
    fn names_unknown_model(&self, status: StatusCode) -> bool {
        if self.codes().any(|code| code == "model_not_found") {
            return true;
        }
        let message = self.message.as_deref().unwrap_or_default().to_ascii_lowercase();
        status == StatusCode::BAD_REQUEST
            && message.contains("model")
            && (message.contains("not supported") || message.contains("does not exist"))
    }
}

/// The code and the message of an error body in the shapes that [`ErrorDetails`]
/// reads, for an answer that is not a model call's.
pub(crate) fn server_error(body: &str) -> (Option<String>, Option<String>) {
    let details = ErrorDetails::parse(body);
    (details.code.or(details.kind), details.message)
}

fn clip(message: &str) -> String {
    message.chars().take(MAX_ERROR_MESSAGE).collect()
}

fn transport(error: HttpError) -> ProviderError {
    ProviderError::Transport { source: Box::new(error) }
}

/// `unsigned` with the credentials of `token` and, on the subscription path, the
/// `originator` and account headers, or on the API path the organization and project
/// headers that the config names. Model requests, catalog fetches and the key check
/// send the same.
pub(crate) fn sign(
    unsigned: &HttpRequest,
    config: &OpenAiConfig,
    token: &AccessToken,
) -> Result<HttpRequest, HttpError> {
    let mut request = unsigned.clone().bearer_auth(token.secret())?;
    match config.backend() {
        Backend::Subscription => {
            request = request
                .header_text(HeaderName::from_static(ORIGINATOR_HEADER), config.originator())?;
            if let Some(account_id) = token.account_id() {
                request =
                    request.header_text(HeaderName::from_static(ACCOUNT_HEADER), account_id)?;
            }
        }
        Backend::Api => {
            if let Some(organization) = config.organization() {
                request = request
                    .header_text(HeaderName::from_static(ORGANIZATION_HEADER), organization)?;
            }
            if let Some(project) = config.project() {
                request = request.header_text(HeaderName::from_static(PROJECT_HEADER), project)?;
            }
        }
    }
    Ok(request)
}

/// The events of a streaming response, until the answer is done or fails.
fn events(response: HttpResponse, span: tracing::Span, timing: Timing) -> ProviderStream {
    let state = EventState {
        sse: Some(response.into_sse()),
        mapper: EventMapper::new(),
        pending: VecDeque::new(),
        timing,
    };
    Box::pin(stream::unfold(state, move |mut state| {
        let span = span.clone();
        async move { state.next().await.map(|item| (item, state)) }.instrument(span)
    }))
}

struct EventState {
    /// The body, dropped as soon as the answer has ended so the connection is released
    /// even if the consumer keeps the stream.
    sse: Option<SseStream<ByteStream>>,
    mapper: EventMapper,
    pending: VecDeque<Result<ProviderEvent, ProviderError>>,
    timing: Timing,
}

impl EventState {
    async fn next(&mut self) -> Option<Result<ProviderEvent, ProviderError>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                if item.is_ok() {
                    self.timing.first_event();
                }
                return Some(item);
            }
            let sse = self.sse.as_mut()?;
            let next = sse.next().await;
            if matches!(next, Some(Ok(_))) {
                self.timing.accepted();
            }
            let ended = match next {
                Some(Ok(event)) => match self.mapper.map(&event) {
                    Ok(events) => {
                        self.pending.extend(events.into_iter().map(Ok));
                        self.mapper.is_done()
                    }
                    Err(error) => {
                        self.pending.push_back(Err(error));
                        true
                    }
                },
                Some(Err(error)) => {
                    self.pending.push_back(Err(transport(error)));
                    true
                }
                None => {
                    self.pending.push_back(Err(ProviderError::Incomplete));
                    true
                }
            };
            if ended {
                self.sse = None;
            }
        }
    }
}

#[cfg(test)]
mod tests;
