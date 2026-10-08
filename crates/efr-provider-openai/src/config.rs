//! Which backend a provider talks to, and how.

use efr_http::{HeaderValue, HttpRequest, RetryPolicy};
use efr_provider::ModelInfo;

use crate::OpenAiError;

/// The ChatGPT subscription backend that Codex uses
/// (`codex-rs/model-provider-info/src/lib.rs`, `CHATGPT_CODEX_BASE_URL`; goose
/// `chatgpt_codex.rs`, `CODEX_API_ENDPOINT`).
pub const SUBSCRIPTION_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// The public API, reached with an API key.
pub const API_BASE_URL: &str = "https://api.openai.com/v1";

/// The `originator` header the subscription path sends unless the config names
/// another. Codex sends `codex_cli_rs`, opencode `opencode`; which values the backend
/// accepts from a client other than Codex is an open question of the structure
/// document, so the value is configurable.
pub const DEFAULT_ORIGINATOR: &str = "efr";

/// The two ways of reaching OpenAI's models.
///
/// They differ in the base URL and the headers, and the subscription backend refuses
/// a few request fields that the API accepts. Their `provider_raw` items are not
/// interchangeable: encrypted reasoning is bound to the account that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Backend {
    /// The ChatGPT subscription backend, with a token from the subscription login. It
    /// takes the `chatgpt-account-id` and `originator` headers and no
    /// `max_output_tokens` (Codex never sends one, and opencode removes it to match).
    Subscription,
    /// The public API, with an API key.
    Api,
}

/// Whether a request asks the model to reason.
///
/// A reasoning request carries the `reasoning` parameter and asks for
/// `reasoning.encrypted_content`, which a model outside the reasoning families refuses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReasoningMode {
    /// Decide by the model id: the `gpt-5`, `gpt-6`, `o1`, `o3`, `o4` and `codex-`
    /// families reason, their `-chat` variants and every other model do not.
    #[default]
    ByModel,
    /// Every request reasons.
    Always,
    /// No request reasons.
    Never,
}

/// The settings of one [`OpenAiProvider`](crate::OpenAiProvider), built by the daemon
/// from its config file.
///
/// Start from [`OpenAiConfig::subscription`] or [`OpenAiConfig::api`] and adjust it
/// with the `with_` methods. Settings that a request's `provider_options` can also set
/// (reasoning effort and summary, parallel tool calls) are defaults the request
/// overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    backend: Backend,
    base_url: String,
    originator: String,
    models: Vec<ModelInfo>,
    retry: RetryPolicy,
    reasoning: ReasoningMode,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
    parallel_tool_calls: bool,
}

impl OpenAiConfig {
    /// The ChatGPT subscription backend at [`SUBSCRIPTION_BASE_URL`], with the
    /// [`DEFAULT_ORIGINATOR`], no models of its own (the
    /// [catalog](crate::ModelCatalog) lists them), the default retry policy, the
    /// backend's default reasoning effort, `auto` reasoning summaries and parallel tool
    /// calls on.
    pub fn subscription() -> Self {
        OpenAiConfig::with_backend(Backend::Subscription, SUBSCRIPTION_BASE_URL)
    }

    /// The public API at [`API_BASE_URL`], with the defaults of
    /// [`OpenAiConfig::subscription`].
    pub fn api() -> Self {
        OpenAiConfig::with_backend(Backend::Api, API_BASE_URL)
    }

    fn with_backend(backend: Backend, base_url: &str) -> Self {
        OpenAiConfig {
            backend,
            base_url: base_url.to_owned(),
            originator: DEFAULT_ORIGINATOR.to_owned(),
            models: Vec::new(),
            retry: RetryPolicy::default(),
            reasoning: ReasoningMode::ByModel,
            reasoning_effort: None,
            reasoning_summary: Some("auto".to_owned()),
            parallel_tool_calls: true,
        }
    }

    /// The same config with requests going to `base_url`, such as a test server or a
    /// proxy. Requests go to `<base_url>/responses`; a trailing `/` is ignored. Fails
    /// unless the result is an absolute `http` or `https` URL.
    pub fn with_base_url(mut self, base_url: &str) -> Result<Self, OpenAiError> {
        let base_url = base_url.trim_end_matches('/');
        HttpRequest::post(&responses_url(base_url))
            .map_err(|source| OpenAiError::InvalidBaseUrl { source })?;
        base_url.clone_into(&mut self.base_url);
        Ok(self)
    }

    /// The same config with `originator` as the value of the `originator` header on the
    /// subscription path. Fails when the value cannot be a header value.
    pub fn with_originator(mut self, originator: &str) -> Result<Self, OpenAiError> {
        if HeaderValue::from_str(originator).is_err() {
            return Err(OpenAiError::InvalidOriginator { originator: originator.to_owned() });
        }
        originator.clone_into(&mut self.originator);
        Ok(self)
    }

    /// The same config with `models` laid over the catalog: a model of the catalog
    /// takes the window and output limit that its entry here gives, and a model that
    /// the catalog does not list is added. The daemon puts the models of
    /// `[openai] models` here.
    #[must_use]
    pub fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = models;
        self
    }

    /// The same config retrying with `retry`. Whatever the policy, a request is sent
    /// again only when the server certainly did not act on it (see
    /// `efr_http::RetryPolicy`), so a model call never runs twice.
    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The same config deciding with `reasoning` whether requests reason.
    #[must_use]
    pub fn with_reasoning(mut self, reasoning: ReasoningMode) -> Self {
        self.reasoning = reasoning;
        self
    }

    /// The same config asking for `effort` (such as `low` or `high`) as the default
    /// reasoning effort, or for the backend's default with `None`.
    #[must_use]
    pub fn with_reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    /// The same config asking for `summary` (such as `auto`, `concise` or `detailed`)
    /// as the default reasoning summary, or for none with `None`.
    #[must_use]
    pub fn with_reasoning_summary(mut self, summary: Option<String>) -> Self {
        self.reasoning_summary = summary;
        self
    }

    /// The same config allowing, or not, several tool calls in one answer by default.
    /// The conversation runs the calls of one answer one after another either way.
    #[must_use]
    pub fn with_parallel_tool_calls(mut self, parallel: bool) -> Self {
        self.parallel_tool_calls = parallel;
        self
    }

    /// The backend.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// The base URL, without a trailing `/`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The URL requests are posted to.
    pub fn responses_url(&self) -> String {
        responses_url(&self.base_url)
    }

    /// The `originator` header value of the subscription path.
    pub fn originator(&self) -> &str {
        &self.originator
    }

    /// The models that the config lays over the catalog.
    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    /// The retry policy.
    pub fn retry(&self) -> &RetryPolicy {
        &self.retry
    }

    /// How requests decide whether to reason.
    pub fn reasoning(&self) -> ReasoningMode {
        self.reasoning
    }

    /// The default reasoning effort, when one is set.
    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort.as_deref()
    }

    /// The default reasoning summary, when one is asked for.
    pub fn reasoning_summary(&self) -> Option<&str> {
        self.reasoning_summary.as_deref()
    }

    /// Whether one answer may hold several tool calls by default.
    pub fn parallel_tool_calls(&self) -> bool {
        self.parallel_tool_calls
    }
}

fn responses_url(base_url: &str) -> String {
    format!("{base_url}/responses")
}

#[cfg(test)]
mod tests;
