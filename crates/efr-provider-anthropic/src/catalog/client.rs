//! The fetch of the model catalog and the check of an API key.
//!
//! Both are `GET <base_url>/models` with `Authorization: Bearer <key>`,
//! `anthropic-version` and, when the config names one, `anthropic-workspace-id`. Such
//! a request runs no model. The fetch asks for `limit=1000` and follows `after_id`
//! while `has_more` is true; the check asks for `limit=1`. A 401 is
//! `ProviderError::Unauthorized` with the server's message at once, because a key
//! cannot refresh.
//!
//! Each page of the fetch goes through the config's `efr_http::RetryPolicy` with the
//! classes of `failure`, as a model call does: a 429 that is not a spend cap, a 5xx
//! and a 529 are sent again, a spend cap and a refusal are not. Because the request is
//! a `GET`, a timeout or a broken connection is sent again too. The check goes once.

use std::sync::Arc;
use std::time::Duration;

use efr_http::{HeaderValue, HttpClient, HttpError, HttpRequest, Url, header};
use efr_provider::{ProviderError, SecretString, TokenSource};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use serde_json::Value;

use super::entries_of;
use crate::failure::{self, Answered, Failure, transport, without_key};
use crate::messages::sign;
use crate::{AnthropicConfig, Catalog};

/// How long one page of the fetch may take. The fetch runs in the background, so this
/// only bounds how long a hung API holds a connection.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the check of a key may take. A person waits for it.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// The most models that the API sends on one page.
const PAGE_LIMIT: &str = "1000";

/// The most pages that one fetch reads, so a list whose pages never end cannot hold
/// the fetch forever. A thousand models a page makes this far more than the API lists.
const MAX_PAGES: usize = 100;

/// Fetches the catalog of the API that an [`AnthropicConfig`] names.
#[derive(Debug, Clone)]
pub struct CatalogClient {
    config: AnthropicConfig,
    http: HttpClient,
    tokens: Arc<dyn TokenSource>,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
}

/// One page of the model list.
#[derive(Debug)]
struct Page {
    body: Value,
}

impl Page {
    /// The id to send as `after_id` for the next page; `None` after the last page.
    fn next(&self) -> Result<Option<&str>, ProviderError> {
        if self.body.get("has_more").and_then(Value::as_bool) != Some(true) {
            return Ok(None);
        }
        match self.body.get("last_id").and_then(Value::as_str) {
            Some(last) if !last.is_empty() => Ok(Some(last)),
            _ => Err(list_error("the model list has more pages but names no next one")),
        }
    }
}

impl CatalogClient {
    /// A client for the API of `config`, sending through `http` with the key from
    /// `tokens`. `clock` dates each list, and `clock` and `rng` time the retries.
    pub fn new(
        config: AnthropicConfig,
        http: HttpClient,
        tokens: Arc<dyn TokenSource>,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
    ) -> Self {
        CatalogClient { config, http, tokens, clock, rng }
    }

    /// The base URL of the API, which the cache file records.
    pub fn base_url(&self) -> &str {
        self.config.base_url()
    }

