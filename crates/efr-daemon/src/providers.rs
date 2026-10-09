//! The model providers: credential rows to token sources to providers, the model
//! catalog, and the logins.
//!
//! Three providers run, over one shared `efr_http` client:
//!
//! - `openai-subscription`: the ChatGPT plan through `OpenAiTokenSource`, which reads
//!   the `openai-subscription` credential that `admin.login_openai` saves, refreshes it
//!   ahead of expiry and returns the access token with its ChatGPT account id;
//! - `openai-api`: the public OpenAI API with the `openai-api` credential, an API key,
//!   read at each request and handed over through `StaticToken`;
//! - `anthropic-api`: Anthropic's Messages API (`efr_provider_anthropic`) with the
//!   `anthropic-api` credential, an API key read the same way, and the settings of
//!   `[anthropic]`.
//!
//! `admin.login_api_key` takes a key for `openai-api` or `anthropic-api`, checks it
//! with its provider (`api_key.rs`) unless the client says not to, and stores it under
//! the provider's id. `admin.logout` deletes a provider's credential.
//!
//! When the provider of new conversations uses a stored key, its model calls and the
//! fetches of its model list report a 401 to [`KeyRefusals`] (`refusals.rs`), and
//! `admin.status` shows when the key was refused until a request with it works again or
//! a login or a logout replaces it.
//!
//! The config picks the provider of new conversations; a [`ProviderFactory`] given to
//! the daemon replaces how it is built, which is how an in-process daemon answers from
//! a replay instead of the network. The provider and the fetch of the model catalog
//! (`catalog.rs`) share one catalog of the provider's company. After a login the token
//! source forgets its cached token, so the running provider uses the new account at
//! once, and the model catalog is fetched again. A daemon with a factory never fetches
//! the catalog.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use efr_config::{AnthropicSettings, CacheTtlChoice, OpenAiSettings, Settings, WebSocketChoice};
use efr_credentials::{CredentialId, CredentialRecord, SecretStore};
use efr_http::HttpClient;
use efr_oauth_openai::{OAuthConfig, OpenAiLogin, OpenAiTokenSource, PendingLogin};
use efr_protocol::{LoginKind, ProviderStatus, SecretText};
use efr_provider::{
    AccessToken, ExposeSecret as _, ModelInfo, Provider, ProviderError, ProviderId, SecretString,
    StaticToken, TokenSource,
};
use efr_provider_anthropic::{AnthropicConfig, AnthropicProvider, CacheTtl};
use efr_provider_openai::{OpenAiConfig, OpenAiProvider, WebSocketMode};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;

use crate::DaemonError;
use crate::catalog::{Fetcher, Models, NoList, ProviderCatalog, backend};

mod api_key;
mod refusals;

use self::api_key::{KEY_PROVIDERS, KeyChecks};
use self::refusals::Watched;
pub(crate) use self::refusals::{KeyRefusals, KeySource};

/// The subscription provider and its credential.
pub const SUBSCRIPTION: &str = "openai-subscription";
/// The API key provider and its credential.
pub const API: &str = "openai-api";
/// The Anthropic provider and its credential, an API key.
pub const ANTHROPIC: &str = "anthropic-api";

/// Every provider that a login can name, in the order that `admin.status` lists them.
const PROVIDERS: [&str; 3] = [SUBSCRIPTION, API, ANTHROPIC];

/// Builds the provider that new conversations talk to.
pub trait ProviderFactory: Send + Sync + fmt::Debug {
    /// The provider named `id` (`openai-subscription`, `openai-api` or
    /// `anthropic-api`).
    fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, DaemonError>;
}

/// The daemon's providers, the model catalog and the login.
#[derive(Debug)]
pub(crate) struct Providers {
    store: Arc<dyn SecretStore>,
    subscription: Arc<OpenAiTokenSource>,
    login: OpenAiLogin,
    checks: KeyChecks,
    /// The provider of new conversations, as `[model] provider` names it.
    configured: String,
    active: Arc<dyn Provider>,
    models: Arc<Models>,
    refusals: Arc<KeyRefusals>,
}

