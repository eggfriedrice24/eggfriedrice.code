//! The model catalog: the Claude models that the API offers, with their windows, their
//! output limits and their efforts.
//!
//! The API lists its models at `GET <base_url>/models` (`limit` up to 1000, more
//! pages through `after_id` while `has_more` is true). Each entry has an `id`,
//! `max_input_tokens`, `max_tokens`, `capabilities` (with the effort levels) and a
//! `lifecycle`. A model is offered only when its `lifecycle` is `active`; the API's
//! default filter also returns `deprecated` models. An entry without a `lifecycle` is
//! taken as `active`. An entry that cannot be read is left out, and the others stay.
//!
//! An offered model becomes an `efr_provider::ModelInfo` with:
//!
//! - `context_window` and `max_context_window` from `max_input_tokens` (an entry of
//!   `[anthropic] models` can lower the window);
//! - `max_output_tokens` from `max_tokens`;
//! - `efforts` from `capabilities.effort`: each level whose `supported` is true, from
//!   `low` to `max` (a level that efr does not know comes after them, by name), and
//!   [`DEFAULT_EFFORT`](crate::DEFAULT_EFFORT) as `default_effort` when the model takes
//!   it;
//! - `edit_tool` `EditTool::Replace`, and neither freeform tools nor WebSockets.
//!
//! efr keeps no table of Claude models: there is no list until a fetch or the cache
//! file gives one, and no guessed window. A [`ModelCatalog`] holds the current list in
//! memory, and a request never waits for a fetch. The daemon fetches at start, after a
//! login and every hour, and before the first turn when it has no list at all.
//!
//! The provider lays the models of the config (`[anthropic] models`) over the catalog
//! ([`with_extra`]): an entry of the same id sets the window and the output limit, up
//! to the catalog's limits, and any other entry comes after the catalog's models.

mod cache;
mod client;

use std::sync::{Arc, PoisonError, RwLock};

use efr_provider::{EditTool, ModelInfo};
use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub use self::cache::{read_cache, write_cache};
pub use self::client::{CatalogClient, check_key};
use crate::{DEFAULT_EFFORT, DEFAULT_MODEL};

/// The `lifecycle` of a model that efr offers.
const ACTIVE: &str = "active";

/// The effort levels in the order a picker shows them. It orders the levels that the
/// API lists; it never adds one.
const EFFORT_ORDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];

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
    entries: Vec<CatalogEntry>,
    listed: usize,
    origin: CatalogOrigin,
    fetched_at: Timestamp,
    base_url: String,
}

impl Catalog {
    /// A catalog that the API at `base_url` answered with at `now`: the `entries` that
    /// could be read, of `listed` in all.
    pub(crate) fn from_backend(
        base_url: &str,
        entries: Vec<CatalogEntry>,
        listed: usize,
        now: Timestamp,
    ) -> Catalog {
        Catalog {
            entries,
            listed,
            origin: CatalogOrigin::Backend,
            fetched_at: now,
            base_url: base_url.to_owned(),
        }
    }

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
        self.offered().map(CatalogEntry::model).collect()
    }

    /// The model of a turn that names none: [`DEFAULT_MODEL`] when it is on offer, else
    /// the first model on offer.
    pub fn default_model(&self) -> Option<String> {
        if self.offered().any(|entry| entry.id == DEFAULT_MODEL) {
            Some(DEFAULT_MODEL.to_owned())
        } else {
            self.offered().next().map(|entry| entry.id.clone())
        }
    }

    /// How many models of the list are not on offer: not `active`, or not readable.
    pub fn left_out(&self) -> usize {
        self.listed.saturating_sub(self.offered().count())
    }

    /// The base URL of the API that sent the list.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The entries that could be read, in the API's form.
    pub(crate) fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    fn offered(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.iter().filter(|entry| entry.offered())
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
        if catalog.offered().next().is_none() {
            return Applied::Refused { listed: catalog.listed };
        }
        let catalog = Arc::new(catalog);
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&catalog));
        Applied::Changed(catalog)
    }
}

