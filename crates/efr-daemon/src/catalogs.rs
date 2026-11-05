//! The model catalogs of all providers, and which providers have a login: the one model
//! list that `models.list` shows and that each turn checks its model against.
//!
//! The list holds the models of the provider of new conversations (`[model] provider`)
//! first, with the default model marked; then the models of each other provider with
//! a login, in the order of [`PROVIDERS`](crate::providers::PROVIDERS), never marked as
//! the default. Without a login another provider's list cannot serve a turn, so it is
//! left out, and a model id that two providers list goes to the first one that can
//! serve it. Each model names its provider.
//!
//! The logins are read from the credential store once at start and then follow each
//! login and logout of efrd, so the list comes from memory and never waits.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use efr_config::Settings;
use efr_protocol::{CatalogStatus, ModelInfo};

use crate::catalog::Models;

/// The catalogs, each with its fetch, and the providers that have a login.
#[derive(Debug)]
pub(crate) struct Catalogs {
    /// The catalog of the provider of new conversations at start first.
    all: Vec<Arc<Models>>,
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
        Catalogs { all, first, logins: Mutex::new(BTreeSet::new()) }
    }

    /// Every catalog, each with its own fetch.
    pub(crate) fn all(&self) -> &[Arc<Models>] {
        &self.all
    }

    /// The catalog of the provider of new conversations at start.
    pub(crate) fn first(&self) -> &Arc<Models> {
        &self.first
    }

    /// The catalog of `provider`, if efrd keeps one.
    pub(crate) fn of(&self, provider: &str) -> Option<&Arc<Models>> {
        self.all.iter().find(|models| models.provider() == provider)
    }

    /// The catalog of the provider of new conversations, `[model] provider` of
    /// `settings`. efrd does not start without it, so only a test that gives one
    /// catalog of another provider gets the first one.
    pub(crate) fn configured(&self, settings: &Settings) -> &Arc<Models> {
        self.of(&settings.model.provider).unwrap_or(&self.first)
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
        for other in &self.all {
            if Arc::ptr_eq(other, configured) || !self.logged_in(other.provider()) {
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
}

#[cfg(test)]
mod tests;
