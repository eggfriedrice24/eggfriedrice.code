//! The conversations' router: one running provider per id, and the provider of new
//! conversations from the latest settings.

use std::sync::Arc;

use efr_config::Settings;
use efr_conversation::ProviderRouter;
use efr_provider::Provider;
use tokio::sync::watch;

/// The providers that efrd runs, by id, with the latest settings for the default.
#[derive(Debug)]
pub(crate) struct Routes {
    /// The provider of `[model] provider` at start, the default when the settings name
    /// a provider that efrd does not run.
    first: (String, Arc<dyn Provider>),
    others: Vec<(String, Arc<dyn Provider>)>,
    settings: watch::Receiver<Arc<Settings>>,
}

impl Routes {
    /// The providers `first`, the one of `[model] provider` at start, and `others`, each
    /// under its id, with the default from `settings`.
    pub(crate) fn new(
        first: (String, Arc<dyn Provider>),
        others: Vec<(String, Arc<dyn Provider>)>,
        settings: watch::Receiver<Arc<Settings>>,
    ) -> Self {
        Routes { first, others, settings }
    }
}

impl ProviderRouter for Routes {
    fn default_provider(&self) -> Arc<dyn Provider> {
        // NOTE: the id is cloned out so the watch's read lock is held only here.
        let configured = self.settings.borrow().model.provider.clone();
        self.get(&configured).unwrap_or_else(|| Arc::clone(&self.first.1))
    }

    fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        std::iter::once(&self.first)
            .chain(&self.others)
            .find(|(route, _)| route == id)
            .map(|(_, provider)| Arc::clone(provider))
    }
}
