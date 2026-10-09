//! The model catalog: the models a backend offers, with their windows, their efforts
//! and the form of their tools.
//!
//! The subscription backend lists its models at `GET <base_url>/models` (Codex
//! `codex-rs/codex-api/src/endpoint/models.rs`), one entry per model in the shape of
//! Codex's `ModelInfo` (`codex-rs/protocol/src/openai_models.rs`). A [`Catalog`] holds
//! one such list and where it came from ([`CatalogOrigin`]): the backend, the cache
//! file of an earlier fetch, or the table built into efr. A [`ModelCatalog`] is the
//! shared, current catalog that requests read from memory: a fetch never blocks a
//! request, and its result applies from the next request on.
//!
//! A model is offered when its entry says `visibility: "list"`, when efr knows the
//! form of its tools (`apply_patch_tool_type`), and, on the API key backend, when it is
//! `supported_in_api`. The default model is the offered model with the best (lowest)
//! `priority`. An entry that cannot be read is left out; the others stay.
//!
//! The API key backend answers `GET <base_url>/models` with ids only (`{"object":
//! "list", "data": [{"id": ...}]}`), and no window, effort or tool form. Its catalog is
//! the table built into efr cut down to the ids that the key lists: a model that the
//! key cannot use is not offered, and a model that the table does not know is not
//! offered either, because efr does not know its tools. `[openai] models` can still
//! add one.
//!
//! efr ignores `minimal_client_version`: it is a version of Codex, which efr's own
//! version cannot be compared with.

mod cache;
mod client;

use std::sync::{Arc, PoisonError, RwLock};

use efr_provider::ModelInfo;
use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub use self::cache::{read_cache, write_cache};
pub use self::client::{CatalogClient, Fetched, check_key};
use crate::config::Backend;
use crate::models::builtin_entries;

/// The version that efr sends as `client_version`: efr's own, never another client's.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The value of `apply_patch_tool_type` for a model that takes freeform tools.
pub(crate) const FREEFORM: &str = "freeform";

/// The value of `apply_patch_tool_type` for a model that takes function tools only.
const FUNCTION: &str = "function";

/// Where a [`Catalog`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CatalogOrigin {
    /// The backend answered this daemon with it, or confirmed it with a 304.
    Backend,
    /// The cache file of an earlier fetch, read at start.
    Cache,
    /// The table built into efr.
    Builtin,
}

/// One list of models and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    backend: Backend,
    entries: Vec<CatalogEntry>,
    origin: CatalogOrigin,
    fetched_at: Option<Timestamp>,
    etag: Option<String>,
    base_url: Option<String>,
    client_version: Option<String>,
}

impl Catalog {
    /// The table built into efr, for `backend`.
    pub fn builtin(backend: Backend) -> Catalog {
        Catalog {
            backend,
            entries: builtin_entries(backend),
            origin: CatalogOrigin::Builtin,
            fetched_at: None,
            etag: None,
            base_url: None,
            client_version: None,
        }
    }

    /// A catalog that the backend at `base_url` answered with at `now`.
    pub(crate) fn from_backend(
        backend: Backend,
        base_url: &str,
        entries: Vec<CatalogEntry>,
        etag: Option<String>,
        now: Timestamp,
    ) -> Catalog {
        Catalog {
            backend,
            entries,
            origin: CatalogOrigin::Backend,
            fetched_at: Some(now),
            etag,
            base_url: Some(base_url.to_owned()),
            client_version: Some(CLIENT_VERSION.to_owned()),
        }
    }

    /// The catalog of the API key backend at `base_url`, whose `/models` listed `ids`
    /// with the tag `etag` at `now`: the models of the built-in table whose id the key
    /// lists.
    pub(crate) fn from_api(
        base_url: &str,
        ids: &[String],
        etag: Option<String>,
        now: Timestamp,
    ) -> Catalog {
        let entries = builtin_entries(Backend::Api)
            .into_iter()
            .filter(|entry| ids.contains(&entry.slug))
            .collect();
        Catalog::from_backend(Backend::Api, base_url, entries, etag, now)
    }

    /// Where the catalog came from.
    pub fn origin(&self) -> CatalogOrigin {
        self.origin
    }

    /// When the backend last sent or confirmed the catalog; `None` for the built-in
    /// table.
    pub fn fetched_at(&self) -> Option<Timestamp> {
        self.fetched_at
    }

