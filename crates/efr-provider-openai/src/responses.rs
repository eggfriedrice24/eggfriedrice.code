//! The Responses API client.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_http::{
    ByteStream, HeaderName, HeaderValue, HttpClient, HttpError, HttpRequest, HttpResponse,
    SseStream, StatusCode, header,
};
use efr_provider::{
    AccessToken, ModelInfo, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream,
    Request, TokenSource,
};
use efr_stdx::time::Clock;
use futures::{StreamExt as _, stream};
use serde_json::Value;
use tracing::Instrument as _;

use crate::OpenAiConfig;
use crate::config::Backend;
use crate::convert::request_body;
use crate::sse_events::{EventMapper, retry_hint};

/// The header that routes a subscription request to the account the token belongs to
/// (goose `chatgpt_codex.rs`, `post_streaming`; Codex sends it as `ChatGPT-Account-ID`,
/// and header names are case-insensitive).
const ACCOUNT_HEADER: &str = "chatgpt-account-id";

/// The header that names the client to the subscription backend (Codex
/// `codex-rs/login/src/auth/default_client.rs`, `add_originator_header`).
const ORIGINATOR_HEADER: &str = "originator";

/// The header that gives the subscription backend the request's prompt cache key. The
/// backend routes a request to its prompt cache by it (Codex
/// `codex-rs/core/src/client.rs`, `responses_session_id`, and
/// `codex-rs/codex-api/src/requests/headers.rs`, `build_session_headers`).
const SESSION_HEADER: &str = "session-id";

/// The response header that carries the server's id of the request, for the log.
const REQUEST_ID_HEADER: &str = "x-request-id";

/// The longest error message kept from a response body, in characters.
const MAX_ERROR_MESSAGE: usize = 1000;

/// Error codes of a 429 that mean money or plan, not pace: waiting does not help, so
/// they are reported as API errors rather than as rate limiting (the list Codex maps
/// to `QuotaExceeded` and `UsageNotIncluded` in `codex-rs/codex-api/src/api_bridge.rs`).
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
/// turns the server-sent events into [`ProviderEvent`]s. The token comes from the
/// [`TokenSource`] for every request; the provider never sees a refresh token. When the
/// server answers 401, the provider invalidates the token, fetches a new one and sends
/// the request once more; a second 401 is [`ProviderError::Unauthorized`]. Other
/// retries follow the config's `efr_http::RetryPolicy`, which sends a `POST` again only
/// when the server certainly did not act on it, so a model call never runs twice.
///
/// The `Done` event carries the response's output items verbatim as `provider_raw`,
/// and a later request sends them back unchanged, so encrypted reasoning survives a
/// stateless (`store: false`) conversation.
#[derive(Debug)]
pub struct OpenAiProvider {
    id: ProviderId,
    config: OpenAiConfig,
    http: HttpClient,
    tokens: Arc<dyn TokenSource>,
    clock: Arc<dyn Clock>,
}

impl OpenAiProvider {
    /// A provider named `id` that sends requests through `http` with tokens from
    /// `tokens`. `clock` dates the wait of a rate limit that names the time it resets.
    pub fn new(
        id: ProviderId,
        config: OpenAiConfig,
        http: HttpClient,
        tokens: Arc<dyn TokenSource>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        OpenAiProvider { id, config, http, tokens, clock }
    }

    /// The provider's settings.
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// The request for `request` without its credentials: built once, then cloned for
    /// each attempt, since only the token may change between attempts.
    fn unsigned(&self, request: &Request) -> Result<HttpRequest, HttpError> {
        let body = request_body(request, &self.config);
        let mut unsigned = HttpRequest::post(&self.config.responses_url())?
            .json(&body)?
            .header(header::ACCEPT, HeaderValue::from_static("text/event-stream"));
        if self.config.backend() == Backend::Subscription
            && let Some(key) = body.prompt_cache_key()
        {
            unsigned = unsigned.header_text(HeaderName::from_static(SESSION_HEADER), key)?;
        }
        Ok(unsigned.recorded())
    }

    fn signed(
        &self,
        unsigned: &HttpRequest,
        token: &AccessToken,
    ) -> Result<HttpRequest, HttpError> {
        let mut request = unsigned.clone().bearer_auth(token.secret())?;
        if self.config.backend() == Backend::Subscription {
            request = request.header_text(
                HeaderName::from_static(ORIGINATOR_HEADER),
                self.config.originator(),
            )?;
            if let Some(account_id) = token.account_id() {
                request =
                    request.header_text(HeaderName::from_static(ACCOUNT_HEADER), account_id)?;
            }
        }
        Ok(request)
    }

    /// Sends the request and returns the successful response, with one token refresh on
    /// a 401.
    async fn post(
        &self,
        unsigned: &HttpRequest,
        model: &str,
    ) -> Result<HttpResponse, ProviderError> {
        let mut refreshed = false;
        loop {
            let token = self.tokens.access_token().await?;
            let request = self.signed(unsigned, &token).map_err(transport)?;
            let response = self
                .http
                .send_with_retry(&request, self.config.retry())
                .await
                .map_err(transport)?;
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
            if refreshed {
                return Err(ProviderError::Unauthorized);
            }
            tracing::warn!("the provider refused the access token; refreshing it once");
            drop(response);
            self.tokens.invalidate().await;
            refreshed = true;
        }
    }

    /// The error for a response that is neither a success nor a 401.
    async fn status_error(&self, response: HttpResponse, model: &str) -> ProviderError {
        let status = response.status();
        let now = self.clock.now();
        let header_wait = efr_http::retry_after(response.headers(), now);
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => return transport(error),
        };
        let details = ErrorDetails::parse(&body);
        let quota = details.codes().any(|code| QUOTA_CODES.contains(&code));
        if status == StatusCode::TOO_MANY_REQUESTS && !quota {
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

    fn models(&self) -> &[ModelInfo] {
        self.config.models()
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let span = tracing::info_span!(
            "provider_request",
            provider = %self.id,
            model = %request.model,
            request_id = tracing::field::Empty,
        );
        let unsigned = self.unsigned(&request).map_err(transport)?;
        let response = self.post(&unsigned, &request.model).instrument(span.clone()).await?;
        span.in_scope(|| {
            tracing::debug!(status = response.status().as_u16(), "the response is streaming")
        });
        Ok(events(response, span))
    }
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

fn clip(message: &str) -> String {
    message.chars().take(MAX_ERROR_MESSAGE).collect()
}

fn transport(error: HttpError) -> ProviderError {
    ProviderError::Transport { source: Box::new(error) }
}

/// The events of a streaming response, until the answer is done or fails.
fn events(response: HttpResponse, span: tracing::Span) -> ProviderStream {
    let state = EventState {
        sse: Some(response.into_sse()),
        mapper: EventMapper::new(),
        pending: VecDeque::new(),
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
}

impl EventState {
    async fn next(&mut self) -> Option<Result<ProviderEvent, ProviderError>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Some(item);
            }
            let sse = self.sse.as_mut()?;
            let ended = match sse.next().await {
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