/// What a login with an API key stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyLogin {
    /// What a client may show of the key.
    pub(crate) hint: String,
    /// True when the provider of the key is the one of new conversations.
    pub(crate) active: bool,
}

/// What [`Providers::build`] takes besides the config and the store.
#[derive(Debug)]
pub(crate) struct ProviderParts {
    pub(crate) http: HttpClient,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) rng: Arc<dyn Rng>,
    /// Replaces how the conversations' provider is built; the catalog is then never
    /// fetched.
    pub(crate) factory: Option<Arc<dyn ProviderFactory>>,
    /// Replaces the authorization server.
    pub(crate) issuer: Option<String>,
    /// The catalog of `[model] provider` to start with: from the cache, built in, or
    /// without a list.
    pub(crate) catalog: ProviderCatalog,
    /// The cache file of the catalog.
    pub(crate) catalog_cache: PathBuf,
}

impl Providers {
    /// The providers of `config`, with credentials in `store`.
    pub(crate) fn build(
        config: &Settings,
        store: Arc<dyn SecretStore>,
        parts: ProviderParts,
    ) -> Result<Self, DaemonError> {
        let ProviderParts { http, clock, rng, factory, issuer, catalog, catalog_cache } = parts;
        let mut oauth = OAuthConfig::default();
        oauth.originator.clone_from(&config.openai.originator);
        if let Some(issuer) = issuer {
            oauth.issuer = issuer;
        }
        let subscription = Arc::new(OpenAiTokenSource::new(
            oauth.clone(),
            http.clone(),
            Arc::clone(&store),
            credential(SUBSCRIPTION)?,
            Arc::clone(&clock),
        ));
        let login = OpenAiLogin::new(
            oauth,
            http.clone(),
            Arc::clone(&store),
            credential(SUBSCRIPTION)?,
            Arc::clone(&clock),
            Arc::clone(&rng),
        );
        let api_base_url = config.openai.api_base_url.as_deref();
        let api = openai_config(OpenAiConfig::api(), &config.openai, api_base_url)?;
        let anthropic = anthropic_config(&config.anthropic)?;
        let checks = KeyChecks::new(http.clone(), api.clone(), anthropic.clone());
        let refusals = Arc::new(KeyRefusals::new(Arc::clone(&clock)));
        let provider = config.model.provider.as_str();
        let fetcher = match (provider, &catalog) {
            _ if factory.is_some() => None,
            (SUBSCRIPTION | API, ProviderCatalog::OpenAi(shared)) => {
                let (openai, tokens) = if provider == API {
                    (api, stored_key(&store, API)?)
                } else {
                    let base_url = config.openai.subscription_base_url.as_deref();
                    let openai =
                        openai_config(OpenAiConfig::subscription(), &config.openai, base_url)?;
                    (openai, Arc::clone(&subscription) as Arc<dyn TokenSource>)
                };
                let client = efr_provider_openai::CatalogClient::new(
                    openai,
                    http.clone(),
                    tokens,
                    Arc::clone(&clock),
                );
                Some(Fetcher::OpenAi(shared.clone(), client))
            }
            (ANTHROPIC, ProviderCatalog::Anthropic(shared)) => {
                let client = efr_provider_anthropic::CatalogClient::new(
                    anthropic.clone(),
                    http.clone(),
                    stored_key(&store, ANTHROPIC)?,
                    Arc::clone(&clock),
                    Arc::clone(&rng),
                );
                Some(Fetcher::Anthropic(shared.clone(), client))
            }
            _ => None,
        };
        let uses_key = KEY_PROVIDERS.contains(&provider);
        let models = match fetcher {
            Some(fetcher) => {
                let models = Models::fetched(provider, fetcher, catalog_cache, Arc::clone(&clock));
                if uses_key { models.with_refusals(Arc::clone(&refusals)) } else { models }
            }
            None => Models::fixed(provider, catalog.clone()),
        };
        let factory: Arc<dyn ProviderFactory> = match factory {
            Some(factory) => factory,
            None => Arc::new(CredentialProviders {
                openai: config.openai.clone(),
                anthropic,
                http,
                subscription: Arc::clone(&subscription),
                store: Arc::clone(&store),
                clock,
                rng,
                catalog,
            }),
        };
        let mut active = factory.provider(&config.model.provider)?;
        if uses_key {
            active = Arc::new(Watched::new(active, provider, Arc::clone(&refusals)));
        }
        let models = Arc::new(models);
        let model = models.default_model(config);
        warn_unfit_defaults(config, &models);
        tracing::info!(provider = %active.id(), model = %model, fetches_catalog = models.fetches(), "provider ready");
        let configured = config.model.provider.clone();
        Ok(Providers { store, subscription, login, checks, configured, active, models, refusals })
    }

