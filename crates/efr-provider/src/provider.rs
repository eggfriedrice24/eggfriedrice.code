//! The provider boundary.

use std::fmt;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::{Completion, ProviderError, ProviderEvent, ProviderId, Request};

/// The stream a [`Provider`] answers with. It is boxed and `Send` so that the trait
/// stays dyn-compatible and the conversation can poll it from any task.
pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<ProviderEvent, ProviderError>> + Send>>;

/// A way of reaching a model: the OpenAI subscription, an OpenAI API key, later
/// Anthropic, and the replay provider in tests.
///
/// [`stream`](Provider::stream) is the one model call a provider implements;
/// [`complete`](Provider::complete) collects it for callers that want the whole answer,
/// the shape goose uses. The trait is dyn-compatible (through `async-trait`) because
/// the daemon composes providers from credential rows at run time and hands the
/// conversation an `Arc<dyn Provider>`.
///
/// A provider takes its credentials from a [`TokenSource`](crate::TokenSource) and
/// never learns how a token was obtained. On a 401 it calls
/// [`TokenSource::invalidate`](crate::TokenSource::invalidate), retries once, and on a
/// second 401 fails with [`ProviderError::Unauthorized`].
#[async_trait]
pub trait Provider: Send + Sync + fmt::Debug {
    /// The provider's configured name, for logs, the event log and deciding whether a
    /// message's `provider_raw` belongs to it.
    fn id(&self) -> &ProviderId;

    /// The models the provider knows it can serve. Empty when it does not say, in which
    /// case any model id is passed through and the provider's answer decides.
    fn models(&self) -> &[ModelInfo] {
        &[]
    }

    /// Sends `request` and returns the answer as it streams. An error here means the
    /// request failed before any event; later failures arrive as error items.
    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError>;

    /// Sends `request` and collects the whole answer with [`Completion::collect`].
    async fn complete(&self, request: Request) -> Result<Completion, ProviderError> {
        Completion::collect(self.stream(request).await?).await
    }
}

/// What a provider says about one model it serves.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelInfo {
    /// The model id a [`Request`] names.
    pub id: String,
    /// The context window in tokens, when known.
    pub context_window: Option<u64>,
    /// The output token limit, when known.
    pub max_output_tokens: Option<u32>,
}

impl ModelInfo {
    /// A model with no known limits.
    pub fn new(id: impl Into<String>) -> Self {
        ModelInfo { id: id.into(), context_window: None, max_output_tokens: None }
    }

    /// The same model with a known context window.
    #[must_use]
    pub fn with_context_window(mut self, tokens: u64) -> Self {
        self.context_window = Some(tokens);
        self
    }

    /// The same model with a known output token limit.
    #[must_use]
    pub fn with_max_output_tokens(mut self, tokens: u32) -> Self {
        self.max_output_tokens = Some(tokens);
        self
    }
}

#[cfg(test)]
mod tests;
