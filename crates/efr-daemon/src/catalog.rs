//! The model catalog of the active provider: which models a prompt may name, the
//! window, the efforts and the tool form of each, and the default model.
//!
//! The subscription's catalog comes from its backend (`efr_provider_openai`'s
//! `CatalogClient`). efrd starts with the cache file of the last fetch in its state
//! root ([`CATALOG_FILE`]), else with the table built into efr, and fetches in the
//! background: at start, after a login, then every [`REFRESH_INTERVAL`] with the tag of
//! the list it has, so an unchanged list costs a 304. A failed fetch keeps the list it
//! has and tries again sooner: after 15 s, 30 s, 1 min and 2 min, then every 5 min
//! ([`retry_wait`]), so a start before the network is up gets a list soon. Each new
//! list goes to the cache file.
//! The API key backend keeps the built-in table, because its `/v1/models` says nothing
//! about windows. A daemon whose provider a test injects never fetches.
//!
//! Every reader takes the current list from memory ([`Models::current`]): a prompt,
//! `models.list` and the settings tool never wait for a fetch, and a new list applies
//! from the next turn on.
//!
//! The effective list ([`effective_models`]) is the catalog's offered models, best
//! priority first, then the ids of `[openai] models` that it does not hold. An entry of
//! `[openai] models` with a `context_window` sets the window of a catalog model, up to
//! its `max_context_window`; above it efrd uses the maximum and warns once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_config::Settings;
use efr_protocol::{
    CatalogOrigin as WireOrigin, CatalogStatus, ModelInfo as WireModel, ModelSource,
};
use efr_provider::ProviderError;
use efr_provider_openai::{
    Applied, Backend, Catalog, CatalogClient, CatalogOrigin, ModelCatalog, OpenAiConfig,
};
use efr_stdx::time::Clock;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::providers::API;

/// The cache file of the catalog, in the state root.
pub(crate) const CATALOG_FILE: &str = "model_catalog.json";

/// How often efrd asks the backend again after an answer.
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

/// How soon efrd asks again after one, two, three and four fetches in a row that
/// failed.
const RETRY_WAITS: [Duration; 4] = [
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];

/// How soon efrd asks again after five or more fetches in a row that failed.
const LONGEST_RETRY_WAIT: Duration = Duration::from_secs(300);

/// How soon efrd asks again after `failures` fetches in a row that failed (1 or more):
/// the waits grow from 15 s to 5 min, and stay at 5 min.
pub(crate) fn retry_wait(failures: u32) -> Duration {
    let index = usize::try_from(failures.saturating_sub(1)).unwrap_or(usize::MAX);
    RETRY_WAITS.get(index).copied().unwrap_or(LONGEST_RETRY_WAIT)
}

/// One entry of `[openai] models` whose window is above the model's largest one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Clamp {
    pub(crate) model: String,
    pub(crate) asked: u64,
    pub(crate) max: u64,
}

/// The catalog and the task that keeps it fresh.
#[derive(Debug)]
pub(crate) struct Models {
    catalog: ModelCatalog,
    refresh: Option<Refresh>,
    wake: Notify,
    /// The clamps already warned about, so each costs one warning.
    warned: Mutex<HashSet<Clamp>>,
}

/// What a fetch needs.
#[derive(Debug)]
struct Refresh {
    client: CatalogClient,
    cache: PathBuf,
    clock: Arc<dyn Clock>,
    /// The fetches in a row that failed, which set the wait before the next one.
    failures: AtomicU32,
}

impl Models {
    /// The models of `catalog`, never fetched again.
    pub(crate) fn fixed(catalog: ModelCatalog) -> Self {
        Models { catalog, refresh: None, wake: Notify::new(), warned: Mutex::default() }
    }

    /// The models of `catalog`, fetched again with `client` and kept in `cache`.
    pub(crate) fn fetched(
        catalog: ModelCatalog,
        client: CatalogClient,
        cache: PathBuf,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let refresh = Refresh { client, cache, clock, failures: AtomicU32::new(0) };
        Models { refresh: Some(refresh), ..Models::fixed(catalog) }
    }

    /// True when efrd fetches the catalog from the backend.
    pub(crate) fn fetches(&self) -> bool {
        self.refresh.is_some()
    }

    /// The current catalog.
    pub(crate) fn current(&self) -> Arc<Catalog> {
        self.catalog.current()
    }

