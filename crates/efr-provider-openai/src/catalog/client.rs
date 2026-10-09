//! The fetch of the model catalog from the backend, and the check of an API key.
//!
//! The subscription: `GET <base_url>/models?client_version=<efr's version>` with the
//! same credentials and headers as a model request (Codex
//! `codex-rs/codex-api/src/endpoint/models.rs`, `ModelsClient`). The API key backend:
//! `GET <base_url>/models`, whose ids cut the built-in table down. A list that efr
//! already has goes with its tag in `If-None-Match`, and a 304 answer confirms it
//! without a body.
//!
//! [`check_key`] sends the API key backend's request once with a key that is not
//! stored yet, before a login stores it. The request runs no model.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use efr_http::{HeaderValue, HttpClient, HttpError, HttpRequest, StatusCode, Url, header};
use efr_provider::{AccessToken, ExposeSecret as _, ProviderError, SecretString, TokenSource};
use efr_stdx::time::Clock;
use serde_json::Value;

use super::{CLIENT_VERSION, Catalog, api_ids_of, entries_of};
use crate::OpenAiConfig;
use crate::config::Backend;
use crate::responses::{server_error, sign};

/// How long one fetch may take. The fetch runs in the background, so this only bounds
/// how long a hung backend holds a connection.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the check of a key may take. A person waits for it.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// What a server message shows in place of the key, when it quotes the key.
const KEY_PLACEHOLDER: &str = "<the key>";

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

    /// The URL of the catalog, with efr's own version as `client_version` on the
    /// subscription.
    pub fn url(&self) -> Result<Url, HttpError> {
        let mut url = Url::parse(&self.config.models_url())
            .map_err(|source| HttpError::InvalidUrl { source })?;
        if self.config.backend() == Backend::Subscription {
            url.query_pairs_mut().append_pair("client_version", CLIENT_VERSION);
        }
        Ok(url)
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
                if !self.tokens.refreshable() {
                    return Err(ProviderError::Unauthorized { message: None });
                }
                if refreshed {
                    self.refuses_fresh.store(true, Ordering::Relaxed);
                    return Err(ProviderError::Unauthorized { message: None });
                }
                if self.refuses_fresh.load(Ordering::Relaxed) {
                    tracing::debug!(
                        "the backend still refuses the model catalog to a fresh token; no refresh"
                    );
                    return Err(ProviderError::Unauthorized { message: None });
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
            if self.config.backend() == Backend::Api {
                let Some(ids) = api_ids_of(&body) else {
                    return Err(ProviderError::api(
                        Some(status.as_u16()),
                        None,
                        "the model list has no data member".to_owned(),
                    ));
                };
                self.refuses_fresh.store(false, Ordering::Relaxed);
                let base_url = self.config.base_url();
                return Ok(Fetched::Changed(Catalog::from_api(
                    base_url,
                    &ids,
                    tag,
                    self.clock.now(),
                )));
            }
            let Some((entries, broken)) = entries_of(&body) else {
                return Err(ProviderError::api(
                    Some(status.as_u16()),
                    None,
                    "the model catalog has no list of models".to_owned(),
                ));
            };
            self.refuses_fresh.store(false, Ordering::Relaxed);
            if broken > 0 {
                tracing::debug!(broken, "the model catalog has entries that efr cannot read");
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

/// Checks `key` with `GET <base_url>/models` and the organization and project headers
/// of `config`, the API key backend's, before a login stores the key. `Ok` for a 200;
/// a 401 is `ProviderError::Unauthorized` and any other status an `Api` error, each
/// with the server's message; no answer is a `Transport` error. The request runs no
/// model, and it is sent again only after an answer that says the server did not
/// handle it. The key never reaches an error or a log line: a server message that
/// quotes it shows a placeholder.
pub async fn check_key(
    http: &HttpClient,
    config: &OpenAiConfig,
    key: &SecretString,
) -> Result<(), ProviderError> {
    let unsigned = HttpRequest::get(&config.models_url())
        .map_err(transport)?
        .header(header::ACCEPT, HeaderValue::from_static("application/json"))
        .timeout(CHECK_TIMEOUT);
    let request = sign(&unsigned, config, &AccessToken::new(key.clone())).map_err(transport)?;
    let response = http.send_with_retry(&request, config.retry()).await.map_err(transport)?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    let (code, message) = server_error(&body);
    let message = message.map(|message| hide_key(&message, key));
    if status == StatusCode::UNAUTHORIZED {
        return Err(ProviderError::Unauthorized { message });
    }
    let message = message.unwrap_or_else(|| {
        let reason = status.canonical_reason().unwrap_or("no reason");
        format!("the key check failed: {} {reason}", status.as_u16())
    });
    // NOTE: built directly, not through `ProviderError::api`, which reads a 413 or an
    // overflow code as a full context window; a key check sends no context.
    Err(ProviderError::Api { status: Some(status.as_u16()), code, message })
}

/// `message` with every copy of `key` replaced by a placeholder.
fn hide_key(message: &str, key: &SecretString) -> String {
    let key = key.expose_secret();
    if key.is_empty() { message.to_owned() } else { message.replace(key, KEY_PLACEHOLDER) }
}

fn transport(error: HttpError) -> ProviderError {
    ProviderError::Transport { source: Box::new(error) }
}

#[cfg(test)]
mod tests;