    /// The provider of new conversations.
    pub(crate) fn active(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.active)
    }

    /// The model catalog and its fetch.
    pub(crate) fn models(&self) -> Arc<Models> {
        Arc::clone(&self.models)
    }

    /// True when `provider` is the provider of new conversations, `[model] provider`.
    pub(crate) fn is_active(&self, provider: &str) -> bool {
        provider == self.configured
    }

    /// Refuses a prompt whose turn cannot run because the catalog still has no list
    /// after [`Models::ready`], with the cause: no key is stored, or the fetch failed
    /// or gave no list. A turn on a model whose output limit the config gives (in the
    /// config's list of models or as `[model] max_output_tokens`) runs without a list,
    /// unless no key is stored. `asked` is the prompt's own model.
    pub(crate) fn check_list(
        &self,
        settings: &Settings,
        asked: Option<&str>,
    ) -> Result<(), DaemonError> {
        let Some(missing) = self.models.missing() else {
            return Ok(());
        };
        if !matches!(missing, NoList::NotLoggedIn) {
            let model = asked.map_or_else(|| self.models.default_model(settings), str::to_owned);
            let limited = self
                .active
                .models()
                .iter()
                .any(|info| info.id == model && info.max_output_tokens.is_some());
            if limited || settings.model.max_output_tokens.is_some() {
                return Ok(());
            }
        }
        let list = self.models.list().vendor().list_name();
        Err(match missing {
            NoList::NotLoggedIn => DaemonError::NoModelListWithoutKey {
                provider: self.configured.clone(),
                login: login_command(&self.configured),
            },
            NoList::Failed(source) => DaemonError::ModelListFetch { list, source },
            NoList::NoActiveModel => {
                DaemonError::NoModelList { list, reason: "the list has no active model" }
            }
            NoList::NotFetched => {
                DaemonError::NoModelList { list, reason: "the fetch did not end in time" }
            }
        })
    }

    /// Binds the login callback and returns the login to complete.
    pub(crate) async fn start_login(&self) -> Result<PendingLogin, DaemonError> {
        self.login.start().await.map_err(|source| DaemonError::Login { source })
    }

    /// Makes the subscription provider read the new login at its next request, and
    /// fetches the model catalog of the new account.
    pub(crate) fn login_completed(&self) {
        self.subscription.clear_cache();
        self.models.refresh_now();
    }

    /// Checks `key` with `provider` unless `check` is false, and stores it as the
    /// provider's credential. The key reaches no error and no log line. The catalog of
    /// the active provider is fetched again, so a new key's list applies.
    pub(crate) async fn login_api_key(
        &self,
        provider: &str,
        key: &SecretText,
        check: bool,
    ) -> Result<KeyLogin, DaemonError> {
        if !PROVIDERS.contains(&provider) {
            return Err(DaemonError::NoSuchProvider { provider: provider.to_owned() });
        }
        if !KEY_PROVIDERS.contains(&provider) {
            return Err(DaemonError::NoApiKeyLogin { provider: provider.to_owned() });
        }
        api_key::validate(provider, key.expose_secret())?;
        let key = SecretString::from(key.expose_secret());
        if check {
            self.checks.check(provider, &key).await.map_err(|source| DaemonError::KeyCheck {
                provider: provider.to_owned(),
                source,
            })?;
        }
        let hint = api_key::hint(key.expose_secret());
        let id = credential(provider)?;
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || store.save(&id, &CredentialRecord::ApiKey { key }))
            .await
            .map_err(|_| DaemonError::TaskPanicked { task: "store an API key" })?
            .map_err(|source| DaemonError::Credentials { source })?;
        self.refusals.reset(provider);
        let active = provider == self.configured;
        if active {
            self.models.refresh_now();
        }
        Ok(KeyLogin { hint, active })
    }

    /// Deletes the credential of `provider`. False when none was stored.
    pub(crate) async fn logout(&self, provider: &str) -> Result<bool, DaemonError> {
        if !PROVIDERS.contains(&provider) {
            return Err(DaemonError::NoSuchProvider { provider: provider.to_owned() });
        }
        let id = credential(provider)?;
        let store = Arc::clone(&self.store);
        let deleted = tokio::task::spawn_blocking(move || store.delete(&id))
            .await
            .map_err(|_| DaemonError::TaskPanicked { task: "delete a credential" })?
            .map_err(|source| DaemonError::Credentials { source })?;
        if provider == SUBSCRIPTION {
            // The running provider must not keep the token of the deleted login.
            self.subscription.clear_cache();
        }
        if deleted {
            self.refusals.reset(provider);
        }
        Ok(deleted)
    }

    /// Whether each provider has credentials, for `admin.status`.
    pub(crate) async fn status(&self) -> Vec<ProviderStatus> {
        let store = Arc::clone(&self.store);
        let loaded = tokio::task::spawn_blocking(move || {
            PROVIDERS.map(|id| {
                let record = CredentialId::new(id).ok().and_then(|id| match store.load(&id) {
                    Ok(record) => record,
                    Err(error) => {
                        tracing::warn!(error = %error, credential = %id, "a credential could not be read");
                        None
                    }
                });
                (id, record)
            })
        })
        .await;
        let Ok(loaded) = loaded else {
            return Vec::new();
        };
        loaded
            .into_iter()
            .map(|(id, record)| {
                let mut status = provider_status(id, record.as_ref(), id == self.configured);
                if status.login == Some(LoginKind::ApiKey) {
                    status.key_refused_at = self.refusals.refused_at(id);
                }
                status
            })
            .collect()
    }
}

