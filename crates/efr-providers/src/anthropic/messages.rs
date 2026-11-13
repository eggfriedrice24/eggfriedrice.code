//! The Messages API client: [`AnthropicProvider`].
//!
//! Each call is one `POST <base_url>/messages` with `stream: true`, signed with
//! `Authorization: Bearer <key>` through `efr_http::HttpRequest::bearer_auth` (the
//! header is sensitive, so no record shows it), `anthropic-version`
//! ([`ANTHROPIC_VERSION`]), the optional `anthropic-workspace-id` and the
//! `anthropic-beta` values that the body needs (`thinking-binding-controls-2026-08-01`
//! for `drop_block`). The body comes from `convert`, the events go through
//! `sse_events`, an error answer through `failure`, and the attempts through the
//! config's `efr_http::RetryPolicy` on the injected clock and random source. The
//! provider reads no `provider_options` key: the conversation sends OpenAI's
//! `prompt_cache_key` to every provider, and an unknown body member is a 400. The
//! response's `request-id` goes into the request's span and into every failure line.
//!
//! The provider has the live web (`WebSupport::Live`): a request with
//! `Request::hosted` gets the API's web search and web fetch tools (see `convert`), and
//! its stream holds back the trailing punctuation of each text block for the mark of a
//! citation (see `sse_events`). An answer that fails or ends early first sends that
//! held text.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use efr_http::{
    ByteStream, HeaderName, HeaderValue, HttpClient, HttpError, HttpRequest, HttpResponse,
    SseStream, header,
};
use efr_provider::{
    CallNotice, CallNotices, EditTool, ModelInfo, NoNotices, Provider, ProviderError,
    ProviderEvent, ProviderId, ProviderStream, Request, SecretString, TokenSource, WebSupport,
};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use futures::{StreamExt as _, stream};
use tracing::Instrument as _;

use crate::anthropic::catalog::with_extra;
use crate::anthropic::convert::{MessagesBody, request_body};
use crate::anthropic::failure::{self, Answered, Failure, transport, without_key};
use crate::anthropic::sse_events::EventMapper;
use crate::anthropic::timing::Timing;
use crate::anthropic::{ANTHROPIC_VERSION, AnthropicConfig, ModelCatalog};

/// The header that names the version of the API.
const VERSION_HEADER: &str = "anthropic-version";

/// The header that names the workspace of a key that is not scoped to one.
const WORKSPACE_HEADER: &str = "anthropic-workspace-id";

/// The header that turns on beta features, as one comma-separated list.
const BETA_HEADER: &str = "anthropic-beta";

/// The response header that carries the server's id of the request, for the log.
const REQUEST_ID_HEADER: &str = "request-id";

/// A [`Provider`] over Anthropic's Messages API with an API key (see
/// [`AnthropicConfig`]).
///
/// Each [`Provider::stream`] posts `<base_url>/messages` and turns the server-sent
/// events into canonical events. The key comes from the [`TokenSource`] for every
/// request; a 401 fails at once with [`ProviderError::Unauthorized`] and the server's
/// message, because a key cannot refresh (a token source that can refresh gets one
/// refresh first). A 429 that is not a spend cap, a 5xx and a 529 are sent again by the
/// config's retry policy, but a model call is never sent again after its stream has
/// started.
///
/// The `Done` event carries the message's content as the JSON text that efr built from
/// the stream, as `provider_raw`, and a later request to the same model sends it back
/// byte for byte, so the signed thinking stays valid. The models, their windows and their efforts come
/// from the [`ModelCatalog`], read from memory for each request, with the models of the
/// config laid over it.
#[derive(Debug)]
pub struct AnthropicProvider {
    id: ProviderId,
    config: AnthropicConfig,
    catalog: ModelCatalog,
    http: HttpClient,
    tokens: Arc<dyn TokenSource>,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
}