    /// Where the current catalog came from, for `models.list` and `admin.status`.
    pub(crate) fn status(&self) -> CatalogStatus {
        status(&self.current())
    }

    /// The model of a turn that names none, with `settings`.
    pub(crate) fn default_model(&self, settings: &Settings) -> String {
        default_model(settings, &self.current())
    }

    /// The effective model list with `settings`. A window of `[openai] models` above
    /// the model's largest one is cut down to it, with one warning per entry.
    pub(crate) fn effective(&self, settings: &Settings) -> Vec<WireModel> {
        let (models, clamps) = effective_models(settings, &self.current());
        if !clamps.is_empty() {
            let mut warned = self.warned.lock().unwrap_or_else(PoisonError::into_inner);
            for clamp in clamps {
                if warned.insert(clamp.clone()) {
                    tracing::warn!(
                        model = %clamp.model,
                        context_window = clamp.asked,
                        max_context_window = clamp.max,
                        "openai.models sets a context window above the largest that the model takes; efrd uses the largest"
                    );
                }
            }
        }
        models
    }

    /// Asks the task to fetch now, such as after a login.
    pub(crate) fn refresh_now(&self) {
        self.wake.notify_one();
    }

    /// Fetches the catalog now, then again after each wait, until `stop`. Returns at
    /// once for a catalog that is never fetched.
    pub(crate) async fn serve(self: Arc<Self>, stop: CancellationToken) {
        let Some(refresh) = &self.refresh else {
            return;
        };
        loop {
            let wait = tokio::select! {
                () = stop.cancelled() => return,
                wait = self.fetch(refresh) => wait,
            };
            tokio::select! {
                () = stop.cancelled() => return,
                () = refresh.clock.sleep(wait) => {}
                () = self.wake.notified() => {}
            }
        }
    }

    /// One fetch, and the wait until the next one.
    async fn fetch(&self, refresh: &Refresh) -> Duration {
        let current = self.catalog.current();
        let fetched = match refresh.client.fetch(&current).await {
            Ok(fetched) => fetched,
            Err(ProviderError::NotLoggedIn) => {
                tracing::debug!("no login yet, so the model catalog is not fetched");
                refresh.failures.store(0, Ordering::Relaxed);
                return REFRESH_INTERVAL;
            }
            Err(error) => {
                let failures = refresh.failures.fetch_add(1, Ordering::Relaxed).saturating_add(1);
                let wait = retry_wait(failures);
                tracing::warn!(error = %efr_stdx::with_causes(&error), origin = ?current.origin(), failures, retry_in_s = wait.as_secs(), "the model catalog could not be fetched; the current list stays");
                return wait;
            }
        };
        refresh.failures.store(0, Ordering::Relaxed);
        match self.catalog.apply(fetched, refresh.clock.now()) {
            Applied::Changed(catalog) => {
                tracing::info!(
                    models = catalog.models().len(),
                    left_out = catalog.left_out(),
                    default = ?catalog.default_model(),
                    "the model catalog came from the backend"
                );
                write(&refresh.cache, catalog).await;
            }
            Applied::Revalidated(catalog) => {
                tracing::debug!("the backend says that the model catalog did not change");
                write(&refresh.cache, catalog).await;
            }
            Applied::Refused { listed } => {
                // NOTE: the ChatGPT backend lists its models only for a Codex
                // `client_version`, so efr gets an empty list today, and the built-in
                // table stays. A warning every hour would only be noise.
                tracing::debug!(
                    listed,
                    "the backend's model catalog offers no model that efr can use; the current list stays"
                );
            }
            _ => {
                tracing::debug!("the backend said not modified for a list that efrd does not have")
            }
        }
        REFRESH_INTERVAL
    }
}

/// Writes `catalog` to the cache file at `path`, off the async workers.
async fn write(path: &Path, catalog: Arc<Catalog>) {
    let owned = path.to_path_buf();
    let written =
        tokio::task::spawn_blocking(move || efr_provider_openai::write_cache(&owned, &catalog))
            .await;
    match written {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %efr_stdx::with_causes(&error), "the model catalog cache could not be written");
        }
        Err(join) => tracing::warn!(error = %join, "the model catalog cache could not be written"),
    }
}

/// The backend of the provider that `settings` names.
pub(crate) fn backend(settings: &Settings) -> Backend {
    if settings.model.provider == API { Backend::Api } else { Backend::Subscription }
}

