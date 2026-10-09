//! Where the Messages API is, and how efr talks to it.

use efr_http::{HeaderValue, HttpRequest, RetryPolicy};
use efr_provider::ModelInfo;

use crate::AnthropicError;

/// The Messages API, reached with an API key.
pub const API_BASE_URL: &str = "https://api.anthropic.com/v1";

/// The `anthropic-version` header of every request. The API refuses a request without
/// it.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// How long the prompt cache keeps the entries that a request writes.
///
/// An entry written for one hour costs twice the input price, an entry for five
/// minutes 1.25 times; a read costs a tenth or less. Every read refreshes an entry's
/// time for free. Where the markers go is the job of the request conversion
/// (`convert::breakpoints`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheTtl {
    /// One hour on the system prompt and the tools and on a moving anchor in the
    /// messages, five minutes on the tail of a tool loop. A call marks a new anchor at
    /// its tail when it is the first call of a turn, or when the five-minute part since
    /// the last anchor has grown past about 20,000 tokens, so a pause of any length
    /// loses at most that part. A side call, such as a compaction's summary request,
    /// never marks an anchor.
    #[default]
    Auto,
    /// Five minutes on every marker.
    FiveMinutes,
    /// One hour on every marker.
    OneHour,
}

impl CacheTtl {
    /// The setting's value in the config: `auto`, `5m` or `1h`.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            CacheTtl::Auto => "auto",
            CacheTtl::FiveMinutes => "5m",
            CacheTtl::OneHour => "1h",
        }
    }
}

/// The settings of one [`AnthropicProvider`](crate::AnthropicProvider), built by the
/// daemon from its config file.
///
/// Start from [`AnthropicConfig::new`] and adjust it with the `with_` methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicConfig {
    base_url: String,
    models: Vec<ModelInfo>,
    retry: RetryPolicy,
    cache_ttl: CacheTtl,
    workspace_id: Option<String>,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        AnthropicConfig::new()
    }
}

impl AnthropicConfig {
    /// The API at [`API_BASE_URL`], with no models of its own (the
    /// [catalog](crate::ModelCatalog) lists them), the default retry policy, the
    /// [`CacheTtl::Auto`] markers and no workspace id.
    pub fn new() -> Self {
        AnthropicConfig {
            base_url: API_BASE_URL.to_owned(),
            models: Vec::new(),
            retry: RetryPolicy::default(),
            cache_ttl: CacheTtl::Auto,
            workspace_id: None,
        }
    }

    /// The same config with requests going to `base_url`, such as a test server or a
    /// proxy. Requests go to `<base_url>/messages` and `<base_url>/models`; a trailing
    /// `/` is ignored. Fails unless the result is an absolute `http` or `https` URL.
    pub fn with_base_url(mut self, base_url: &str) -> Result<Self, AnthropicError> {
        let base_url = base_url.trim_end_matches('/');
        HttpRequest::post(&messages_url(base_url))
            .map_err(|source| AnthropicError::InvalidBaseUrl { source })?;
        base_url.clone_into(&mut self.base_url);
        Ok(self)
    }

    /// The same config with `models` laid over the catalog: a model of the catalog
    /// takes the window and output limit that its entry here gives, up to the limits of
    /// the catalog, and a model that the catalog does not list is added. The daemon puts
    /// the models of `[anthropic] models` here.
    #[must_use]
    pub fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }

    /// The same config retrying with `retry`. Whatever the policy, a model call is
    /// never sent again after its stream has started.
    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The same config placing its prompt cache markers with `ttl`.
    #[must_use]
    pub fn with_cache_ttl(mut self, ttl: CacheTtl) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// The same config sending `workspace_id` as the `anthropic-workspace-id` header
    /// of every request, which a key that is not scoped to one workspace needs. Fails
    /// when the value cannot be a header value.
    pub fn with_workspace_id(mut self, workspace_id: &str) -> Result<Self, AnthropicError> {
        if workspace_id.is_empty() || HeaderValue::from_str(workspace_id).is_err() {
            return Err(AnthropicError::InvalidWorkspaceId {
                workspace_id: workspace_id.to_owned(),
            });
        }
        self.workspace_id = Some(workspace_id.to_owned());
        Ok(self)
    }

    /// The base URL, without a trailing `/`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The URL that model calls are posted to.
    pub fn messages_url(&self) -> String {
        messages_url(&self.base_url)
    }

    /// The URL of the model list.
    pub fn models_url(&self) -> String {
        format!("{}/models", self.base_url)
    }

    /// The models that the config lays over the catalog.
    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    /// The retry policy.
    pub fn retry(&self) -> &RetryPolicy {
        &self.retry
    }

    /// How the requests place their prompt cache markers.
    pub fn cache_ttl(&self) -> CacheTtl {
        self.cache_ttl
    }

    /// The workspace id that every request names, when one is set.
    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }
}

fn messages_url(base_url: &str) -> String {
    format!("{base_url}/messages")
}

#[cfg(test)]
mod tests;
