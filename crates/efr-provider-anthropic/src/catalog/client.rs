//! The fetch of the model catalog and the check of an API key.
//!
//! Both are `GET <base_url>/models` with `Authorization: Bearer <key>`,
//! `anthropic-version` and, when the config names one, `anthropic-workspace-id`. Such
//! a request runs no model. The fetch asks for `limit=1000` and follows `after_id`
//! while `has_more` is true; the check asks for `limit=1`. A 401 is
//! `ProviderError::Unauthorized` with the server's message at once, because a key
//! cannot refresh.

use std::sync::Arc;

use efr_http::HttpClient;
use efr_provider::{ProviderError, SecretString, TokenSource};
use efr_stdx::time::Clock;

use crate::failure::not_built;
use crate::{AnthropicConfig, Catalog};

/// Fetches the catalog of the API that an [`AnthropicConfig`] names.
#[derive(Debug, Clone)]
pub struct CatalogClient {
    config: AnthropicConfig,
    #[expect(dead_code, reason = "the fetch is not built yet")]
    http: HttpClient,
    #[expect(dead_code, reason = "the fetch is not built yet")]
    tokens: Arc<dyn TokenSource>,
    #[expect(dead_code, reason = "the fetch is not built yet")]
    clock: Arc<dyn Clock>,
}

impl CatalogClient {
    /// A client for the API of `config`, sending through `http` with the key from
    /// `tokens`. `clock` dates each list.
    pub fn new(
        config: AnthropicConfig,
        http: HttpClient,
        tokens: Arc<dyn TokenSource>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        CatalogClient { config, http, tokens, clock }
    }

    /// The base URL of the API, which the cache file records.
    pub fn base_url(&self) -> &str {
        self.config.base_url()
    }

    /// Asks the API for every page of its model list and returns it as one catalog from
    /// the backend, the models that are not `active` left out.
    pub async fn fetch(&self) -> Result<Catalog, ProviderError> {
        Err(not_built())
    }
}

/// Checks that the API takes `key`, with `GET <base_url>/models?limit=1` and the
/// headers of `config`, before a login stores the key. `Ok` for a 200; a 401 is
/// `ProviderError::Unauthorized` and a 403 an `Api` error, each with the server's
/// message (such as the one that asks for `anthropic-workspace-id`); any other status
/// is an `Api` error with the status, and no answer is a `Transport` error. The key
/// never reaches an error, a log line or a recorded request.
pub async fn check_key(
    http: &HttpClient,
    config: &AnthropicConfig,
    key: &SecretString,
) -> Result<(), ProviderError> {
    // NOTE: the request is not built yet.
    let _ = (http, config, key);
    Err(not_built())
}
