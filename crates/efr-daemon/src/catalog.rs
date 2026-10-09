//! The model catalog of the active provider: which models a prompt may name, the
//! window, the efforts and the tool form of each, and the default model.
//!
//! The catalog has the form of the provider's company ([`ProviderCatalog`]): an OpenAI
//! catalog for `openai-subscription` and `openai-api`, an Anthropic one for
//! `anthropic-api`. Each company's list has a cache file of its own in the state root
//! ([`Vendor::cache_file`]), so a change of `[model] provider` never reads the list of
//! the other company.
//!
//! The subscription's catalog comes from its backend (`efr_provider_openai`'s
//! `CatalogClient`). efrd starts with the cache file of the last fetch, else with the
//! table built into efr, and fetches in the background: at start, after a login, then
//! every [`REFRESH_INTERVAL`] with the tag of the list it has, so an unchanged list
//! costs a 304. A failed fetch keeps the list it has and tries again sooner: after
//! 15 s, 30 s, 1 min and 2 min, then every 5 min ([`retry_wait`]), so a start before
//! the network is up gets a list soon. Each new list goes to the cache file.
//! The API key backend fetches its `/v1/models` with the stored key the same way: its
//! ids cut the built-in table down to the models that the key can use, because the
//! list says nothing about windows or tools.
//!
//! The Anthropic catalog comes from `GET /v1/models` with the stored key
//! (`efr_provider_anthropic`'s `CatalogClient`), at the same times and with the same
//! waits. efr has no table of Claude models, so without a cache file there is no list
//! until a fetch works. Then a prompt waits for one fetch before it goes to its
//! conversation ([`Models::ready`]): a turn without a list cannot know the output limit
//! of its model, and efr never guesses it. A fetch that ends without a list keeps why
//! ([`NoList`]): no key is stored, the fetch failed, or the list has no active model, so
//! the prompt that needs the list fails with the cause (`prompt_send.rs`).
//!
//! A fetch with a stored key reports a 401 to the key's refusals
//! (`providers/refusals.rs`), and a fetch that works says that the key works.
//!
//! A daemon whose provider a test injects never fetches.
//!
//! Every reader takes the current list from memory ([`Models::list`]): a prompt (when a
//! list exists), `models.list` and the settings tool never wait for a fetch, and a new
//! list applies from the next turn on.
//!
//! The effective list ([`effective_models`]) is the catalog's offered models, then the
//! ids of the config's list of the company (`[openai] models` or `[anthropic] models`)
//! that it does not hold. An entry with a `context_window` sets the window of a catalog
//! model, up to its `max_context_window`; above it efrd uses the maximum and warns
//! once.

mod list;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_config::Settings;
use efr_protocol::{CatalogStatus, ModelInfo as WireModel, ModelSource};
use efr_provider::ProviderError;
use efr_stdx::time::Clock;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

pub(crate) use self::list::{ModelList, ProviderCatalog, Vendor, backend};
use crate::providers::{KeyRefusals, KeySource};

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

/// How long a prompt waits at most for the fetch of a catalog that has no list. The
/// fetch's own timeouts end it sooner; this only bounds a fetch task that is stuck.
const LIST_WAIT: Duration = Duration::from_secs(60);

/// How soon efrd asks again after `failures` fetches in a row that failed (1 or more):
/// the waits grow from 15 s to 5 min, and stay at 5 min.
pub(crate) fn retry_wait(failures: u32) -> Duration {
    let index = usize::try_from(failures.saturating_sub(1)).unwrap_or(usize::MAX);
    RETRY_WAITS.get(index).copied().unwrap_or(LONGEST_RETRY_WAIT)
}

/// One entry of the config's list of models whose window is above the model's largest
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Clamp {
    pub(crate) model: String,
    pub(crate) asked: u64,
    pub(crate) max: u64,
}