/// The status of provider `id` with the credential `record`; `active` when it is the
/// provider of new conversations.
pub(crate) fn provider_status(
    id: &str,
    record: Option<&CredentialRecord>,
    active: bool,
) -> ProviderStatus {
    let (login, expires_at, key_hint) = match record {
        Some(CredentialRecord::OAuth(tokens)) => {
            (Some(LoginKind::Subscription), tokens.expires_at, None)
        }
        Some(CredentialRecord::ApiKey { key }) => {
            (Some(LoginKind::ApiKey), None, Some(api_key::hint(key.expose_secret())))
        }
        _ => (None, None, None),
    };
    ProviderStatus {
        provider: id.to_owned(),
        logged_in: record.is_some(),
        expires_at,
        active,
        login,
        key_hint,
        key_refused_at: None,
    }
}

/// The command that logs in to `provider`.
pub(crate) fn login_command(provider: &str) -> &'static str {
    match provider {
        ANTHROPIC => "efr login anthropic",
        API => "efr login openai-api",
        _ => "efr login openai",
    }
}

/// Warns when `[model] name` is a model of another company than `[model] provider`
/// (`efr_config::ForeignModel`): every turn that does not name its own model then
/// fails until the file is fixed. efrd calls it at start and after each reload. True
/// when it warned.
pub(crate) fn warn_foreign_model(config: &Settings) -> bool {
    let Some(foreign) = config.foreign_model() else {
        return false;
    };
    tracing::warn!(
        model = %foreign.name,
        company = foreign.company.name(),
        provider = %foreign.provider,
        fix = %foreign.fix(),
        "model.name is a model of another company than model.provider, so a turn that does not name its own model fails"
    );
    true
}

