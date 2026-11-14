//! The conversations' router: one running provider per id, the provider of new
//! conversations from the latest settings, and the switch offer of a turn whose
//! account is out.

use std::sync::{Arc, PoisonError, RwLock};

use efr_config::Settings;
use efr_conversation::ProviderRouter;
use efr_protocol::{AccountOut, LimitReached};
use efr_provider::Provider;
use tokio::sync::watch;

use crate::catalogs::Catalogs;

/// The providers that efrd runs besides the first, each under its id, shared between
/// `Providers` and the router, so an account that a login adds runs at once.
pub(crate) type SharedRoutes = Arc<RwLock<Vec<(String, Arc<dyn Provider>)>>>;

/// The providers that efrd runs, by id, with the latest settings for the default.
#[derive(Debug)]
pub(crate) struct Routes {
    /// The provider of `[model] provider` at start, the default when the settings name
    /// a provider that efrd does not run.
    first: (String, Arc<dyn Provider>),
    /// The other providers, shared with `Providers`, so an account that a login adds
    /// runs at once.
    others: SharedRoutes,
    settings: watch::Receiver<Arc<Settings>>,
    /// The model lists, the logins and the limits of the accounts, for the offer of a
    /// turn whose account is out.
    catalogs: Arc<Catalogs>,
}

impl Routes {
    /// The providers `first`, the one of `[model] provider` at start, and `others`, each
    /// under its id, with the default from `settings` and the accounts of `catalogs`.
    pub(crate) fn new(
        first: (String, Arc<dyn Provider>),
        others: SharedRoutes,
        settings: watch::Receiver<Arc<Settings>>,
        catalogs: Arc<Catalogs>,
    ) -> Self {
        Routes { first, others, settings, catalogs }
    }
}

impl ProviderRouter for Routes {
    fn default_provider(&self) -> Arc<dyn Provider> {
        // NOTE: the id is cloned out so the watch's read lock is held only here.
        let configured = self.settings.borrow().model.provider.clone();
        self.get(&configured).unwrap_or_else(|| Arc::clone(&self.first.1))
    }

    fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        if self.first.0 == id {
            return Some(Arc::clone(&self.first.1));
        }
        let others = self.others.read().unwrap_or_else(PoisonError::into_inner);
        others.iter().find(|(route, _)| route == id).map(|(_, provider)| Arc::clone(provider))
    }

    fn account_out(&self, provider: &str, model: &str, reached: LimitReached) -> AccountOut {
        // NOTE: the settings are cloned out so the watch's read lock is not held while
        // the lists are read.
        let settings = Arc::clone(&self.settings.borrow());
        self.catalogs.account_out(&settings, provider, model, reached)
    }
}
