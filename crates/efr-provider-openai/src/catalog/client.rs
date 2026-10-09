//! The fetch of the model catalog from the backend.
//!
//! `GET <base_url>/models` with the same credentials and headers as a model request
//! (Codex `codex-rs/codex-api/src/endpoint/models.rs`, `ModelsClient`). Codex adds its
//! own version as `client_version`, and the backend keeps back each model that needs a
//! newer Codex. efr sends no `client_version`, because it has no Codex version. A list that efr already has goes with its tag in `If-None-Match`,
//! and a 304 answer confirms it without a body.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use efr_http::{HeaderValue, HttpClient, HttpError, HttpRequest, StatusCode, Url, header};
use efr_provider::{ExposeSecret as _, ProviderError, TokenSource};
use efr_stdx::time::Clock;
use serde_json::Value;

use super::{Catalog, entries_of};
use crate::OpenAiConfig;
use crate::responses::sign;

/// How long one fetch may take. The fetch runs in the background, so this only bounds
/// how long a hung backend holds a connection.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// What a fetch got.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fetched {
    /// A list, maybe the same as before.
    Changed(Catalog),
    /// The backend confirmed the list whose tag efr sent.
    NotModified,
}

/// Fetches the catalog of the backend that an [`OpenAiConfig`] names.
#[derive(Debug, Clone)]
pub struct CatalogClient {
    config: OpenAiConfig,
    http: HttpClient,
    tokens: Arc<dyn TokenSource>,
    clock: Arc<dyn Clock>,
    /// The last fetch got a 401 also with a token refreshed for it: the backend
    /// refuses the catalog to tokens that it takes for model calls. Until a fetch
    /// works, a 401 forces no refresh.
    refuses_fresh: Arc<AtomicBool>,
}

impl CatalogClient {
    /// A client for the backend of `config`, sending through `http` with tokens from
    /// `tokens`. `clock` dates each list.
    pub fn new(
        config: OpenAiConfig,
        http: HttpClient,
        tokens: Arc<dyn TokenSource>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        CatalogClient {
            config,
            http,
            tokens,
            clock,
            refuses_fresh: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The base URL of the backend, which the cache file records.
    pub fn base_url(&self) -> &str {
        self.config.base_url()
    }

    /// The URL of the catalog, without a `client_version`.
    pub fn url(&self) -> Result<Url, HttpError> {
        Url::parse(&format!("{}/models", self.config.base_url()))
            .map_err(|source| HttpError::InvalidUrl { source })
    }

    /// Asks the backend for its catalog. `current` is the list that efr has: when the
    /// same backend sent it to this version of efr, its tag goes along, and the answer
    /// can be [`Fetched::NotModified`]. A 401 makes the token source forget its token,
    /// and the request goes once more with a new one. When that one gets a 401 too, a
    /// 401 forces no refresh until a fetch works: the token source refreshes a token
    /// by its age, and a model call refreshes a token that the backend refuses.
    pub async fn fetch(&self, current: &Catalog) -> Result<Fetched, ProviderError> {
        let etag = current.etag_for(self.config.base_url());
        let url = self.url().map_err(transport)?;
        let mut unsigned = HttpRequest::get(url.as_str())
            .map_err(transport)?
            .header(header::ACCEPT, HeaderValue::from_static("application/json"))
            .timeout(FETCH_TIMEOUT);
        if let Some(etag) = etag {
            unsigned = unsigned.header_text(header::IF_NONE_MATCH, etag).map_err(transport)?;
        }
        let mut refreshed = false;
        loop {
            let token = self.tokens.access_token().await?;
            let request = sign(&unsigned, &self.config, &token).map_err(transport)?;
            let response = self
                .http
                .send_with_retry(&request, self.config.retry())
                .await
                .map_err(transport)?;
            let status = response.status();
            if status == StatusCode::NOT_MODIFIED && etag.is_some() {
                self.refuses_fresh.store(false, Ordering::Relaxed);
                return Ok(Fetched::NotModified);
            }
            if status == StatusCode::UNAUTHORIZED {
                if refreshed {
                    self.refuses_fresh.store(true, Ordering::Relaxed);
                    return Err(ProviderError::Unauthorized);
                }
                if self.refuses_fresh.load(Ordering::Relaxed) {
                    tracing::debug!(
                        "the backend still refuses the model catalog to a fresh token; no refresh"
                    );
                    return Err(ProviderError::Unauthorized);
                }
                tracing::warn!(
                    "the backend refused the access token for the model catalog; refreshing it once"
                );
                drop(response);
                // NOTE: a model call may have refreshed the token since this fetch read
                // it; the new token was never refused, so it must not be forgotten.
                let current = self.tokens.access_token().await?;
                if current.secret().expose_secret() == token.secret().expose_secret() {
                    self.tokens.invalidate().await;
                }
                refreshed = true;
                continue;
            }
            if !status.is_success() {
                let reason = status.canonical_reason().unwrap_or("no reason");
                return Err(ProviderError::api(
                    Some(status.as_u16()),
                    None,
                    format!("the model catalog request failed: {reason}"),
                ));
            }
            let tag = response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = response.bytes().await.map_err(transport)?;
            let body: Value =
                serde_json::from_slice(&body).map_err(|source| ProviderError::Decode { source })?;
            let Some((entries, broken)) = entries_of(&body) else {
                return Err(ProviderError::api(
                    Some(status.as_u16()),
                    None,
                    "the model catalog has no list of models".to_owned(),
                ));
            };
            self.refuses_fresh.store(false, Ordering::Relaxed);
            if broken > 0 {
                tracing::warn!(broken, "the model catalog has entries that efr cannot read");
            }
            return Ok(Fetched::Changed(Catalog::from_backend(
                self.config.backend(),
                self.config.base_url(),
                entries,
                tag,
                self.clock.now(),
            )));
        }
    }
}

fn transport(error: HttpError) -> ProviderError {
    ProviderError::Transport { source: Box::new(error) }
}

#[cfg(test)]
mod tests;
