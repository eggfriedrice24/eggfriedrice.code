//! Which provider answers a turn: the daemon keeps one provider per id, and each turn
//! takes the one of its model.
//!
//! The model list of [`ConversationConfig::models`] names the provider of each model, in
//! the order of routing that the daemon gives it: the providers with a login first, the
//! provider of new conversations first among them. A turn takes the first provider that
//! lists its model; a model that no list names goes to the default provider
//! ([`ProviderRouter::default_provider`]), and the backend's answer decides. The turn keeps
//! its provider to its end, for each model call, a compaction, the key of its raw
//! items and its edit tool. A manual compaction and the context view pick theirs the
//! same way.

use std::fmt;
use std::sync::Arc;

use efr_provider::Provider;

use crate::{ConversationConfig, ConversationError};

/// The providers of the daemon, by id, such as `anthropic-api`.
pub trait ProviderRouter: Send + Sync + fmt::Debug {
    /// The provider of a model that no model list names: `[model] provider`. The
    /// daemon always runs it.
    fn default_provider(&self) -> Arc<dyn Provider>;

    /// The provider `id`, or `None` when the daemon runs no such provider.
    fn get(&self, id: &str) -> Option<Arc<dyn Provider>>;
}

/// One provider that serves every id: a daemon with one provider, such as one that a
/// test gives its own model.
#[derive(Debug, Clone)]
pub struct OneProvider(Arc<dyn Provider>);

impl OneProvider {
    /// A router that sends every turn to `provider`.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        OneProvider(provider)
    }
}

impl ProviderRouter for OneProvider {
    fn default_provider(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.0)
    }

    fn get(&self, _id: &str) -> Option<Arc<dyn Provider>> {
        Some(Arc::clone(&self.0))
    }
}

/// The id of the provider of `model`: the first provider whose list in `config` names
/// it, else the default of `router`.
pub(crate) fn provider_id(
    model: &str,
    config: &ConversationConfig,
    router: &dyn ProviderRouter,
) -> String {
    config
        .models
        .iter()
        .filter(|info| info.id == model)
        .find_map(|info| info.provider.clone())
        .unwrap_or_else(|| router.default_provider().id().as_str().to_owned())
}

/// The provider of `model`, as [`provider_id`] picks it, or the error of a provider
/// that the daemon does not run.
pub(crate) fn provider_of(
    model: &str,
    config: &ConversationConfig,
    router: &dyn ProviderRouter,
) -> Result<Arc<dyn Provider>, ConversationError> {
    let id = provider_id(model, config, router);
    router.get(&id).ok_or(ConversationError::NoProvider { provider: id })
}

/// The provider of a manual compaction or of the context view on `model`. `newest` is
/// the provider of the newest turn when `model` is that turn's model: that provider
/// while it runs and its list still names the model, so the request shares the prefix
/// of the turn's last request; else the provider as [`provider_of`] picks it.
pub(crate) fn provider_after(
    model: &str,
    newest: Option<&str>,
    config: &ConversationConfig,
    router: &dyn ProviderRouter,
) -> Result<Arc<dyn Provider>, ConversationError> {
    let listed = |provider: &str| {
        let mut named = config
            .models
            .iter()
            .filter(|info| info.id == model)
            .filter_map(|info| info.provider.as_deref())
            .peekable();
        // NOTE: a list that names no provider for the model says nothing against it.
        named.peek().is_none() || named.any(|named| named == provider)
    };
    match newest.filter(|provider| listed(provider)).and_then(|provider| router.get(provider)) {
        Some(provider) => Ok(provider),
        None => provider_of(model, config, router),
    }
}

#[cfg(test)]
mod tests;