/// The catalog and the task that keeps it fresh.
#[derive(Debug)]
pub(crate) struct Models {
    /// The provider whose catalog this is.
    provider: String,
    catalog: ProviderCatalog,
    refresh: Option<Refresh>,
    wake: Notify,
    /// True while a fetch runs, so a prompt that waits for a list does not ask for a
    /// second one.
    fetching: AtomicBool,
    /// How many fetches have ended, so a prompt can wait for the next one.
    ended: watch::Sender<u64>,
    /// The clamps already warned about, so each costs one warning.
    warned: Mutex<HashSet<Clamp>>,
    /// Why the last fetch ended without a list, for a catalog that has none.
    no_list: Mutex<NoList>,
    /// Where a fetch with a stored API key reports whether the key works; `None` for
    /// a provider without a key.
    refusals: Option<Arc<KeyRefusals>>,
}

/// Why a catalog that efrd fetches has no list.
#[derive(Debug, Clone)]
pub(crate) enum NoList {
    /// No fetch has ended yet, or none ended in time for the prompt.
    NotFetched,
    /// No key is stored, so no fetch could run.
    NotLoggedIn,
    /// The last fetch failed.
    Failed(Arc<ProviderError>),
    /// The last fetch got a list without an active model.
    NoActiveModel,
}

/// What a fetch asks, and the shared catalog that its answer goes to.
#[derive(Debug)]
pub(crate) enum Fetcher {
    /// An OpenAI backend and its catalog.
    OpenAi(efr_provider_openai::ModelCatalog, efr_provider_openai::CatalogClient),
    /// The Anthropic API and its catalog.
    Anthropic(efr_provider_anthropic::ModelCatalog, efr_provider_anthropic::CatalogClient),
}

impl Fetcher {
    fn catalog(&self) -> ProviderCatalog {
        match self {
            Fetcher::OpenAi(catalog, _) => ProviderCatalog::OpenAi(catalog.clone()),
            Fetcher::Anthropic(catalog, _) => ProviderCatalog::Anthropic(catalog.clone()),
        }
    }
}

/// What a fetch needs.
#[derive(Debug)]
struct Refresh {
    fetcher: Fetcher,
    cache: PathBuf,
    clock: Arc<dyn Clock>,
    /// The fetches in a row that failed, which set the wait before the next one.
    failures: AtomicU32,
}

impl Models {
    /// The models of `catalog`, the catalog of `provider`, never fetched again.
    pub(crate) fn fixed(provider: &str, catalog: ProviderCatalog) -> Self {
        Models {
            provider: provider.to_owned(),
            catalog,
            refresh: None,
            wake: Notify::new(),
            fetching: AtomicBool::new(false),
            ended: watch::Sender::new(0),
            warned: Mutex::default(),
            no_list: Mutex::new(NoList::NotFetched),
            refusals: None,
        }
    }

    /// The same models, whose fetches report whether the stored key of the provider
    /// works to `refusals`.
    pub(crate) fn with_refusals(self, refusals: Arc<KeyRefusals>) -> Self {
        Models { refusals: Some(refusals), ..self }
    }

    /// The models of the catalog of `fetcher`, fetched again with it and kept in
    /// `cache`.
    pub(crate) fn fetched(
        provider: &str,
        fetcher: Fetcher,
        cache: PathBuf,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let catalog = fetcher.catalog();
        let refresh = Refresh { fetcher, cache, clock, failures: AtomicU32::new(0) };
        Models { refresh: Some(refresh), ..Models::fixed(provider, catalog) }
    }

    /// True when efrd fetches the catalog from the backend.
    pub(crate) fn fetches(&self) -> bool {
        self.refresh.is_some()
    }

    /// The current list.
    pub(crate) fn list(&self) -> ModelList {
        self.catalog.list()
    }