    /// The backend's tag of this list, which a later fetch sends in `If-None-Match`.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// The models on offer, best priority first.
    pub fn models(&self) -> Vec<ModelInfo> {
        let mut offered: Vec<&CatalogEntry> =
            self.entries.iter().filter(|entry| entry.offered(self.backend)).collect();
        offered.sort_by_key(|entry| entry.priority);
        offered.into_iter().map(CatalogEntry::model).collect()
    }

    /// The model of a turn that names none: the offered model with the best priority.
    pub fn default_model(&self) -> Option<String> {
        self.models().into_iter().next().map(|model| model.id)
    }

    /// How many models of the list are not on offer: hidden, with a tool form that efr
    /// does not know, or not served on this backend.
    pub fn left_out(&self) -> usize {
        self.entries.iter().filter(|entry| !entry.offered(self.backend)).count()
    }

    /// The tag to send when the backend at `base_url` is asked again: only for a list
    /// that the same backend sent to this version of efr, because another version can
    /// get another list.
    pub(crate) fn etag_for(&self, base_url: &str) -> Option<&str> {
        let same_backend = self.base_url.as_deref() == Some(base_url);
        let same_client = self.client_version.as_deref() == Some(CLIENT_VERSION);
        match self.origin {
            CatalogOrigin::Builtin => None,
            _ if same_backend && same_client => self.etag.as_deref(),
            _ => None,
        }
    }

    /// The same list, confirmed by the backend at `now`.
    pub(crate) fn revalidated(&self, now: Timestamp) -> Catalog {
        Catalog { origin: CatalogOrigin::Backend, fetched_at: Some(now), ..self.clone() }
    }
}

/// The current catalog, shared by the provider, which reads it for each request, and
/// the task that fetches a new one.
///
/// A reader takes the current catalog from memory and keeps it for its unit of work; a
/// new catalog replaces it for the readers after that. No reader ever waits for a
/// fetch.
#[derive(Debug, Clone)]
pub struct ModelCatalog {
    current: Arc<RwLock<Arc<Catalog>>>,
}

/// What [`ModelCatalog::apply`] did with a fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Applied {
    /// A new list is current now; it belongs in the cache.
    Changed(Arc<Catalog>),
    /// The backend confirmed the current list; its new time belongs in the cache.
    Revalidated(Arc<Catalog>),
    /// The backend's list offers no model that efr can use, so the current catalog
    /// stays. `listed` is how many models the list held.
    Refused {
        /// How many models the backend listed.
        listed: usize,
    },
    /// The backend said "not modified" for a list that efr did not have, so nothing
    /// changed.
    Ignored,
}

impl ModelCatalog {
    /// A shared catalog that starts as `catalog`.
    pub fn new(catalog: Catalog) -> Self {
        ModelCatalog { current: Arc::new(RwLock::new(Arc::new(catalog))) }
    }

    /// The current catalog.
    pub fn current(&self) -> Arc<Catalog> {
        // NOTE: a panic while the lock was held cannot leave a half-written catalog,
        // because the value is one `Arc` that is swapped whole.
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Makes the result of a fetch at `now` current, unless it offers no model.
    pub fn apply(&self, fetched: Fetched, now: Timestamp) -> Applied {
        let mut current = self.current.write().unwrap_or_else(PoisonError::into_inner);
        match fetched {
            Fetched::Changed(catalog) if catalog.models().is_empty() => {
                Applied::Refused { listed: catalog.entries.len() }
            }
            Fetched::Changed(catalog) => {
                let catalog = Arc::new(catalog);
                *current = Arc::clone(&catalog);
                Applied::Changed(catalog)
            }
            Fetched::NotModified if current.origin == CatalogOrigin::Builtin => Applied::Ignored,
            Fetched::NotModified => {
                let catalog = Arc::new(current.revalidated(now));
                *current = Arc::clone(&catalog);
                Applied::Revalidated(catalog)
            }
        }
    }
}

/// `models` with `extra` laid over them: an extra model of the same id gives its window
/// and output limit where it has them, and any other extra model is added at the end.
/// The provider sends requests by this list.
pub(crate) fn with_extra(mut models: Vec<ModelInfo>, extra: &[ModelInfo]) -> Vec<ModelInfo> {
    for model in extra {
        match models.iter_mut().find(|known| known.id == model.id) {
            Some(known) => {
                known.context_window = model.context_window.or(known.context_window);
                known.max_output_tokens = model.max_output_tokens.or(known.max_output_tokens);
            }
            None => models.push(model.clone()),
        }
    }
    models
}

/// Whether a catalog shows a model in its picker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Visibility {
    /// Shown, so efr offers it. An entry without the key counts as shown.
    #[default]
    List,
    /// Hidden, such as a model for one task of Codex.
    Hide,
    /// Not shown.
    None,
    /// A value that this version of efr does not know; not offered.
    #[serde(other)]
    Other,
}

