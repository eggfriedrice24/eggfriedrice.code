//! The refusals of stored API keys, for `admin.status`: when a provider last answered
//! a request with the stored key with a 401.
//!
//! Two requests report: a model call of the provider of new conversations, through
//! [`Watched`], and a fetch of its model list (`catalog.rs`). A refusal is kept until
//! a request with the same key works again, or until a login or a logout replaces the
//! key ([`KeyRefusals::reset`]). Each reset starts a new generation, and a request
//! reports only into the generation that it started in: an answer to the old key that
//! comes after a new login does not mark the new key.
//!
//! A refusal holds a time and nothing of the key.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use efr_provider::{
    EditTool, ModelInfo, Provider, ProviderError, ProviderId, ProviderStream, Request,
};
use efr_stdx::time::Clock;
use futures::StreamExt as _;
use jiff::Timestamp;

/// The refusals of the stored keys, by provider id.
#[derive(Debug)]
pub(crate) struct KeyRefusals {
    clock: Arc<dyn Clock>,
    keys: Mutex<HashMap<String, KeyState>>,
}

/// What efrd knows of the stored key of one provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct KeyState {
    /// Counts the logins and logouts of the provider.
    generation: u64,
    /// When the provider last refused the key; `None` while it works.
    refused_at: Option<Timestamp>,
}

/// One request with the stored key of a provider, which reports how it went.
#[derive(Debug, Clone)]
pub(crate) struct KeyUse {
    refusals: Arc<KeyRefusals>,
    provider: String,
    generation: u64,
}

impl KeyRefusals {
    /// No refusal yet; the times come from `clock`.
    pub(crate) fn new(clock: Arc<dyn Clock>) -> Self {
        KeyRefusals { clock, keys: Mutex::default() }
    }

    /// A request that starts now with the stored key of `provider`.
    pub(crate) fn start(self: &Arc<Self>, provider: &str) -> KeyUse {
        let generation = self.lock().get(provider).map_or(0, |state| state.generation);
        KeyUse { refusals: Arc::clone(self), provider: provider.to_owned(), generation }
    }

    /// Forgets the refusal of `provider`, whose key a login or a logout replaced, and
    /// ignores the reports of requests that started before.
    pub(crate) fn reset(&self, provider: &str) {
        let mut keys = self.lock();
        let state = keys.entry(provider.to_owned()).or_default();
        state.generation = state.generation.wrapping_add(1);
        state.refused_at = None;
    }

    /// When `provider` last refused its stored key, while it still does.
    pub(crate) fn refused_at(&self, provider: &str) -> Option<Timestamp> {
        self.lock().get(provider).and_then(|state| state.refused_at)
    }

    /// Records the end of `used`: refused at the clock's time, or working.
    fn report(&self, used: &KeyUse, refused: bool) {
        let now = refused.then(|| self.clock.now());
        let mut keys = self.lock();
        let state = keys.entry(used.provider.clone()).or_default();
        if state.generation != used.generation {
            return;
        }
        if refused && state.refused_at.is_none() {
            tracing::warn!(provider = %used.provider, "the provider refused the stored API key");
        }
        state.refused_at = now;
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, KeyState>> {
        // NOTE: every change replaces whole values, so a poisoned map is still usable.
        self.keys.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl KeyUse {
    /// The provider took the key.
    pub(crate) fn worked(&self) {
        self.refusals.report(self, false);
    }

    /// The request failed with `error`: a refusal of the key for
    /// [`ProviderError::Unauthorized`]; any other error says nothing about the key.
    pub(crate) fn failed(&self, error: &ProviderError) {
        if matches!(error, ProviderError::Unauthorized { .. }) {
            self.refusals.report(self, true);
        }
    }
}

/// The provider of new conversations when it uses a stored API key: every model call
/// reports to [`KeyRefusals`] whether the provider took the key. The answer itself is
/// passed on as it comes.
#[derive(Debug)]
pub(crate) struct Watched {
    inner: Arc<dyn Provider>,
    /// The provider id of the credential, such as `anthropic-api`.
    provider: String,
    refusals: Arc<KeyRefusals>,
}

impl Watched {
    /// `inner`, whose key is the credential of `provider`, reporting to `refusals`.
    pub(crate) fn new(
        inner: Arc<dyn Provider>,
        provider: &str,
        refusals: Arc<KeyRefusals>,
    ) -> Self {
        Watched { inner, provider: provider.to_owned(), refusals }
    }
}

#[async_trait]
impl Provider for Watched {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.inner.models()
    }

    fn default_edit_tool(&self) -> EditTool {
        self.inner.default_edit_tool()
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let used = self.refusals.start(&self.provider);
        let stream = self.inner.stream(request).await.inspect_err(|error| used.failed(error))?;
        // NOTE: a stream can still fail with a 401 before its first event, such as a
        // WebSocket call whose error status comes as an item, so the key counts as
        // working only from its first event on.
        let mut first = true;
        Ok(Box::pin(stream.inspect(move |item| match item {
            Ok(_) if first => {
                first = false;
                used.worked();
            }
            Ok(_) => {}
            Err(error) => used.failed(error),
        })))
    }
}

#[cfg(test)]
mod tests;