/// The catalog that efrd starts with: the cache file at `path` when it holds a list of
/// the configured subscription backend that offers a model, else the built-in table.
pub(crate) async fn load(settings: &Settings, path: &Path) -> Catalog {
    let backend = backend(settings);
    if backend == Backend::Api {
        return Catalog::builtin(backend);
    }
    let base_url = subscription_config(settings)
        .map(|config| config.base_url().to_owned())
        .unwrap_or_else(|| efr_provider_openai::SUBSCRIPTION_BASE_URL.to_owned());
    let owned = path.to_path_buf();
    let read = tokio::task::spawn_blocking(move || {
        efr_provider_openai::read_cache(&owned, backend, &base_url)
    })
    .await;
    match read {
        Ok(Ok(Some(catalog))) if !catalog.models().is_empty() => {
            tracing::info!(
                fetched_at = ?catalog.fetched_at(),
                models = catalog.models().len(),
                "the model catalog comes from the cache until the backend answers"
            );
            catalog
        }
        Ok(Ok(_)) => Catalog::builtin(backend),
        Ok(Err(error)) => {
            tracing::warn!(error = %efr_stdx::with_causes(&error), "the model catalog cache could not be used; the built-in list stands in");
            Catalog::builtin(backend)
        }
        Err(join) => {
            tracing::warn!(error = %join, "the model catalog cache could not be read; the built-in list stands in");
            Catalog::builtin(backend)
        }
    }
}

/// The subscription's config with the base URL of `settings`, or `None` when that URL
/// is not one (the config's checks refuse such a file before this runs).
fn subscription_config(settings: &Settings) -> Option<OpenAiConfig> {
    let config = OpenAiConfig::subscription();
    match settings.openai.subscription_base_url.as_deref() {
        Some(url) => config.with_base_url(url).ok(),
        None => Some(config),
    }
}

/// Where `catalog` came from, on the wire.
pub(crate) fn status(catalog: &Catalog) -> CatalogStatus {
    let origin = match catalog.origin() {
        CatalogOrigin::Backend => WireOrigin::Backend,
        CatalogOrigin::Cache => WireOrigin::Cache,
        _ => WireOrigin::Builtin,
    };
    CatalogStatus { origin, fetched_at: catalog.fetched_at() }
}

/// The model of new conversations: `[model] name`, else the catalog's offered model
/// with the best priority, else the first model of `[openai] models`.
pub(crate) fn default_model(settings: &Settings, catalog: &Catalog) -> String {
    settings
        .model
        .name
        .clone()
        .or_else(|| catalog.default_model())
        .or_else(|| {
            settings
                .openai
                .models
                .as_ref()
                .and_then(|models| models.first())
                .map(|model| model.id().to_owned())
        })
        .unwrap_or_default()
}

/// The effective model list with `settings` over `catalog`, as `models.list` answers it
/// and a turn checks against it, and the windows of `[openai] models` that were cut
/// down to a model's largest one.
pub(crate) fn effective_models(
    settings: &Settings,
    catalog: &Catalog,
) -> (Vec<WireModel>, Vec<Clamp>) {
    let default = default_model(settings, catalog);
    let mut models: Vec<WireModel> = catalog
        .models()
        .into_iter()
        .map(|model| WireModel {
            default: model.id == default,
            id: model.id,
            efforts: model.efforts,
            default_effort: model.default_effort,
            source: ModelSource::Builtin,
            context_window: model.context_window,
            max_context_window: model.max_context_window,
            prefer_websockets: model.prefer_websockets,
        })
        .collect();
    let mut clamps = Vec::new();
    for entry in settings.openai.models.iter().flatten() {
        if let Some(model) = models.iter_mut().find(|model| model.id == entry.id()) {
            let Some(asked) = entry.context_window() else {
                continue;
            };
            model.context_window = match model.max_context_window {
                Some(max) if asked > max => {
                    clamps.push(Clamp { model: model.id.clone(), asked, max });
                    Some(max)
                }
                _ => Some(asked),
            };
            continue;
        }
        models.push(WireModel {
            id: entry.id().to_owned(),
            efforts: Vec::new(),
            default_effort: None,
            default: entry.id() == default,
            source: ModelSource::Config,
            context_window: entry.context_window(),
            max_context_window: None,
            prefer_websockets: false,
        });
    }
    (models, clamps)
}

#[cfg(test)]
mod tests;