/// One reasoning level of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReasoningLevel {
    pub(crate) effort: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
}

impl From<&str> for ReasoningLevel {
    fn from(effort: &str) -> Self {
        ReasoningLevel { effort: effort.to_owned(), description: None }
    }
}

/// One model of a catalog: the members of Codex's `ModelInfo` that efr reads, by their
/// names on the wire. The cache file stores entries in the same form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CatalogEntry {
    pub(crate) slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// Lower is better; a missing or broken value sorts last.
    #[serde(default = "last_priority", deserialize_with = "priority")]
    pub(crate) priority: i64,
    #[serde(default)]
    pub(crate) visibility: Visibility,
    #[serde(default, deserialize_with = "tokens", skip_serializing_if = "Option::is_none")]
    pub(crate) context_window: Option<u64>,
    #[serde(default, deserialize_with = "tokens", skip_serializing_if = "Option::is_none")]
    pub(crate) max_context_window: Option<u64>,
    #[serde(default)]
    pub(crate) supported_reasoning_levels: Vec<ReasoningLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default_reasoning_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) apply_patch_tool_type: Option<String>,
    #[serde(default)]
    pub(crate) prefer_websockets: bool,
    /// Whether the API key backend serves the model; an entry without it counts as
    /// served.
    #[serde(default = "served")]
    pub(crate) supported_in_api: bool,
}

impl CatalogEntry {
    /// True when efr offers the model on `backend`.
    fn offered(&self, backend: Backend) -> bool {
        self.visibility == Visibility::List
            && (backend != Backend::Api || self.supported_in_api)
            && matches!(self.apply_patch_tool_type.as_deref(), None | Some(FREEFORM | FUNCTION))
    }

    /// The model as a provider describes it. A missing window takes the largest one,
    /// and the largest one is never below the window.
    fn model(&self) -> ModelInfo {
        let window = self.context_window.or(self.max_context_window);
        let max_window = match (self.max_context_window, window) {
            (Some(max), Some(window)) => Some(max.max(window)),
            (max, window) => max.or(window),
        };
        let efforts = self.supported_reasoning_levels.iter().map(|level| level.effort.clone());
        let mut model = ModelInfo::new(&self.slug)
            .with_efforts(efforts, self.default_reasoning_level.as_deref())
            .with_freeform_tools(self.apply_patch_tool_type.as_deref() == Some(FREEFORM))
            .with_prefer_websockets(self.prefer_websockets);
        model.context_window = window;
        model.max_context_window = max_window;
        model
    }
}

const fn last_priority() -> i64 {
    i64::MAX
}

const fn served() -> bool {
    true
}

/// A priority from any JSON value: an integer, else the last place.
fn priority<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(Value::as_i64).unwrap_or(i64::MAX))
}

/// A token count from any JSON value: a positive integer, else unknown. Codex's
/// catalog uses signed numbers and `null`.
fn tokens<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(Value::as_u64).filter(|tokens| *tokens > 0))
}

/// The entries of a catalog body, `{"models": [...]}`: each one that reads, and how
/// many did not. `None` when the body has no list of models.
pub(crate) fn entries_of(body: &Value) -> Option<(Vec<CatalogEntry>, usize)> {
    let listed = body.get("models")?.as_array()?;
    let mut broken = 0;
    let entries = listed
        .iter()
        .filter_map(|entry| match CatalogEntry::deserialize(entry) {
            Ok(entry) if !entry.slug.trim().is_empty() => Some(entry),
            _ => {
                broken += 1;
                None
            }
        })
        .collect();
    Some((entries, broken))
}

/// The model ids of an API key backend's list, `{"data": [{"id": ...}]}`. `None` when
/// the body has no such list. An entry without an id is left out.
pub(crate) fn api_ids_of(body: &Value) -> Option<Vec<String>> {
    let listed = body.get("data")?.as_array()?;
    let ids = listed
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .collect();
    Some(ids)
}

#[cfg(test)]
mod tests;
