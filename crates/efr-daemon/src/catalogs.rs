//! The model catalogs of all providers, and which providers have a login: the one model
//! list that `models.list` shows and that each turn checks its model against.
//!
//! The list holds the models of the provider of new conversations (`[model] provider`)
//! first, with the default model marked; then the models of each other provider with
//! a login, in the order of [`PROVIDERS`](crate::providers::PROVIDERS), never marked as
//! the default. Without a login another provider's list cannot serve a turn, so it is
//! left out. Each model names its provider.
//!
//! The turns read the same list in the order of routing ([`Catalogs::routed`]): the
//! provider of new conversations moves behind the others while it has no login, so a
//! model id that two providers list, such as a GPT model of the ChatGPT plan and of an
//! API key, goes to the first one that can serve it.
//!
//! The logins are read from the credential store once at start and then follow each
//! login and logout of efrd, so the list comes from memory and never waits.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use efr_config::Settings;
use efr_protocol::{CatalogStatus, ModelInfo};

use crate::catalog::Models;

/// The catalogs, each with its fetch, and the providers that have a login.
#[derive(Debug)]
pub(crate) struct Catalogs {
    /// The catalog of the provider of new conversations at start first; an account
    /// that a login adds while efrd runs comes last.
    all: RwLock<Vec<Arc<Models>>>,
    /// The first of `all`.
    first: Arc<Models>,
    logins: Mutex<BTreeSet<String>>,
}

/// The model list of the latest settings, with the status of each catalog in it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Listed {
    pub(crate) models: Vec<ModelInfo>,
    /// The catalog of the provider of new conversations first.
    pub(crate) catalogs: Vec<CatalogStatus>,
}

impl Catalogs {
    /// The catalog `first` of the provider of new conversations and the catalogs
    /// `others`, none of them with a login yet.
    pub(crate) fn new(first: Arc<Models>, others: Vec<Arc<Models>>) -> Self {
        let all = std::iter::once(Arc::clone(&first)).chain(others).collect();
        Catalogs { all: RwLock::new(all), first, logins: Mutex::new(BTreeSet::new()) }
    }

    /// Every catalog, each with its own fetch.
    pub(crate) fn all(&self) -> Vec<Arc<Models>> {
        self.all.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Adds the catalog of an account that a login added while efrd runs.
    pub(crate) fn add(&self, models: Arc<Models>) {
        self.all.write().unwrap_or_else(PoisonError::into_inner).push(models);
    }

    /// The catalog of the provider of new conversations at start.
    pub(crate) fn first(&self) -> Arc<Models> {
        Arc::clone(&self.first)
    }

    /// The catalog of `provider`, if efrd keeps one.
    pub(crate) fn of(&self, provider: &str) -> Option<Arc<Models>> {
        self.all
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|models| models.provider() == provider)
            .cloned()
    }

    /// The catalog of the provider of new conversations, `[model] provider` of
    /// `settings`. efrd does not start without it, so only a test that gives one
    /// catalog of another provider gets the first one.
    pub(crate) fn configured(&self, settings: &Settings) -> Arc<Models> {
        self.of(&settings.model.provider).unwrap_or_else(|| self.first())
    }

    /// Records whether `provider` has a login now.
    pub(crate) fn set_logged_in(&self, provider: &str, logged_in: bool) {
        let mut logins = self.logins.lock().unwrap_or_else(PoisonError::into_inner);
        if logged_in {
            logins.insert(provider.to_owned());
        } else {
            logins.remove(provider);
        }
    }

    /// True when `provider` has a login.
    pub(crate) fn logged_in(&self, provider: &str) -> bool {
        self.logins.lock().unwrap_or_else(PoisonError::into_inner).contains(provider)
    }

    /// The model list of `settings`: the configured provider's models first, then
    /// those of each other provider with a login.
    pub(crate) fn listed(&self, settings: &Settings) -> Listed {
        let configured = self.configured(settings);
        let mut models = configured.effective(settings);
        let mut catalogs = vec![configured.status()];
        for other in &self.all() {
            if Arc::ptr_eq(other, &configured) || !self.logged_in(other.provider()) {
                continue;
            }
            let mut listed = other.effective(settings);
            for model in &mut listed {
                model.default = false;
            }
            models.extend(listed);
            catalogs.push(other.status());
        }
        Listed { models, catalogs }
    }

    /// The list of [`listed`](Self::listed) in the order of routing: the models of the
    /// providers with a login first, each group in its own order. A turn goes to the
    /// first provider that lists its model.
    pub(crate) fn routed(&self, settings: &Settings) -> Vec<ModelInfo> {
        let (mut first, rest): (Vec<ModelInfo>, Vec<ModelInfo>) =
            self.listed(settings).models.into_iter().partition(|model| {
                model.provider.as_deref().is_some_and(|provider| self.logged_in(provider))
            });
        first.extend(rest);
        first
    }
}

#[cfg(test)]
mod tests;
