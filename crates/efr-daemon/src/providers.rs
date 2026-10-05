//! The model providers: credential rows to token sources to providers, and the
//! subscription login.
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
//! The config picks the provider of new conversations; a [`ProviderFactory`] given to
//! the daemon replaces how it is built, which is how an in-process daemon answers from
//! a replay instead of the network. After a login the token source forgets its cached
//! token, so the running provider uses the new account at once.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use efr_config::{OpenAiSettings, Settings};
use efr_credentials::{CredentialId, CredentialRecord, SecretStore};
use efr_http::HttpClient;
use efr_oauth_openai::{OAuthConfig, OpenAiLogin, OpenAiTokenSource, PendingLogin};
use efr_protocol::ProviderStatus;
use efr_provider::{
    AccessToken, ModelInfo, Provider, ProviderError, ProviderId, StaticToken, TokenSource,
};
use efr_provider_openai::{OpenAiConfig, OpenAiProvider};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;

use crate::DaemonError;

/// The subscription provider and its credential.
pub const SUBSCRIPTION: &str = "openai-subscription";
/// The API key provider and its credential.
pub const API: &str = "openai-api";

/// Builds the provider that new conversations talk to.
pub trait ProviderFactory: Send + Sync + fmt::Debug {
    /// The provider named `id` (`openai-subscription` or `openai-api`).
    fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, DaemonError>;
}

/// The daemon's providers and the login.
#[derive(Debug)]
pub(crate) struct Providers {
    store: Arc<dyn SecretStore>,
    subscription: Arc<OpenAiTokenSource>,
    login: OpenAiLogin,
    active: Arc<dyn Provider>,
}

impl Providers {
    /// The providers of `config`, with credentials in `store`. `factory` replaces how
    /// the conversations' provider is built, and `issuer` the authorization server.
    pub(crate) fn build(
        config: &Settings,
        store: Arc<dyn SecretStore>,
        http: HttpClient,
        clock: Arc<dyn Clock>,
        rng: Arc<dyn Rng>,
        factory: Option<Arc<dyn ProviderFactory>>,
        issuer: Option<String>,
    ) -> Result<Self, DaemonError> {
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
        let factory: Arc<dyn ProviderFactory> = match factory {
            Some(factory) => factory,
            None => Arc::new(CredentialProviders {
                openai: config.openai.clone(),
                effort: config.model.effort.clone(),
                http,
                subscription: Arc::clone(&subscription),
                store: Arc::clone(&store),
                clock,
            }),
        };
        let active = factory.provider(&config.model.provider)?;
        let model = default_model(config);
        let known = known_models(config);
        let known: Vec<&str> = known.iter().map(String::as_str).collect();
        if let Some(name) = config.unknown_model(&known) {
            tracing::warn!(model = %name, known = ?known, "model.name is not in the model list, so the backend may refuse it");
        }
        tracing::info!(provider = %active.id(), model = %model, "provider ready");
        Ok(Providers { store, subscription, login, active })
    }

    /// The provider of new conversations.
    pub(crate) fn active(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.active)
    }

    /// Binds the login callback and returns the login to complete.
    pub(crate) async fn start_login(&self) -> Result<PendingLogin, DaemonError> {
        self.login.start().await.map_err(|source| DaemonError::Login { source })
    }

    /// Makes the subscription provider read the new login at its next request.
    pub(crate) fn login_completed(&self) {
        self.subscription.clear_cache();
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

/// The model of new conversations: the configured one, else the first of a configured
/// model list, else the subscription's default.
pub(crate) fn default_model(config: &Settings) -> String {
    config
        .model
        .name
        .clone()
        .or_else(|| config.openai.models.as_ref().and_then(|models| models.first().cloned()))
        .unwrap_or_else(|| efr_provider_openai::DEFAULT_SUBSCRIPTION_MODEL.to_owned())
}

/// The model ids the provider of `config` offers: `openai.models` when the config names
/// them, else the built-in list of that provider.
pub(crate) fn known_models(config: &Settings) -> Vec<String> {
    if let Some(models) = &config.openai.models {
        return models.clone();
    }
    let builtin = if config.model.provider == API {
        efr_provider_openai::api_models()
    } else {
        efr_provider_openai::subscription_models()
    };
    builtin.into_iter().map(|model| model.id).collect()
}

fn credential(id: &str) -> Result<CredentialId, DaemonError> {
    CredentialId::new(id).map_err(|source| DaemonError::Credentials { source })
}

/// The production factory: OpenAI providers over the stored credentials.
#[derive(Debug)]
struct CredentialProviders {
    openai: OpenAiSettings,
    effort: Option<String>,
    http: HttpClient,
    subscription: Arc<OpenAiTokenSource>,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
}

impl ProviderFactory for CredentialProviders {
    fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let provider_id = ProviderId::new(id).map_err(|source| DaemonError::Provider { source })?;
        let (config, tokens) = if id == API {
            let tokens: Arc<dyn TokenSource> =
                Arc::new(StoredApiKey { store: Arc::clone(&self.store), id: credential(API)? });
            let base_url = self.openai.api_base_url.as_deref();
            (
                openai_config(OpenAiConfig::api(), &self.openai, self.effort.as_deref(), base_url)?,
                tokens,
            )
        } else {
            let tokens: Arc<dyn TokenSource> = self.subscription.clone();
            let base_url = self.openai.subscription_base_url.as_deref();
            (
                openai_config(
                    OpenAiConfig::subscription(),
                    &self.openai,
                    self.effort.as_deref(),
                    base_url,
                )?,
                tokens,
            )
        };
        Ok(Arc::new(OpenAiProvider::new(
            provider_id,
            config,
            self.http.clone(),
            tokens,
            Arc::clone(&self.clock),
        )))
    }
}

/// `config` with the user's settings applied. `effort`, `[model] effort`, becomes the
/// provider's default reasoning effort.
pub(crate) fn openai_config(
    config: OpenAiConfig,
    settings: &OpenAiSettings,
    effort: Option<&str>,
    base_url: Option<&str>,
) -> Result<OpenAiConfig, DaemonError> {
    let invalid = |source| DaemonError::OpenAi { source };
    let mut config = config.with_originator(&settings.originator).map_err(invalid)?;
    if let Some(base_url) = base_url {
        config = config.with_base_url(base_url).map_err(invalid)?;
    }
    if let Some(models) = &settings.models {
        config = config.with_models(models.iter().map(ModelInfo::new).collect());
    }
    if let Some(effort) = effort {
        config = config.with_reasoning_effort(Some(effort.to_owned()));
    }
    Ok(config)
}

/// The API key of the `openai-api` credential, read at each request so a key saved
/// while the daemon runs is used without a restart.
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
}

#[cfg(test)]
mod tests;