/// Warns when the config's default model belongs to another company than the
/// provider, is not in the model list, or its default effort is not one the default
/// model takes: every prompt that leaves them to the config then fails until the file
/// is fixed.
fn warn_unfit_defaults(config: &Settings, models: &Models) {
    let foreign = warn_foreign_model(config);
    let list = models.effective(config);
    if list.is_empty() {
        return;
    }
    let known: Vec<&str> = list.iter().map(|model| model.id.as_str()).collect();
    if let Some(name) = config.unknown_model(&known).filter(|_| !foreign) {
        tracing::warn!(model = %name, known = ?known, "model.name is not in the model list, so a turn that does not name its own model fails");
    }
    let default = models.default_model(config);
    let efforts = list.iter().find(|model| model.id == default).map(|model| &model.efforts);
    if let (Some(effort), Some(efforts)) = (&config.model.effort, efforts)
        && !efforts.is_empty()
        && !efforts.contains(effort)
    {
        tracing::warn!(effort = %effort, model = %default, efforts = ?efforts, "model.effort is not an effort of the default model, so a turn on it that does not name its own effort fails");
    }
}

fn credential(id: &str) -> Result<CredentialId, DaemonError> {
    CredentialId::new(id).map_err(|source| DaemonError::Credentials { source })
}

/// The API key that is stored under the credential `id`, read at each request.
fn stored_key(store: &Arc<dyn SecretStore>, id: &str) -> Result<Arc<dyn TokenSource>, DaemonError> {
    Ok(Arc::new(StoredApiKey { store: Arc::clone(store), id: credential(id)? }))
}

/// The production factory: the providers over the stored credentials, reading their
/// models from the shared catalog.
#[derive(Debug)]
struct CredentialProviders {
    openai: OpenAiSettings,
    anthropic: AnthropicConfig,
    http: HttpClient,
    subscription: Arc<OpenAiTokenSource>,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    catalog: ProviderCatalog,
}

impl ProviderFactory for CredentialProviders {
    fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let provider_id = ProviderId::new(id).map_err(|source| DaemonError::Provider { source })?;
        let (config, tokens) = match id {
            API => {
                let tokens = stored_key(&self.store, API)?;
                let base_url = self.openai.api_base_url.as_deref();
                (openai_config(OpenAiConfig::api(), &self.openai, base_url)?, tokens)
            }
            SUBSCRIPTION => {
                let tokens: Arc<dyn TokenSource> = self.subscription.clone();
                let base_url = self.openai.subscription_base_url.as_deref();
                (openai_config(OpenAiConfig::subscription(), &self.openai, base_url)?, tokens)
            }
            ANTHROPIC => {
                let provider = AnthropicProvider::new(
                    provider_id,
                    self.anthropic.clone(),
                    self.http.clone(),
                    stored_key(&self.store, ANTHROPIC)?,
                    Arc::clone(&self.clock),
                    Arc::clone(&self.rng),
                )
                .with_catalog(self.catalog.anthropic());
                return Ok(Arc::new(provider));
            }
            // NOTE: the config's checks refuse any other id; an OpenAI provider under
            // an unknown name would send the user's prompts to the wrong company.
            _ => return Err(DaemonError::UnknownProvider { id: id.to_owned() }),
        };
        let provider = OpenAiProvider::new(
            provider_id,
            config,
            self.http.clone(),
            tokens,
            Arc::clone(&self.clock),
            Arc::clone(&self.rng),
        )
        .with_catalog(self.catalog.openai(backend(id)));
        Ok(Arc::new(provider))
    }
}