impl AnthropicProvider {
    /// A provider named `id` that sends requests through `http` with the key from
    /// `tokens`. `clock` and `rng` time the retries. Its catalog has no list until
    /// [`with_catalog`](Self::with_catalog) gives it a shared one.
    pub fn new(
        id: ProviderId,
        config: AnthropicConfig,
        http: HttpClient,
        tokens: Arc<dyn TokenSource>,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Self {
        AnthropicProvider { id, config, catalog: ModelCatalog::new(), http, tokens, clock, rng }
    }

    /// The same provider reading its models from `catalog`, which a fetch in the
    /// background can replace while the provider runs.
    #[must_use]
    pub fn with_catalog(mut self, catalog: ModelCatalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// The provider's settings.
    pub fn config(&self) -> &AnthropicConfig {
        &self.config
    }

    /// The request for `body` without its credentials: built once, then signed for each
    /// round of attempts.
    fn unsigned(&self, body: &MessagesBody) -> Result<HttpRequest, HttpError> {
        let mut unsigned = HttpRequest::post(&self.config.messages_url())?
            .json(body)?
            .header(header::ACCEPT, HeaderValue::from_static("text/event-stream"));
        let betas = body.betas();
        if !betas.is_empty() {
            unsigned =
                unsigned.header_text(HeaderName::from_static(BETA_HEADER), &betas.join(","))?;
        }
        Ok(unsigned.recorded())
    }

    /// Sends the request through the retry policy and returns the successful response.
    /// A 401 from a token source that can refresh invalidates the token and runs the
    /// policy once more.
    async fn post(
        &self,
        unsigned: &HttpRequest,
        model: &str,
        notices: &dyn CallNotices,
    ) -> Result<HttpResponse, ProviderError> {
        let mut refreshed = false;
        loop {
            efr_stdx::mark::hit("prov.post.begin");
            let token = self.tokens.access_token().await?;
            efr_stdx::mark::hit("prov.token");
            let request = sign(unsigned, &self.config, token.secret()).map_err(transport)?;
            let retrying = |attempt, result: &Result<Answered, Failure>, wait| {
                let status = result.as_ref().err().and_then(|failure| status_of(&failure.error));
                notices.notice(CallNotice::Retrying { attempt, wait, status });
            };
            let result = self
                .config
                .retry()
                .run_reporting(
                    &*self.clock,
                    &*self.rng,
                    |attempt| {
                        notices.notice(CallNotice::Connecting);
                        self.attempt(&request, token.secret(), model, attempt)
                    },
                    retrying,
                )
                .await;
            match result {
                Ok(Answered(response)) => {
                    notices.notice(CallNotice::Connected);
                    return Ok(response);
                }
                Err(Failure { error: ProviderError::Unauthorized { .. }, .. })
                    if self.tokens.refreshable() && !refreshed =>
                {
                    tracing::warn!("the provider refused the token; refreshing it once");
                    self.tokens.invalidate().await;
                    refreshed = true;
                }
                Err(failure) => return Err(failure.error),
            }
        }
    }

    /// One attempt: the response when its status is a success, else what its error
    /// body, without `key`, means.
    async fn attempt(
        &self,
        request: &HttpRequest,
        key: &SecretString,
        model: &str,
        attempt: u32,
    ) -> Result<Answered, Failure> {
        efr_stdx::mark::hit("prov.attempt");
        let response = self.http.send(request).await.map_err(Failure::transport)?;
        efr_stdx::mark::hit("prov.headers");
        let request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if let Some(id) = &request_id {
            tracing::Span::current().record("request_id", id.as_str());
        }
        let status = response.status();
        if status.is_success() {
            return Ok(Answered(response));
        }
        let retry_after = efr_http::retry_after(response.headers(), self.clock.now());
        let body = response.text().await.map_err(Failure::transport)?;
        let failure = failure::answer(status, retry_after, &without_key(&body, key), Some(model));
        tracing::debug!(
            attempt,
            status = status.as_u16(),
            request_id = request_id.as_deref().unwrap_or("none"),
            again = ?failure.again,
            "the provider refused the model call"
        );
        Err(failure)
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        let catalog = self.catalog.current().map(|catalog| catalog.models()).unwrap_or_default();
        with_extra(catalog, self.config.models())
    }

    fn default_edit_tool(&self) -> EditTool {
        EditTool::Replace
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.stream_with_notices(request, &NoNotices).await
    }

    async fn stream_with_notices(
        &self,
        request: Request,
        notices: &dyn CallNotices,
    ) -> Result<ProviderStream, ProviderError> {
        let span = tracing::info_span!(
            "provider_request",
            provider = %self.id,
            model = %request.model,
            request_id = tracing::field::Empty,
        );
        let timing = Timing::start();
        efr_stdx::mark::hit("prov.begin");
        let model = self.models().into_iter().find(|model| model.id == request.model);
        if model.is_none() {
            // NOTE: without a login no catalog fetch could run, so the model is not
            // listed; the missing login is what the user must fix, not the model.
            self.tokens.access_token().await?;
        }
        let body = request_body(&request, &self.config, model.as_ref())?;
        efr_stdx::mark::hit("prov.body");
        let unsigned = self.unsigned(&body).map_err(transport)?;
        efr_stdx::mark::hit("prov.encoded");
        let response =
            self.post(&unsigned, &request.model, notices).instrument(span.clone()).await?;
        span.in_scope(|| {
            tracing::debug!(status = response.status().as_u16(), "the response is streaming")
        });
        Ok(events(response, span, timing, body.carries_web_tools()))
    }

    fn web_search(&self, _effort: Option<&str>) -> WebSupport {
        WebSupport::Live
    }
}

/// The HTTP status of a failed attempt, for the notice of its retry: `None` when no
/// answer came.
fn status_of(error: &ProviderError) -> Option<u16> {
    match error {
        ProviderError::RateLimited { .. } => Some(429),
        ProviderError::Overloaded { status, .. }
        | ProviderError::Api { status, .. }
        | ProviderError::ContextOverflow { status, .. } => *status,
        _ => None,
    }
}

/// `unsigned` with the key and the headers of every request to the API: model calls,
/// fetches of the model list and key checks send the same.
pub(crate) fn sign(
    unsigned: &HttpRequest,
    config: &AnthropicConfig,
    key: &SecretString,
) -> Result<HttpRequest, HttpError> {
    let mut request = unsigned.clone().bearer_auth(key)?.header(
        HeaderName::from_static(VERSION_HEADER),
        HeaderValue::from_static(ANTHROPIC_VERSION),
    );
    if let Some(workspace_id) = config.workspace_id() {
        request = request.header_text(HeaderName::from_static(WORKSPACE_HEADER), workspace_id)?;
    }
    Ok(request)
}

/// The events of a streaming response, until the answer is done or fails. Nothing here
/// sends the request again: the stream has started. `web` is true when the request
/// carries the web tools (see `sse_events`).
fn events(
    response: HttpResponse,
    span: tracing::Span,
    timing: Timing,
    web: bool,
) -> ProviderStream {
    let request_id = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let state = EventState {
        sse: Some(response.into_sse()),
        mapper: EventMapper::new().with_web_tools(web),
        pending: VecDeque::new(),
        timing,
        request_id,
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
    /// The API's id of the request, for the line of a failed answer.
    request_id: Option<String>,
}

impl EventState {
    async fn next(&mut self) -> Option<Result<ProviderEvent, ProviderError>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                match &item {
                    Ok(_) => self.timing.first_event(),
                    // NOTE: an error ends the answer, so this line comes once per call;
                    // the request id is what the API's support asks for.
                    Err(error) => tracing::info!(
                        request_id = self.request_id.as_deref().unwrap_or("none"),
                        error = %error,
                        "the model call failed"
                    ),
                }
                return Some(item);
            }
            let sse = self.sse.as_mut()?;
            let next = sse.next().await;
            if matches!(next, Some(Ok(_))) {
                self.timing.accepted();
            }
            let failed = match next {
                Some(Ok(event)) => match self.mapper.map(&event) {
                    Ok(events) => {
                        self.pending.extend(events.into_iter().map(Ok));
                        None
                    }
                    Err(error) => Some(error),
                },
                Some(Err(error)) => Some(transport(error)),
                None => Some(ProviderError::Incomplete),
            };
            // NOTE: the text that a block held back for a citation mark still reaches
            // the turn before the error, so a failed answer keeps all of its text.
            let ended = match failed {
                Some(error) => {
                    self.pending.extend(self.mapper.take_held().into_iter().map(Ok));
                    self.pending.push_back(Err(error));
                    true
                }
                None => self.mapper.is_done(),
            };
            if ended {
                self.sse = None;
            }
        }
    }
}

#[cfg(test)]
mod tests;