/// `models` with `extra` laid over them: an extra model of the same id gives its window
/// and its output limit, each up to the catalog's limit, and any other extra model is
/// added at the end. Every model changes files with `EditTool::Replace`. The provider
/// sends requests by this list.
pub(crate) fn with_extra(mut models: Vec<ModelInfo>, extra: &[ModelInfo]) -> Vec<ModelInfo> {
    for model in extra {
        match models.iter_mut().find(|known| known.id == model.id) {
            Some(known) => {
                known.context_window = match (model.context_window, known.max_context_window) {
                    (Some(asked), Some(max)) => Some(asked.min(max)),
                    (asked, _) => asked.or(known.context_window),
                };
                known.max_output_tokens = match (model.max_output_tokens, known.max_output_tokens) {
                    (Some(asked), Some(cap)) => Some(asked.min(cap)),
                    (asked, cap) => asked.or(cap),
                };
            }
            None => models.push(model.clone().with_edit_tool(EditTool::Replace)),
        }
    }
    models
}

/// One model of `GET /models`: the members that efr reads, by their names on the wire.
/// The cache file stores entries in the same form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CatalogEntry {
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) display_name: Option<String>,
    #[serde(default, deserialize_with = "tokens", skip_serializing_if = "Option::is_none")]
    pub(crate) max_input_tokens: Option<u64>,
    #[serde(default, deserialize_with = "tokens", skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u64>,
    /// The whole tree, as the API sent it; efr reads `effort`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub(crate) capabilities: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lifecycle: Option<String>,
}

impl CatalogEntry {
    /// True when efr offers the model: its `lifecycle` is `active`, or not given.
    fn offered(&self) -> bool {
        self.lifecycle.as_deref().is_none_or(|lifecycle| lifecycle == ACTIVE)
    }

    /// The model as the provider describes it.
    fn model(&self) -> ModelInfo {
        let efforts = self.efforts();
        let default =
            efforts.iter().any(|effort| effort == DEFAULT_EFFORT).then_some(DEFAULT_EFFORT);
        let mut model = ModelInfo::new(&self.id)
            .with_efforts(efforts, default)
            .with_edit_tool(EditTool::Replace);
        model.context_window = self.max_input_tokens;
        model.max_context_window = self.max_input_tokens;
        model.max_output_tokens = self.max_tokens.and_then(|tokens| u32::try_from(tokens).ok());
        model
    }

    /// The effort levels whose `supported` is true, in [`EFFORT_ORDER`], then any
    /// other level by name.
    fn efforts(&self) -> Vec<String> {
        let Some(levels) = self.capabilities.get("effort").and_then(Value::as_object) else {
            return Vec::new();
        };
        if levels.get("supported").and_then(Value::as_bool) == Some(false) {
            return Vec::new();
        }
        let mut efforts: Vec<&String> = levels
            .iter()
            .filter(|(_, level)| level.get("supported").and_then(Value::as_bool) == Some(true))
            .map(|(name, _)| name)
            .collect();
        efforts.sort_by_key(|name| {
            let rank = EFFORT_ORDER.iter().position(|known| known == name);
            (rank.unwrap_or(EFFORT_ORDER.len()), name.as_str())
        });
        efforts.into_iter().cloned().collect()
    }
}

/// A token count from any JSON value: a positive integer, else unknown.
fn tokens<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(Value::as_u64).filter(|tokens| *tokens > 0))
}

/// The entries of a list of models, `[...]`: each one that reads, and how many did
/// not. A value that is not a list has no entries.
pub(crate) fn entries_of(list: &Value) -> (Vec<CatalogEntry>, usize) {
    let listed = list.as_array().map(Vec::as_slice).unwrap_or_default();
    let mut broken = 0;
    let entries = listed
        .iter()
        .filter_map(|entry| match CatalogEntry::deserialize(entry) {
            Ok(entry) if !entry.id.trim().is_empty() => Some(entry),
            _ => {
                broken += 1;
                None
            }
        })
        .collect();
    (entries, broken)
}

#[cfg(test)]
mod tests;