/// `config` with the user's settings applied: the originator, the base URL, the
/// organization and the project of the API path, the transport of `[openai] websocket`
/// and the models of `[openai] models`, laid over the catalog with the limits that an
/// entry gives.
///
/// The reasoning effort is not set here: each turn sends its own as the request's
/// `effort`, so a change of `[model] effort` reaches the next turn without a
/// restart, and a turn without one leaves it to the backend.
pub(crate) fn openai_config(
    config: OpenAiConfig,
    settings: &OpenAiSettings,
    base_url: Option<&str>,
) -> Result<OpenAiConfig, DaemonError> {
    let invalid = |source| DaemonError::OpenAi { source };
    let mut config = config
        .with_originator(&settings.originator)
        .map_err(invalid)?
        .with_websocket(websocket_mode(settings.websocket));
    if let Some(base_url) = base_url {
        config = config.with_base_url(base_url).map_err(invalid)?;
    }
    if let Some(organization) = &settings.organization {
        config = config.with_organization(organization).map_err(invalid)?;
    }
    if let Some(project) = &settings.project {
        config = config.with_project(project).map_err(invalid)?;
    }
    if let Some(extra) = &settings.models {
        let models = extra
            .iter()
            .map(|entry| {
                let mut model = ModelInfo::new(entry.id());
                model.context_window = entry.context_window();
                model.max_output_tokens = entry.max_output_tokens();
                model
            })
            .collect();
        config = config.with_models(models);
    }
    Ok(config)
}

/// The Anthropic config of `[anthropic]`: the base URL, the workspace, the time to
/// live of the prompt cache and the models of `[anthropic] models` with the limits
/// that an entry gives. The key check uses the same config.
///
/// The effort is not set here, as for OpenAI: each turn sends its own as the request's
/// `effort`, and the provider sends its default for a turn without one.
pub(crate) fn anthropic_config(
    settings: &AnthropicSettings,
) -> Result<AnthropicConfig, DaemonError> {
    let invalid = |source| DaemonError::Anthropic { source };
    let mut config = AnthropicConfig::new().with_cache_ttl(cache_ttl(settings.cache_ttl));
    if let Some(base_url) = &settings.base_url {
        config = config.with_base_url(base_url).map_err(invalid)?;
    }
    if let Some(workspace) = &settings.workspace_id {
        config = config.with_workspace_id(workspace).map_err(invalid)?;
    }
    if let Some(extra) = &settings.models {
        let models = extra
            .iter()
            .map(|entry| {
                let mut model = ModelInfo::new(entry.id());
                model.context_window = entry.context_window();
                model.max_output_tokens = entry.max_output_tokens();
                model
            })
            .collect();
        config = config.with_models(models);
    }
    Ok(config)
}

/// The provider's prompt cache times for the choice of `[anthropic] cache_ttl`.
fn cache_ttl(choice: CacheTtlChoice) -> CacheTtl {
    match choice {
        CacheTtlChoice::FiveMinutes => CacheTtl::FiveMinutes,
        CacheTtlChoice::OneHour => CacheTtl::OneHour,
        _ => CacheTtl::Auto,
    }
}

/// The provider's transport for the choice of `[openai] websocket`.
fn websocket_mode(choice: WebSocketChoice) -> WebSocketMode {
    match choice {
        WebSocketChoice::On => WebSocketMode::On,
        WebSocketChoice::Off => WebSocketMode::Off,
        _ => WebSocketMode::Auto,
    }
}

/// The API key of a credential (`openai-api` or `anthropic-api`), read at each request so a key saved
/// while the daemon runs is used without a restart. A key cannot refresh, so a provider
/// fails at its first 401 and does not send the same key again.
#[derive(Debug)]
struct StoredApiKey {
    store: Arc<dyn SecretStore>,
    id: CredentialId,
}

#[async_trait]
impl TokenSource for StoredApiKey {
    async fn access_token(&self) -> Result<AccessToken, ProviderError> {
        let store = Arc::clone(&self.store);
        let id = self.id.clone();
        let record = tokio::task::spawn_blocking(move || store.load(&id))
            .await
            .map_err(|join| ProviderError::Token { source: Box::new(join) })?
            .map_err(|source| ProviderError::Token { source: Box::new(source) })?;
        match record {
            Some(CredentialRecord::ApiKey { key }) => StaticToken::new(key).access_token().await,
            _ => Err(ProviderError::NotLoggedIn),
        }
    }

    async fn invalidate(&self) {}

    fn refreshable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;
