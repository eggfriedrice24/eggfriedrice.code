//! The model catalog: the Claude models that the API offers, with their windows, their
//! output limits and their efforts.
//!
//! The API lists its models at `GET <base_url>/models` (`limit` up to 1000, more
//! pages through `after_id` while `has_more` is true). Each entry has an `id`,
//! `max_input_tokens`, `max_tokens`, `capabilities` (with the effort levels) and a
//! `lifecycle`. A model is offered only when its `lifecycle` is `active`; the API's
//! default filter also returns `deprecated` models. An entry that cannot be read is
//! left out, and the others stay.
//!
//! An offered model becomes an `efr_provider::ModelInfo` with:
//!
//! - `context_window` and `max_context_window` from `max_input_tokens` (an entry of
//!   `[anthropic] models` can lower the window);
//! - `max_output_tokens` from `max_tokens`;
//! - `efforts` from `capabilities`, and [`DEFAULT_EFFORT`](crate::DEFAULT_EFFORT) as
//!   `default_effort` when the model takes it;
//! - `edit_tool` `EditTool::Replace`, and neither freeform tools nor WebSockets.
//!
//! efr keeps no table of Claude models: there is no list until a fetch or the cache
//! file gives one, and no guessed window. A [`ModelCatalog`] holds the current list in
//! memory, and a request never waits for a fetch. The daemon fetches at start, after a
//! login and every hour, and before the first turn when it has no list at all.

mod cache;
mod client;

use std::sync::{Arc, PoisonError, RwLock};

use efr_provider::ModelInfo;
use jiff::Timestamp;

pub use self::cache::{read_cache, write_cache};
pub use self::client::{CatalogClient, check_key};
use crate::DEFAULT_MODEL;

/// Where a [`Catalog`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CatalogOrigin {
    /// The API answered this daemon with it.
    Backend,
    /// The cache file of an earlier fetch, read at start.
    Cache,
}

/// One list of models and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    models: Vec<ModelInfo>,
    listed: usize,
    origin: CatalogOrigin,
    fetched_at: Timestamp,
}

impl Catalog {
    /// Where the catalog came from.
    pub fn origin(&self) -> CatalogOrigin {
        self.origin
    }

    /// When the API sent the list.
    pub fn fetched_at(&self) -> Timestamp {
        self.fetched_at
    }

    /// The models on offer, in the API's order (the newest first).
    pub fn models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    /// The model of a turn that names none: [`DEFAULT_MODEL`] when it is on offer, else
    /// the first model on offer.
    pub fn default_model(&self) -> Option<String> {
        if self.models.iter().any(|model| model.id == DEFAULT_MODEL) {
            Some(DEFAULT_MODEL.to_owned())
        } else {
            self.models.first().map(|model| model.id.clone())
        }
    }

    /// How many models of the list are not on offer: not `active`, or not readable.
    pub fn left_out(&self) -> usize {
        self.listed.saturating_sub(self.models.len())
    }
}

/// The current catalog, shared by the provider, which reads it for each request, and
/// the task that fetches a new one.
///
/// It starts without a list. A reader takes the current catalog from memory and keeps
/// it for its unit of work; a new catalog replaces it for the readers after that.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    current: Arc<RwLock<Option<Arc<Catalog>>>>,
}

/// What [`ModelCatalog::apply`] did with a list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Applied {
    /// The new list is current now; it belongs in the cache.
    Changed(Arc<Catalog>),
    /// The list offers no model that efr can use, so the current catalog stays.
    /// `listed` is how many models the list held.
    Refused {
        /// How many models the API listed.
        listed: usize,
    },
}

impl ModelCatalog {
    /// A shared catalog without a list.
    pub fn new() -> Self {
        ModelCatalog::default()
    }

    /// A shared catalog that starts as `catalog`, such as the list of the cache file.
    pub fn with_catalog(catalog: Catalog) -> Self {
        ModelCatalog { current: Arc::new(RwLock::new(Some(Arc::new(catalog)))) }
    }

    /// The current catalog; `None` until a fetch or the cache file gave one.
    pub fn current(&self) -> Option<Arc<Catalog>> {
        // NOTE: a panic while the lock was held cannot leave a half-written catalog,
        // because the value is one `Arc` that is swapped whole.
        self.current.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Makes `catalog` current, unless it offers no model.
    pub fn apply(&self, catalog: Catalog) -> Applied {
        if catalog.models.is_empty() {
            return Applied::Refused { listed: catalog.listed };
        }
        let catalog = Arc::new(catalog);
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&catalog));
        Applied::Changed(catalog)
    }
}

#[cfg(test)]
mod tests;