    /// Asks the API for every page of its model list and returns it as one catalog from
    /// the backend, the models that are not `active` left out. A 401 is
    /// `ProviderError::Unauthorized` with the server's message: at once for a key, and
    /// after one refresh for a token source that can refresh.
    pub async fn fetch(&self) -> Result<Catalog, ProviderError> {
        let mut entries = Vec::new();
        let mut listed = 0;
        let mut after: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page = self.page(after.as_deref()).await?;
            let Some(data) = page.body.get("data").filter(|data| data.is_array()) else {
                return Err(list_error("the model list has no data"));
            };
            let (read, broken) = entries_of(data);
            if broken > 0 {
                tracing::debug!(broken, "the model list has entries that efr cannot read");
            }
            listed += read.len() + broken;
            entries.extend(read);
            match page.next()? {
                Some(next) if after.as_deref() != Some(next) => after = Some(next.to_owned()),
                Some(_) => return Err(list_error("the model list names the same page again")),
                None => {
                    let now = self.clock.now();
                    return Ok(Catalog::from_backend(self.base_url(), entries, listed, now));
                }
            }
        }
        Err(list_error("the model list has more pages than efr reads"))
    }

    /// One page of the list, after the model `after` when it is given.
    async fn page(&self, after: Option<&str>) -> Result<Page, ProviderError> {
        let mut url = Url::parse(&self.config.models_url())
            .map_err(|source| transport(HttpError::InvalidUrl { source }))?;
        url.query_pairs_mut().append_pair("limit", PAGE_LIMIT);
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after_id", after);
        }
        let unsigned = list_request(&url).map_err(transport)?;
        let mut refreshed = false;
        loop {
            let token = self.tokens.access_token().await?;
            let request = sign(&unsigned, &self.config, token.secret()).map_err(transport)?;
            let result = self
                .config
                .retry()
                .run(&*self.clock, &*self.rng, |attempt| {
                    self.attempt(&request, token.secret(), attempt)
                })
                .await;
            match result {
                Ok(Answered(response)) => {
                    let body = response.bytes().await.map_err(transport)?;
                    let body = serde_json::from_slice(&body)
                        .map_err(|source| ProviderError::Decode { source })?;
                    return Ok(Page { body });
                }
                Err(Failure { error: ProviderError::Unauthorized { .. }, .. })
                    if self.tokens.refreshable() && !refreshed =>
                {
                    tracing::warn!(
                        "the API refused the token for the model list; refreshing it once"
                    );
                    self.tokens.invalidate().await;
                    refreshed = true;
                }
                Err(failure) => return Err(failure.error),
            }
        }
    }

    /// One attempt of a page: the response when its status is a success, else what its
    /// error body, without `key`, means.
    async fn attempt(
        &self,
        request: &HttpRequest,
        key: &SecretString,
        attempt: u32,
    ) -> Result<Answered, Failure> {
        let response = self.http.send(request).await.map_err(Failure::idempotent_transport)?;
        let status = response.status();
        if status.is_success() {
            return Ok(Answered(response));
        }
        let retry_after = efr_http::retry_after(response.headers(), self.clock.now());
        let body = response.text().await.map_err(Failure::idempotent_transport)?;
        let failure = failure::answer(status, retry_after, &without_key(&body, key), None);
        tracing::debug!(
            attempt,
            status = status.as_u16(),
            again = ?failure.again,
            "the API refused the fetch of the model list"
        );
        Err(failure)
    }
}

/// Checks that the API takes `key`, with `GET <base_url>/models?limit=1` and the
/// headers of `config`, before a login stores the key. `Ok` for a 200; a 401 is
/// `ProviderError::Unauthorized` and a 403 an `Api` error, each with the server's
/// message (such as the one that asks for `anthropic-workspace-id`); any other status
/// is an `Api` error with the status, and no answer is a `Transport` error. The check
/// goes once, as the OpenAI one does: a person waits for the answer and can ask again.
/// The key never reaches an error, a log line or a recorded request: a server message
/// that quotes it shows a placeholder.
pub async fn check_key(
    http: &HttpClient,
    config: &AnthropicConfig,
    key: &SecretString,
) -> Result<(), ProviderError> {
    let mut url = Url::parse(&config.models_url())
        .map_err(|source| transport(HttpError::InvalidUrl { source }))?;
    url.query_pairs_mut().append_pair("limit", "1");
    let request = list_request(&url)
        .map(|unsigned| unsigned.timeout(CHECK_TIMEOUT))
        .and_then(|unsigned| sign(&unsigned, config, key));
    let response = http.send(&request.map_err(transport)?).await.map_err(transport)?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.map_err(transport)?;
    Err(failure::refusal(status, &without_key(&body, key)))
}

/// A `GET` of the model list at `url`, before its credentials.
fn list_request(url: &Url) -> Result<HttpRequest, HttpError> {
    Ok(HttpRequest::get(url.as_str())?
        .header(header::ACCEPT, HeaderValue::from_static("application/json"))
        .timeout(FETCH_TIMEOUT))
}

fn list_error(message: &str) -> ProviderError {
    ProviderError::Api { status: None, code: None, message: message.to_owned() }
}

#[cfg(test)]
mod tests;
