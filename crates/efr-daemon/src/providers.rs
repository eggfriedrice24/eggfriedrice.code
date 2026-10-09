//! The model providers: credential rows to token sources to providers, the model
//! catalog, and the subscription login.
//!
//! Two providers exist at milestone 1, both over `efr_provider_openai::OpenAiProvider`
//! and one shared `efr_http` client:
//!
//! - `openai-subscription`: the ChatGPT plan through `OpenAiTokenSource`, which reads
//!   the `openai-subscription` credential that `admin.login_openai` saves, refreshes it
//!   ahead of expiry and returns the access token with its ChatGPT account id;
//! - `openai-api`: the public API with the `openai-api` credential, an API key, read
//!   at each request and handed over through `StaticToken`.
//!
//! Any other id that the config accepts, such as `anthropic-api`, stops the start with
//! `DaemonError::UnknownProvider` until efrd can build it.
//!
//! The config picks the provider of new conversations; a [`ProviderFactory`] given to
//! the daemon replaces how it is built, which is how an in-process daemon answers from
//! a replay instead of the network. After a login the token source forgets its cached
//! token, so the running provider uses the new account at once, and the model catalog
//! is fetched again (`catalog.rs`). A daemon with a factory never fetches the catalog.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use efr_config::{OpenAiSettings, Settings, WebSocketChoice};
use efr_credentials::{CredentialId, CredentialRecord, SecretStore};
use efr_http::HttpClient;
use efr_oauth_openai::{OAuthConfig, OpenAiLogin, OpenAiTokenSource, PendingLogin};
use efr_protocol::ProviderStatus;
use efr_provider::{
    AccessToken, ModelInfo, Provider, ProviderError, ProviderId, StaticToken, TokenSource,
};
use efr_provider_openai::{
    Catalog, CatalogClient, ModelCatalog, OpenAiConfig, OpenAiProvider, WebSocketMode,
};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;

use crate::DaemonError;
use crate::catalog::Models;

/// The subscription provider and its credential.
pub const SUBSCRIPTION: &str = "openai-subscription";
/// The API key provider and its credential.
pub const API: &str = "openai-api";

/// Builds the provider that new conversations talk to.
pub trait ProviderFactory: Send + Sync + fmt::Debug {
    /// The provider named `id` (`openai-subscription` or `openai-api`).
    fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, DaemonError>;
}