    /// Why there is no list, for a catalog that efrd fetches and that has none; `None`
    /// when there is a list or efrd does not fetch.
    pub(crate) fn missing(&self) -> Option<NoList> {
        if self.refresh.is_none() || self.catalog.has_list() {
            return None;
        }
        Some(self.no_list.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }

    /// Where the current list came from, for `models.list` and `admin.status`.
    pub(crate) fn status(&self) -> CatalogStatus {
        self.list().status(&self.provider)
    }

    /// The model of a turn that names none, with `settings`.
    pub(crate) fn default_model(&self, settings: &Settings) -> String {
        default_model(settings, &self.list())
    }

    /// The effective model list with `settings`. A window of the config's list above
    /// the model's largest one is cut down to it, with one warning per entry.
    pub(crate) fn effective(&self, settings: &Settings) -> Vec<WireModel> {
        let list = self.list();
        let (models, clamps) = effective_models(settings, &list);
        if !clamps.is_empty() {
            let mut warned = self.warned.lock().unwrap_or_else(PoisonError::into_inner);
            for clamp in clamps {
                if warned.insert(clamp.clone()) {
                    tracing::warn!(
                        key = list.vendor().models_key(),
                        model = %clamp.model,
                        context_window = clamp.asked,
                        max_context_window = clamp.max,
                        "the config's model list sets a context window above the largest that the model takes; efrd uses the largest"
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

    /// Waits for one fetch when the catalog has no list and efrd fetches it, so the
    /// prompt that comes next has its model's limits. Returns at once when there is a
    /// list, when efrd does not fetch, and after one fetch whatever it gave: a fetch
    /// that failed leaves the turn to fail with the provider's error.
    pub(crate) async fn ready(&self) {
        let Some(refresh) = &self.refresh else {
            return;
        };
        // NOTE: subscribed before the check, so a fetch that ends in between is seen.
        let mut ended = self.ended.subscribe();
        if self.catalog.has_list() {
            return;
        }
        if !self.fetching.load(Ordering::Acquire) {
            self.wake.notify_one();
        }
        tracing::debug!(provider = %self.provider, "no model list yet; the prompt waits for a fetch");
        tokio::select! {
            _ = ended.changed() => {}
            () = refresh.clock.sleep(LIST_WAIT) => {
                tracing::warn!(provider = %self.provider, wait_s = LIST_WAIT.as_secs(), "the fetch of the model list did not end in time; the prompt goes on without a list");
            }
        }
    }

    /// Fetches the catalog now, then again after each wait, until `stop`. Returns at
    /// once for a catalog that is never fetched.
    pub(crate) async fn serve(self: Arc<Self>, stop: CancellationToken) {
        let Some(refresh) = &self.refresh else {
            return;
        };
        loop {
            self.fetching.store(true, Ordering::Release);
            let wait = tokio::select! {
                () = stop.cancelled() => return,
                wait = self.fetch(refresh) => wait,
            };
            self.fetching.store(false, Ordering::Release);
            tokio::select! {
                () = stop.cancelled() => return,
                () = refresh.clock.sleep(wait) => {}
                () = self.wake.notified() => {}
            }
        }
    }

    /// One fetch, and the wait until the next one.
    async fn fetch(&self, refresh: &Refresh) -> Duration {
        let used = self
            .refusals
            .as_ref()
            .map(|refusals| refusals.start(&self.provider, KeySource::ModelList));
        let fetched = match &refresh.fetcher {
            Fetcher::OpenAi(catalog, client) => fetch_openai(catalog, client, refresh).await,
            Fetcher::Anthropic(catalog, client) => fetch_anthropic(catalog, client, refresh).await,
        };
        if let Some(used) = &used {
            match &fetched {
                Ok(()) => used.worked(),
                Err(error) => used.failed(error),
            }
        }
        let (wait, no_list) = match fetched {
            Ok(()) => {
                refresh.failures.store(0, Ordering::Relaxed);
                (REFRESH_INTERVAL, NoList::NoActiveModel)
            }
            Err(ProviderError::NotLoggedIn) => {
                tracing::debug!(provider = %self.provider, "no login yet, so the model catalog is not fetched");
                refresh.failures.store(0, Ordering::Relaxed);
                (REFRESH_INTERVAL, NoList::NotLoggedIn)
            }
            Err(error) => {
                let failures = refresh.failures.fetch_add(1, Ordering::Relaxed).saturating_add(1);
                let wait = retry_wait(failures);
                tracing::warn!(error = %efr_stdx::with_causes(&error), provider = %self.provider, origin = ?self.list().origin(), failures, retry_in_s = wait.as_secs(), "the model catalog could not be fetched; the current list stays");
                (wait, NoList::Failed(Arc::new(error)))
            }
        };
        // NOTE: kept before the end is announced, so a prompt that waits for this fetch
        // reads why it gave no list.
        *self.no_list.lock().unwrap_or_else(PoisonError::into_inner) = no_list;
        self.ended.send_modify(|ended| *ended = ended.wrapping_add(1));
        wait
    }
}

/// One fetch of an OpenAI catalog: a new or confirmed list becomes current and goes to
/// the cache file.
async fn fetch_openai(
    catalog: &efr_provider_openai::ModelCatalog,
    client: &efr_provider_openai::CatalogClient,
    refresh: &Refresh,
) -> Result<(), ProviderError> {
    use efr_provider_openai::Applied;

    let fetched = client.fetch(&catalog.current()).await?;
    match catalog.apply(fetched, refresh.clock.now()) {
        Applied::Changed(catalog) => {
            tracing::info!(
                models = catalog.models().len(),
                left_out = catalog.left_out(),
                default = ?catalog.default_model(),
                "the model catalog came from the backend"
            );
            write(&refresh.cache, move |path| efr_provider_openai::write_cache(path, &catalog))
                .await;
        }
        Applied::Revalidated(catalog) => {
            tracing::debug!("the backend says that the model catalog did not change");
            write(&refresh.cache, move |path| efr_provider_openai::write_cache(path, &catalog))
                .await;
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
    Ok(())
}

/// One fetch of the Anthropic catalog: a list with an active model becomes current
/// and goes to the cache file.
async fn fetch_anthropic(
    catalog: &efr_provider_anthropic::ModelCatalog,
    client: &efr_provider_anthropic::CatalogClient,
    refresh: &Refresh,
) -> Result<(), ProviderError> {
    use efr_provider_anthropic::Applied;

    let fetched = client.fetch().await?;
    match catalog.apply(fetched) {
        Applied::Changed(catalog) => {
            tracing::info!(
                models = catalog.models().len(),
                left_out = catalog.left_out(),
                default = ?catalog.default_model(),
                "the model catalog came from the API"
            );
            write(&refresh.cache, move |path| efr_provider_anthropic::write_cache(path, &catalog))
                .await;
        }
        Applied::Refused { listed } => {
            tracing::warn!(
                listed,
                "the API's model list has no active model; the current list stays"
            );
        }
        _ => tracing::debug!("the API's model list changed nothing"),
    }
    Ok(())
}

/// Writes a catalog to the cache file at `path` with `save`, off the async workers.
async fn write<E>(path: &Path, save: impl FnOnce(&Path) -> Result<(), E> + Send + 'static)
where
    E: std::error::Error + Send + 'static,
{
    let owned = path.to_path_buf();
    let written = tokio::task::spawn_blocking(move || save(&owned)).await;
    match written {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %efr_stdx::with_causes(&error), "the model catalog cache could not be written");
        }
        Err(join) => tracing::warn!(error = %join, "the model catalog cache could not be written"),
    }
}

/// The path of the cache file of the catalog of `settings`' provider in `state`, the
/// state root.
pub(crate) fn cache_path(settings: &Settings, state: &Path) -> PathBuf {
    state.join(Vendor::of(&settings.model.provider).cache_file())
}

/// The catalog that efrd starts with, for the provider of `settings`: the cache file
/// at `path` when it holds a list of that provider's backend that offers a model,
/// else the built-in table (OpenAI) or no list (Anthropic).
pub(crate) async fn load(settings: &Settings, path: &Path) -> ProviderCatalog {
    match Vendor::of(&settings.model.provider) {
        Vendor::OpenAi => {
            let catalog = load_openai(settings, path).await;
            ProviderCatalog::OpenAi(efr_provider_openai::ModelCatalog::new(catalog))
        }
        Vendor::Anthropic => {
            let catalog = match load_anthropic(settings, path).await {
                Some(catalog) => efr_provider_anthropic::ModelCatalog::with_catalog(catalog),
                None => efr_provider_anthropic::ModelCatalog::new(),
            };
            ProviderCatalog::Anthropic(catalog)
        }
    }
}

/// The OpenAI catalog of the cache file at `path`, else the built-in table.
async fn load_openai(settings: &Settings, path: &Path) -> efr_provider_openai::Catalog {
    use efr_provider_openai::Catalog;

    let backend = backend(&settings.model.provider);
    let base_url = openai_base_url(settings, backend).unwrap_or_default();
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

/// The Anthropic catalog of the cache file at `path`, or `None` when it holds no list
/// of the configured API that offers a model.
async fn load_anthropic(
    settings: &Settings,
    path: &Path,
) -> Option<efr_provider_anthropic::Catalog> {
    let base_url = settings
        .anthropic
        .base_url
        .clone()
        .unwrap_or_else(|| efr_provider_anthropic::API_BASE_URL.to_owned());
    let owned = path.to_path_buf();
    let read =
        tokio::task::spawn_blocking(move || efr_provider_anthropic::read_cache(&owned, &base_url))
            .await;
    match read {
        Ok(Ok(Some(catalog))) if !catalog.models().is_empty() => {
            tracing::info!(
                fetched_at = %catalog.fetched_at(),
                models = catalog.models().len(),
                "the model catalog comes from the cache until the API answers"
            );
            Some(catalog)
        }
        Ok(Ok(_)) => {
            tracing::info!("no model list yet; efrd fetches it before the first prompt");
            None
        }
        Ok(Err(error)) => {
            tracing::warn!(error = %efr_stdx::with_causes(&error), "the model catalog cache could not be used; efrd fetches the list before the first prompt");
            None
        }
        Err(join) => {
            tracing::warn!(error = %join, "the model catalog cache could not be read; efrd fetches the list before the first prompt");
            None
        }
    }
}

/// The base URL of the OpenAI `backend` in `settings`, or `None` when it is not one
/// (the config's checks refuse such a file before this runs).
fn openai_base_url(settings: &Settings, backend: efr_provider_openai::Backend) -> Option<String> {
    use efr_provider_openai::{Backend, OpenAiConfig};

    let (config, url) = match backend {
        Backend::Api => (OpenAiConfig::api(), settings.openai.api_base_url.as_deref()),
        _ => (OpenAiConfig::subscription(), settings.openai.subscription_base_url.as_deref()),
    };
    let config = match url {
        Some(url) => config.with_base_url(url).ok()?,
        None => config,
    };
    Some(config.base_url().to_owned())
}

/// The model of new conversations: `[model] name`, else the catalog's default, else
/// the first model of the config's list of the company, else the company's own default
/// (Claude Code's model for Anthropic, which has no list until a fetch works).
pub(crate) fn default_model(settings: &Settings, list: &ModelList) -> String {
    let vendor = list.vendor();
    settings
        .model
        .name
        .clone()
        .or_else(|| list.default_model().map(str::to_owned))
        .or_else(|| vendor.entries(settings).first().map(|model| model.id().to_owned()))
        .or_else(|| vendor.fallback_model().map(str::to_owned))
        .unwrap_or_default()
}

/// The effective model list with `settings` over `list`, as `models.list` answers it
/// and a turn checks against it, and the windows of the config's list that were cut
/// down to a model's largest one.
pub(crate) fn effective_models(
    settings: &Settings,
    list: &ModelList,
) -> (Vec<WireModel>, Vec<Clamp>) {
    let default = default_model(settings, list);
    let mut models: Vec<WireModel> = list
        .models()
        .iter()
        .map(|model| WireModel {
            default: model.id == default,
            id: model.id.clone(),
            efforts: model.efforts.clone(),
            default_effort: model.default_effort.clone(),
            source: ModelSource::Builtin,
            context_window: model.context_window,
            max_context_window: model.max_context_window,
            prefer_websockets: model.prefer_websockets,
        })
        .collect();
    let mut clamps = Vec::new();
    for entry in list.vendor().entries(settings) {
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
