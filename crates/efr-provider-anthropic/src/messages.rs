//! The Messages API client: [`AnthropicProvider`].
//!
//! Each call is one `POST <base_url>/messages` with `stream: true`, signed with
//! `Authorization: Bearer <key>` through `efr_http::HttpRequest::bearer_auth` (the
//! header is sensitive, so no record shows it), `anthropic-version`
//! ([`ANTHROPIC_VERSION`](crate::ANTHROPIC_VERSION)), the optional
//! `anthropic-workspace-id` and `anthropic-beta: thinking-binding-controls-2026-08-01`
//! for `drop_block`. The body comes from `convert`, the events go through
//! `sse_events`, an error answer through `failure`, and the attempts through the
//! config's `efr_http::RetryPolicy` on the injected clock and random source. The
//! provider reads no `provider_options` key: the conversation sends OpenAI's
//! `prompt_cache_key` to every provider, and an unknown body member is a 400. The
//! response's `request-id` goes into the request's span and into every failure line.

use std::sync::Arc;

use async_trait::async_trait;
use efr_http::{HeaderName, HeaderValue, HttpClient, HttpError, HttpRequest};
use efr_provider::{
    ModelInfo, Provider, ProviderError, ProviderId, ProviderStream, Request, SecretString,
    TokenSource,
};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;

use crate::failure::not_built;
use crate::{ANTHROPIC_VERSION, AnthropicConfig, ModelCatalog};

/// The header that names the version of the API.
const VERSION_HEADER: &str = "anthropic-version";

/// The header that names the workspace of a key that is not scoped to one.
const WORKSPACE_HEADER: &str = "anthropic-workspace-id";

/// A [`Provider`] over Anthropic's Messages API with an API key (see
/// [`AnthropicConfig`]).
///
/// Each [`Provider::stream`] posts `<base_url>/messages` and turns the server-sent
/// events into canonical events. The key comes from the [`TokenSource`] for every
/// request; a 401 fails at once with [`ProviderError::Unauthorized`] and the server's
/// message, because a key cannot refresh. A model call is never sent again after its
/// stream has started.
///
/// The `Done` event carries the message's content as its exact JSON text, as
/// `provider_raw`, and a later request to the same model sends it back byte for byte,
/// so the signed thinking stays valid. The models, their windows and their efforts come
/// from the [`ModelCatalog`], read from memory for each request, with the models of the
/// config laid over it.
#[derive(Debug)]
pub struct AnthropicProvider {
    id: ProviderId,
    config: AnthropicConfig,
    catalog: ModelCatalog,
    #[expect(dead_code, reason = "the model call is not built yet")]
    http: HttpClient,
    #[expect(dead_code, reason = "the model call is not built yet")]
    tokens: Arc<dyn TokenSource>,
    #[expect(dead_code, reason = "the model call is not built yet")]
    clock: Arc<dyn Clock>,
    #[expect(dead_code, reason = "the model call is not built yet")]
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
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.catalog.current().map(|catalog| catalog.models()).unwrap_or_default()
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        // NOTE: the model call is not built yet; the request is not sent anywhere.
        drop(request);
        Err(not_built())
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