/// The daemon's providers, the model catalog and the login.
#[derive(Debug)]
pub(crate) struct Providers {
    store: Arc<dyn SecretStore>,
    subscription: Arc<OpenAiTokenSource>,
    login: OpenAiLogin,
    active: Arc<dyn Provider>,
    models: Arc<Models>,
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
    /// The catalog to start with, from the cache or built in.
    pub(crate) catalog: Catalog,
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
            rng,
        );
        let catalog = ModelCatalog::new(catalog);
        let fetches = factory.is_none() && config.model.provider != API;
        let models = if fetches {
            let base_url = config.openai.subscription_base_url.as_deref();
            let openai = openai_config(OpenAiConfig::subscription(), &config.openai, base_url)?;
            let client = CatalogClient::new(
                openai,
                http.clone(),
                Arc::clone(&subscription) as Arc<dyn TokenSource>,
                Arc::clone(&clock),
            );
            Models::fetched(catalog.clone(), client, catalog_cache, Arc::clone(&clock))
        } else {
            Models::fixed(catalog.clone())
        };
        let factory: Arc<dyn ProviderFactory> = match factory {
            Some(factory) => factory,
            None => Arc::new(CredentialProviders {
                openai: config.openai.clone(),
                http,
                subscription: Arc::clone(&subscription),
                store: Arc::clone(&store),
                clock,
                catalog,
            }),
        };
        let active = factory.provider(&config.model.provider)?;
        let models = Arc::new(models);
        let model = models.default_model(config);
        warn_unfit_defaults(config, &models);
        tracing::info!(provider = %active.id(), model = %model, fetches_catalog = models.fetches(), "provider ready");
        Ok(Providers { store, subscription, login, active, models })
    }

    /// The provider of new conversations.
    pub(crate) fn active(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.active)
    }

    /// The model catalog and its fetch.
    pub(crate) fn models(&self) -> Arc<Models> {
        Arc::clone(&self.models)
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

    /// Whether each provider has credentials, for `admin.status`.
    pub(crate) async fn status(&self) -> Vec<ProviderStatus> {
        let store = Arc::clone(&self.store);
        let loaded = tokio::task::spawn_blocking(move || {
            [SUBSCRIPTION, API].map(|id| {
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
        loaded.into_iter().map(|(id, record)| provider_status(id, record.as_ref())).collect()
    }
}

/// The status of provider `id` with the credential `record`.
pub(crate) fn provider_status(id: &str, record: Option<&CredentialRecord>) -> ProviderStatus {
    let expires_at = match record {
        Some(CredentialRecord::OAuth(tokens)) => tokens.expires_at,
        _ => None,
    };
    ProviderStatus { provider: id.to_owned(), logged_in: record.is_some(), expires_at }
}

/// Warns when the config's default model is not in the model list, or its default
/// effort is not one the default model takes: every prompt that leaves them to the
/// config then fails until the file is fixed.
fn warn_unfit_defaults(config: &Settings, models: &Models) {
    let list = models.effective(config);
    if list.is_empty() {
        return;
    }
    let known: Vec<&str> = list.iter().map(|model| model.id.as_str()).collect();
    if let Some(name) = config.unknown_model(&known) {
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

/// The production factory: OpenAI providers over the stored credentials, reading their
/// models from the shared catalog.
#[derive(Debug)]
struct CredentialProviders {
    openai: OpenAiSettings,
    http: HttpClient,
    subscription: Arc<OpenAiTokenSource>,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    catalog: ModelCatalog,
}

impl ProviderFactory for CredentialProviders {
    fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let provider_id = ProviderId::new(id).map_err(|source| DaemonError::Provider { source })?;
        let (config, tokens) = match id {
            API => {
                let tokens: Arc<dyn TokenSource> =
                    Arc::new(StoredApiKey { store: Arc::clone(&self.store), id: credential(API)? });
                let base_url = self.openai.api_base_url.as_deref();
                (openai_config(OpenAiConfig::api(), &self.openai, base_url)?, tokens)
            }
            SUBSCRIPTION => {
                let tokens: Arc<dyn TokenSource> = self.subscription.clone();
                let base_url = self.openai.subscription_base_url.as_deref();
                (openai_config(OpenAiConfig::subscription(), &self.openai, base_url)?, tokens)
            }
            // NOTE: a provider id that the config accepts but efrd cannot build yet,
            // such as `anthropic-api`, stops the start; an OpenAI provider under its
            // name would send the user's prompts to the wrong company.
            _ => return Err(DaemonError::UnknownProvider { id: id.to_owned() }),
        };
        let provider = OpenAiProvider::new(
            provider_id,
            config,
            self.http.clone(),
            tokens,
            Arc::clone(&self.clock),
        )
        .with_catalog(self.catalog.clone());
        Ok(Arc::new(provider))
    }
}

/// `config` with the user's settings applied: the originator, the base URL, the
/// transport of `[openai] websocket` and the models of `[openai] models`, laid over the
/// catalog with the limits that an entry gives.
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

/// The provider's transport for the choice of `[openai] websocket`.
fn websocket_mode(choice: WebSocketChoice) -> WebSocketMode {
    match choice {
        WebSocketChoice::On => WebSocketMode::On,
        WebSocketChoice::Off => WebSocketMode::Off,
        _ => WebSocketMode::Auto,
    }
}

/// The API key of the `openai-api` credential, read at each request so a key saved
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
