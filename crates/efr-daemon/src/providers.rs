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
//! - `anthropic-api`: Anthropic's Messages API (`efr_providers::anthropic`) with the
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
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use async_trait::async_trait;
use efr_config::{
    AnthropicSettings, CacheTtlChoice, ModelCompany, OpenAiSettings, Settings, WebSocketChoice,
};
use efr_credentials::{CredentialId, CredentialRecord, SecretStore};
use efr_http::HttpClient;
use efr_oauth_openai::{OAuthConfig, OpenAiLogin, OpenAiTokenSource, PendingLogin};
use efr_protocol::{LoginKind, ProviderStatus, SecretText};
use efr_provider::{
    AccessToken, ExposeSecret as _, ModelInfo, Provider, ProviderError, ProviderId, SecretString,
    StaticToken, TokenSource,
};
use efr_providers::anthropic::{AnthropicConfig, AnthropicProvider, CacheTtl};
use efr_providers::openai::{OpenAiConfig, OpenAiProvider, WebSocketMode};
use efr_stdx::rng::Rng;
use efr_stdx::time::Clock;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::catalog::{self, Fetcher, Models, NoList, ProviderCatalog, backend};
use crate::catalogs::Catalogs;

mod api_key;
mod refusals;
mod routes;

use self::api_key::{KeyChecks, takes_key};
use self::refusals::Watched;
pub(crate) use self::refusals::{KeyRefusals, KeySource};
pub(crate) use self::routes::{Routes, SharedRoutes};

/// The subscription provider and its credential.
pub const SUBSCRIPTION: &str = "openai-subscription";
/// The API key provider and its credential.
pub const API: &str = "openai-api";
/// The Anthropic provider and its credential, an API key.
pub const ANTHROPIC: &str = "anthropic-api";

/// Every provider that a login can name, in the order that `admin.status` lists them.
pub(crate) const PROVIDERS: [&str; 3] = [SUBSCRIPTION, API, ANTHROPIC];

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
    /// The provider of new conversations at start, as `[model] provider` named it.
    configured: String,
    /// The latest settings, whose `[model] provider` is the provider of new
    /// conversations from then on; `None` until efrd hands them over
    /// ([`with_settings`](Self::with_settings)).
    settings: Option<watch::Receiver<Arc<Settings>>>,
    /// The provider of `configured`, under its id.
    active: (String, Arc<dyn Provider>),
    /// The other providers that efrd runs, each under its id, shared with the router
    /// ([`Routes`]), so an account that a login adds runs at once. A daemon whose
    /// provider a test injects runs no other.
    others: SharedRoutes,
    /// Builds the provider and the catalog fetch of an account that a login adds;
    /// `None` in a daemon whose provider a test injects.
    credential: Option<Arc<CredentialProviders>>,
    /// The state root, where each account keeps the cache file of its catalog.
    state: PathBuf,
    /// Ends the catalog fetch of an account that a login adds, with efrd; empty until
    /// efrd starts the fetches ([`serve`](Self::serve)).
    stop: OnceLock<CancellationToken>,
    /// The catalog of each provider, the one of `configured` first, and the logins.
    catalogs: Arc<Catalogs>,
    refusals: Arc<KeyRefusals>,
}

/// The start of the catalog of one provider: its list and its cache file.
#[derive(Debug)]
pub(crate) struct CatalogStart {
    /// The provider id, such as `anthropic-api` or the account `anthropic-api.work`.
    pub(crate) provider: String,
    /// From the cache, built in, or without a list.
    pub(crate) catalog: ProviderCatalog,
    pub(crate) cache: PathBuf,
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
    /// The catalog of each provider to start with ([`catalog_starts`]); the one of
    /// `[model] provider` must be among them.
    pub(crate) catalogs: Vec<CatalogStart>,
    /// The state root, for the cache file of an account that a login adds.
    pub(crate) state: PathBuf,
}

impl Providers {
    /// The providers of `config`, with credentials in `store`.
    pub(crate) fn build(
        config: &Settings,
        store: Arc<dyn SecretStore>,
        parts: ProviderParts,
    ) -> Result<Self, DaemonError> {
        let ProviderParts { http, clock, rng, factory, issuer, catalogs, state } = parts;
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
        let oauth_config = oauth.clone();
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
        let injected = factory.is_some();
        let credential = Arc::new(CredentialProviders {
            openai: config.openai.clone(),
            anthropic: anthropic.clone(),
            http: http.clone(),
            subscription: Arc::clone(&subscription),
            oauth: oauth_config.clone(),
            subscriptions: RwLock::new(Vec::new()),
            store: Arc::clone(&store),
            clock: Arc::clone(&clock),
            rng: Arc::clone(&rng),
            catalogs: RwLock::new(Vec::new()),
        });
        let mut configured_models = None;
        let mut other_models = Vec::new();
        for CatalogStart { provider: id, catalog, cache } in catalogs {
            if id != provider && injected {
                // NOTE: a test's provider serves only the list of `[model] provider`.
                continue;
            }
            credential.add_catalog(&id, catalog.clone());
            let id = id.as_str();
            let fetcher = match (efr_config::provider_kind(id), &catalog) {
                _ if injected => None,
                _ => credential.fetcher(id, &catalog)?,
            };
            let models = match fetcher {
                Some(fetcher) => {
                    let models = Models::fetched(id, fetcher, cache, Arc::clone(&clock));
                    if takes_key(id) { models.with_refusals(Arc::clone(&refusals)) } else { models }
                }
                None => Models::fixed(id, catalog),
            };
            if id == provider {
                configured_models = Some(Arc::new(models));
            } else {
                other_models.push(Arc::new(models));
            }
        }
        let Some(models) = configured_models else {
            return Err(DaemonError::UnknownProvider { id: provider.to_owned() });
        };
        let catalogs = Arc::new(Catalogs::new(Arc::clone(&models), other_models));
        let factory: Arc<dyn ProviderFactory> = match factory {
            Some(factory) => factory,
            None => Arc::clone(&credential) as Arc<dyn ProviderFactory>,
        };
        // NOTE: every provider with a catalog runs, also one without a login: its
        // credential is read at each request, so a later login serves it at once.
        let route = |id: &str| -> Result<(String, Arc<dyn Provider>), DaemonError> {
            let mut provider = factory.provider(id)?;
            if takes_key(id) {
                provider = Arc::new(Watched::new(provider, id, Arc::clone(&refusals)));
            }
            Ok((id.to_owned(), provider))
        };
        let active = route(provider)?;
        // NOTE: another provider that cannot be built, such as one with a bad value in
        // `[openai]`, does not stop efrd; its models then go to no provider.
        let others = catalogs
            .all()
            .iter()
            .map(|models| models.provider())
            .filter(|id| *id != provider)
            .filter_map(|id| match route(id) {
                Ok(route) => Some(route),
                Err(error) => {
                    tracing::warn!(provider = %id, error = %error, "a provider could not be built; a turn on its models fails");
                    None
                }
            })
            .collect::<Vec<_>>();
        let model = models.default_model(config);
        warn_unfit_defaults(config, &models);
        tracing::info!(provider = %active.1.id(), model = %model, fetches_catalog = models.fetches(), "provider ready");
        let configured = config.model.provider.clone();
        Ok(Providers {
            store,
            subscription,
            login,
            checks,
            configured,
            settings: None,
            active,
            others: Arc::new(RwLock::new(others)),
            credential: (!injected).then_some(credential),
            state,
            stop: OnceLock::new(),
            catalogs,
            refusals,
        })
    }

    /// Runs the catalog fetch of each provider, and of each account that a login adds
    /// later, until `stop`.
    pub(crate) async fn serve(&self, stop: CancellationToken) {
        let _ = self.stop.set(stop.clone());
        let serving = self.catalogs.all().into_iter().map(|models| models.serve(stop.clone()));
        futures::future::join_all(serving).await;
    }

    /// Runs the account `id`, such as `anthropic-api.work`, after a login stored its
    /// key: its provider, its catalog from its cache file and its fetch. Nothing
    /// changes when it runs already, or in a daemon whose provider a test injects.
    async fn add_account(&self, id: &str) -> Result<(), DaemonError> {
        let Some(credential) = &self.credential else {
            return Ok(());
        };
        if self.catalogs.of(id).is_some() {
            return Ok(());
        }
        let settings = match &self.settings {
            Some(settings) => Arc::clone(&settings.borrow()),
            None => Arc::new(Settings::default()),
        };
        let cache = catalog::cache_path_of(id, &self.state);
        let catalog = catalog::load_of(&settings, id, &cache).await;
        credential.add_catalog(id, catalog.clone());
        let mut provider = credential.provider(id)?;
        if takes_key(id) {
            provider = Arc::new(Watched::new(provider, id, Arc::clone(&self.refusals)));
        }
        let models = match credential.fetcher(id, &catalog)? {
            Some(fetcher) => {
                let models = Models::fetched(id, fetcher, cache, Arc::clone(&credential.clock));
                if takes_key(id) {
                    models.with_refusals(Arc::clone(&self.refusals))
                } else {
                    models
                }
            }
            None => Models::fixed(id, catalog),
        };
        let models = Arc::new(models);
        self.catalogs.add(Arc::clone(&models));
        self.others.write().unwrap_or_else(PoisonError::into_inner).push((id.to_owned(), provider));
        if let Some(stop) = self.stop.get() {
            tokio::spawn(models.serve(stop.clone()));
        }
        tracing::info!(provider = %id, "account ready");
        Ok(())
    }

    /// Follows `settings`: a reload that changes `[model] provider` changes the
    /// provider of new conversations at once, since every provider runs.
    pub(crate) fn with_settings(mut self, settings: watch::Receiver<Arc<Settings>>) -> Self {
        self.settings = Some(settings);
        self
    }

    /// The provider of new conversations now: `[model] provider` of the latest
    /// settings.
    fn configured(&self) -> String {
        match &self.settings {
            // NOTE: the id is cloned out so the watch's read lock is held only here.
            Some(settings) => settings.borrow().model.provider.clone(),
            None => self.configured.clone(),
        }
    }

    /// Reads from the credential store which providers have a login, for the model
    /// list ([`Catalogs::listed`]). efrd calls it once at start; each login and logout
    /// then keeps it current.
    pub(crate) async fn load_logins(&self) {
        for status in self.status().await {
            self.catalogs.set_logged_in(&status.provider, status.logged_in);
        }
    }

    /// The provider of new conversations at start.
    pub(crate) fn active(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.active.1)
    }

    /// The running provider `id`, else the provider of new conversations at start.
    fn route(&self, id: &str) -> Arc<dyn Provider> {
        if self.active.0 == id {
            return self.active();
        }
        let others = self.others.read().unwrap_or_else(PoisonError::into_inner);
        others
            .iter()
            .find(|(route, _)| route == id)
            .map_or_else(|| self.active(), |(_, provider)| Arc::clone(provider))
    }

    /// The router of the conversations: every provider that efrd runs, with the
    /// default from the latest `settings`.
    pub(crate) fn routes(&self, settings: watch::Receiver<Arc<Settings>>) -> Routes {
        Routes::new(self.active.clone(), Arc::clone(&self.others), settings)
    }

    /// The model catalog of the provider of new conversations, and its fetch.
    pub(crate) fn models(&self) -> Arc<Models> {
        self.catalogs.of(&self.configured()).unwrap_or_else(|| self.catalogs.first())
    }

    /// Every catalog and the logins: the model list of `models.list` and of the turns.
    pub(crate) fn catalogs(&self) -> Arc<Catalogs> {
        Arc::clone(&self.catalogs)
    }

    /// The catalog of `provider`, if efrd keeps one.
    fn models_of(&self, provider: &str) -> Option<Arc<Models>> {
        self.catalogs.of(provider)
    }

    /// True when `provider` is the provider of new conversations, `[model] provider`.
    pub(crate) fn is_active(&self, provider: &str) -> bool {
        provider == self.configured()
    }

    /// The provider of a prompt that asks for the model `asked` (`None`: the default
    /// model) with `settings`, as its turn picks it: the first provider whose list
    /// names the model in the order of routing ([`Catalogs::routed`]), else the
    /// provider of new conversations. It waits first for the first fetch of the
    /// provider of new conversations while it has no list ([`Models::ready`]), and, for
    /// a model that no list names yet, for the first fetch of each provider of the
    /// model's company with a login, such as a Claude list that efrd still fetches. A
    /// list whose fetch failed holds no prompt: the prompt goes on at once, and
    /// [`Providers::check_list`] refuses it with the kept reason when it needs the list.
    /// A prompt that names its provider (`named`), such as an account, goes to that
    /// one after its first fetch.
    pub(crate) async fn provider_of_prompt(
        &self,
        settings: &Settings,
        asked: Option<&str>,
        named: Option<&str>,
    ) -> Result<String, DaemonError> {
        let configured = self.catalogs.configured(settings);
        configured.ready().await;
        // NOTE: by the providers that run, also one whose list efrd still fetches; the
        // turn checks the model against the list of its start.
        if let Some(name) = named {
            let ids = self.ids();
            let logged_in: Vec<&str> =
                ids.iter().map(String::as_str).filter(|id| self.catalogs.logged_in(id)).collect();
            let Some(id) = efr_protocol::provider_id_named(name, &logged_in) else {
                return Err(DaemonError::UnknownProviderName {
                    name: name.to_owned(),
                    choices: logged_in.iter().map(|id| (*id).to_owned()).collect(),
                });
            };
            let id = id.to_owned();
            if let Some(models) = self.models_of(&id) {
                models.ready().await;
            }
            return Ok(id);
        }
        let default;
        let model = match asked {
            Some(model) => model,
            None => {
                default = configured.default_model(settings);
                &default
            }
        };
        if let Some(provider) = self.listed_provider(settings, model) {
            return Ok(provider);
        }
        let Some(company) = ModelCompany::of_model(model) else {
            return Ok(configured.provider().to_owned());
        };
        let all = self.catalogs.all();
        let waiting = all.iter().filter(|models| {
            ModelCompany::of_provider(models.provider()) == Some(company)
                && self.catalogs.logged_in(models.provider())
                && models.missing().is_some()
        });
        futures::future::join_all(waiting.map(|models| models.ready())).await;
        Ok(self
            .listed_provider(settings, model)
            .unwrap_or_else(|| configured.provider().to_owned()))
    }

    /// The first provider whose list names `model`, in the order of routing.
    fn listed_provider(&self, settings: &Settings, model: &str) -> Option<String> {
        self.catalogs
            .routed(settings)
            .into_iter()
            .filter(|info| info.id == model)
            .find_map(|info| info.provider)
    }

    /// Refuses a prompt whose turn cannot run on `provider` because its catalog still
    /// has no list after [`Models::ready`], with the cause: no key is stored, or the
    /// fetch failed or gave no list. A turn on a model whose output limit the config
    /// gives (in the config's list of models or as `[model] max_output_tokens`) runs
    /// without a list, unless no key is stored. `asked` is the prompt's own model.
    pub(crate) fn check_list(
        &self,
        settings: &Settings,
        provider: &str,
        asked: Option<&str>,
    ) -> Result<(), DaemonError> {
        let models = self.models_of(provider).unwrap_or_else(|| self.catalogs.first());
        let Some(missing) = models.missing() else {
            return Ok(());
        };
        if !matches!(missing, NoList::NotLoggedIn) {
            let model = asked.map_or_else(|| models.default_model(settings), str::to_owned);
            let limited = self
                .route(provider)
                .models()
                .iter()
                .any(|info| info.id == model && info.max_output_tokens.is_some());
            if limited || settings.model.max_output_tokens.is_some() {
                return Ok(());
            }
        }
        let list = models.list().vendor().list_name();
        let provider = models.provider();
        Err(match missing {
            NoList::NotLoggedIn => DaemonError::NoModelListWithoutKey {
                provider: provider.to_owned(),
                login: login_command(provider),
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
    ///
    /// `provider` is `openai-subscription` or one more ChatGPT account of it, such as
    /// `openai-subscription.b`. The login of an account asks the browser for a new
    /// sign-in, and every login refuses the ChatGPT account of another login, before
    /// anything is saved.
    pub(crate) async fn start_login(&self, provider: &str) -> Result<PendingLogin, DaemonError> {
        if !efr_config::is_provider(provider) || efr_config::provider_kind(provider) != SUBSCRIPTION
        {
            return Err(DaemonError::NoSuchProvider { provider: provider.to_owned() });
        }
        let taken = self.chatgpt_accounts_besides(provider).await;
        let pending = self
            .login
            .start_for(credential(provider)?, provider != SUBSCRIPTION)
            .await
            .map_err(|source| DaemonError::Login { source })?;
        Ok(pending.refusing(taken))
    }

    /// The ChatGPT account ids of the logins of the subscription besides `provider`.
    pub(crate) async fn chatgpt_accounts_besides(&self, provider: &str) -> Vec<String> {
        let others: Vec<String> = self
            .ids()
            .into_iter()
            .filter(|id| id != provider && efr_config::provider_kind(id) == SUBSCRIPTION)
            .collect();
        let store = Arc::clone(&self.store);
        let loaded = tokio::task::spawn_blocking(move || {
            others
                .into_iter()
                .filter_map(|id| {
                    let id = CredentialId::new(&id).ok()?;
                    match store.load(&id).ok().flatten() {
                        Some(CredentialRecord::OAuth(tokens)) => tokens.account_id,
                        _ => None,
                    }
                })
                .collect()
        });
        loaded.await.unwrap_or_default()
    }

    /// Makes the provider of the ChatGPT login `provider` read the new login at its next
    /// request, runs it when it is a new account, and fetches its model catalog.
    pub(crate) async fn login_completed(&self, provider: &str) -> Result<(), DaemonError> {
        if let Some(credential) = &self.credential {
            credential.token_source(provider)?.clear_cache();
        } else if provider == SUBSCRIPTION {
            self.subscription.clear_cache();
        }
        self.add_account(provider).await?;
        self.catalogs.set_logged_in(provider, true);
        if let Some(models) = self.models_of(provider) {
            models.logged_in();
        }
        Ok(())
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
        if !efr_config::is_provider(provider) {
            return Err(DaemonError::NoSuchProvider { provider: provider.to_owned() });
        }
        if !takes_key(provider) {
            return Err(DaemonError::NoApiKeyLogin { provider: provider.to_owned() });
        }
        api_key::validate(provider, key.expose_secret())?;
        let key = SecretString::from(key.expose_secret());
        if let Some(other) = self.account_with_key(provider, &key).await {
            return Err(DaemonError::KeyOfAnotherAccount { provider: provider.to_owned(), other });
        }
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
        if check {
            self.refusals.checked(provider);
        }
        let active = provider == self.configured();
        // NOTE: a new account runs from now on, with its own catalog and fetch.
        self.add_account(provider).await?;
        self.catalogs.set_logged_in(provider, true);
        if let Some(models) = self.models_of(provider) {
            models.logged_in();
        }
        Ok(KeyLogin { hint, active })
    }

    /// The other account of the kind of `provider` whose stored key is `key`.
    async fn account_with_key(&self, provider: &str, key: &SecretString) -> Option<String> {
        let kind = efr_config::provider_kind(provider).to_owned();
        let others: Vec<String> = self
            .ids()
            .into_iter()
            .filter(|id| id != provider && efr_config::provider_kind(id) == kind)
            .collect();
        let store = Arc::clone(&self.store);
        // NOTE: the key stays a secret: compared inside the task, never copied out.
        let key = key.clone();
        let found = tokio::task::spawn_blocking(move || {
            others.into_iter().find(|id| {
                let record =
                    CredentialId::new(id).ok().and_then(|id| store.load(&id).ok().flatten());
                matches!(
                    record,
                    Some(CredentialRecord::ApiKey { key: stored })
                        if stored.expose_secret() == key.expose_secret()
                )
            })
        });
        found.await.ok().flatten()
    }

    /// Deletes the credential of `provider`. False when none was stored.
    pub(crate) async fn logout(&self, provider: &str) -> Result<bool, DaemonError> {
        if !efr_config::is_provider(provider) {
            return Err(DaemonError::NoSuchProvider { provider: provider.to_owned() });
        }
        let id = credential(provider)?;
        let store = Arc::clone(&self.store);
        let deleted = tokio::task::spawn_blocking(move || store.delete(&id))
            .await
            .map_err(|_| DaemonError::TaskPanicked { task: "delete a credential" })?
            .map_err(|source| DaemonError::Credentials { source })?;
        if efr_config::provider_kind(provider) == SUBSCRIPTION {
            // The running provider must not keep the token of the deleted login.
            match &self.credential {
                Some(credential) => credential.token_source(provider)?.clear_cache(),
                None => self.subscription.clear_cache(),
            }
        }
        if deleted {
            self.catalogs.set_logged_in(provider, false);
            self.refusals.reset(provider);
            if let Some(models) = self.models_of(provider) {
                models.logged_out();
            }
        }
        Ok(deleted)
    }

    /// Whether each provider, each account included, has credentials, for
    /// `admin.status`: the providers in the order of [`PROVIDERS`], then the accounts.
    pub(crate) async fn status(&self) -> Vec<ProviderStatus> {
        let store = Arc::clone(&self.store);
        let ids = self.ids();
        let loaded = tokio::task::spawn_blocking(move || {
            ids.into_iter().map(|id| {
                let record = CredentialId::new(&id).ok().and_then(|id| match store.load(&id) {
                    Ok(record) => record,
                    Err(error) => {
                        tracing::warn!(error = %error, credential = %id, "a credential could not be read");
                        None
                    }
                });
                (id, record)
            })
            .collect::<Vec<_>>()
        })
        .await;
        let Ok(loaded) = loaded else {
            return Vec::new();
        };
        let configured = self.configured();
        loaded
            .into_iter()
            .map(|(id, record)| {
                let mut status = provider_status(&id, record.as_ref(), id == configured);
                if status.login == Some(LoginKind::ApiKey) {
                    status.key_refused_at = self.refusals.refused_at(&id);
                    status.key_checked_at = self.refusals.checked_at(&id);
                }
                status
            })
            .collect()
    }

    /// Every provider id that efrd knows: [`PROVIDERS`], then each account that runs.
    fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = PROVIDERS.iter().map(|id| (*id).to_owned()).collect();
        let others = self.others.read().unwrap_or_else(PoisonError::into_inner);
        for (id, _) in std::iter::once(&self.active).chain(others.iter()) {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids
    }
}

/// The accounts of `store` that efrd runs next to the providers of [`PROVIDERS`]: the
/// stored credentials of a provider with an account, such as `anthropic-api.work` or
/// `openai-subscription.b`, in order. A store that cannot list has none.
pub(crate) async fn stored_accounts(store: Arc<dyn SecretStore>) -> Vec<String> {
    let listed = tokio::task::spawn_blocking(move || store.list()).await;
    let ids = match listed {
        Ok(Ok(ids)) => ids,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "the credentials could not be listed; efrd runs no account");
            return Vec::new();
        }
        Err(_) => return Vec::new(),
    };
    ids.into_iter()
        .map(|id| id.as_str().to_owned())
        .filter(|id| id.contains('.') && efr_config::is_provider(id))
        .collect()
}

/// The catalog of every provider and of each of `accounts` to start with: the cache
/// file of each in the state root `state`, else the list built into efr, else no list.
pub(crate) async fn catalog_starts(
    config: &Settings,
    state: &Path,
    accounts: &[String],
) -> Vec<CatalogStart> {
    let ids = PROVIDERS.iter().map(|id| (*id).to_owned()).chain(accounts.iter().cloned());
    let mut starts = Vec::with_capacity(PROVIDERS.len() + accounts.len());
    for provider in ids {
        let cache = catalog::cache_path_of(&provider, state);
        let catalog = catalog::load_of(config, &provider, &cache).await;
        starts.push(CatalogStart { provider, catalog, cache });
    }
    starts
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
        key_checked_at: None,
    }
}

/// The command that logs in to `provider`.
pub(crate) fn login_command(provider: &str) -> &'static str {
    match efr_config::provider_kind(provider) {
        ANTHROPIC => "efr login anthropic",
        API => "efr login openai-api",
        _ => "efr login openai",
    }
}

/// Warns when `[model] name` is a model of another company than `[model] provider`
/// (`efr_config::ForeignModel`): every turn that does not name its own model then
/// fails while efr has no login to a provider of the model's company. efrd calls it at start, and after a reload that
/// changed what it warns about (C3.1). Returns the warning for the user, as
/// `efr config check` words it, when it warned.
pub(crate) fn warn_foreign_model(config: &Settings) -> Option<String> {
    let foreign = config.foreign_model()?;
    tracing::warn!(
        model = %foreign.name,
        company = foreign.company.name(),
        provider = %foreign.provider,
        fix = %foreign.fix(),
        "model.name is a model of another company than model.provider, so a turn that does not name its own model fails without a login to that company"
    );
    Some(format!(
        "{foreign}, so a turn that does not name its own model fails without a login to {}; {}",
        foreign.company.name(),
        foreign.fix()
    ))
}

/// Warns when the config's default model belongs to another company than the
/// provider, is not in the model list, or its default effort is not one the default
/// model takes: every prompt that leaves them to the config then fails until the file
/// is fixed.
fn warn_unfit_defaults(config: &Settings, models: &Models) {
    let foreign = warn_foreign_model(config).is_some();
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
        tracing::warn!(effort = %effort, model = %default, efforts = ?efforts, "model.effort is not an effort of the default model, so a turn on it that does not name its own effort uses the model's default effort");
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
    /// The token source of the default ChatGPT login, `openai-subscription`.
    subscription: Arc<OpenAiTokenSource>,
    /// The OAuth settings of each other ChatGPT login.
    oauth: OAuthConfig,
    /// The token source of each other ChatGPT login, such as `openai-subscription.b`.
    subscriptions: RwLock<Vec<(String, Arc<OpenAiTokenSource>)>>,
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    rng: Arc<dyn Rng>,
    /// The catalog of each provider, which its provider and its fetch share; a login
    /// adds the one of a new account.
    catalogs: RwLock<Vec<(String, ProviderCatalog)>>,
}

impl CredentialProviders {
    /// The token source of the ChatGPT login `id`: the default one, or the one of an
    /// account, made at its first use.
    fn token_source(&self, id: &str) -> Result<Arc<OpenAiTokenSource>, DaemonError> {
        if id == SUBSCRIPTION {
            return Ok(Arc::clone(&self.subscription));
        }
        let mut sources = self.subscriptions.write().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, source)) = sources.iter().find(|(known, _)| known == id) {
            return Ok(Arc::clone(source));
        }
        let source = Arc::new(OpenAiTokenSource::new(
            self.oauth.clone(),
            self.http.clone(),
            Arc::clone(&self.store),
            credential(id)?,
            Arc::clone(&self.clock),
        ));
        sources.push((id.to_owned(), Arc::clone(&source)));
        Ok(source)
    }

    /// Keeps `catalog` as the one of the provider `id`.
    fn add_catalog(&self, id: &str, catalog: ProviderCatalog) {
        self.catalogs
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id.to_owned(), catalog));
    }

    /// The fetch of the catalog `catalog` of the provider `id`, with its stored key or
    /// its ChatGPT login; `None` for a catalog of another form.
    fn fetcher(&self, id: &str, catalog: &ProviderCatalog) -> Result<Option<Fetcher>, DaemonError> {
        Ok(match (efr_config::provider_kind(id), catalog) {
            (SUBSCRIPTION, ProviderCatalog::OpenAi(shared)) => {
                let base_url = self.openai.subscription_base_url.as_deref();
                let openai = openai_config(OpenAiConfig::subscription(), &self.openai, base_url)?;
                let tokens = self.token_source(id)? as Arc<dyn TokenSource>;
                let client = efr_providers::openai::CatalogClient::new(
                    openai,
                    self.http.clone(),
                    tokens,
                    Arc::clone(&self.clock),
                );
                Some(Fetcher::OpenAi(shared.clone(), client))
            }
            (API, ProviderCatalog::OpenAi(shared)) => {
                let base_url = self.openai.api_base_url.as_deref();
                let api = openai_config(OpenAiConfig::api(), &self.openai, base_url)?;
                let client = efr_providers::openai::CatalogClient::new(
                    api,
                    self.http.clone(),
                    stored_key(&self.store, id)?,
                    Arc::clone(&self.clock),
                );
                Some(Fetcher::OpenAi(shared.clone(), client))
            }
            (ANTHROPIC, ProviderCatalog::Anthropic(shared)) => {
                let client = efr_providers::anthropic::CatalogClient::new(
                    self.anthropic.clone(),
                    self.http.clone(),
                    stored_key(&self.store, id)?,
                    Arc::clone(&self.clock),
                    Arc::clone(&self.rng),
                );
                Some(Fetcher::Anthropic(shared.clone(), client))
            }
            _ => None,
        })
    }
}

impl ProviderFactory for CredentialProviders {
    fn provider(&self, id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let provider_id = ProviderId::new(id).map_err(|source| DaemonError::Provider { source })?;
        let catalog = self
            .catalogs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(provider, _)| provider == id)
            .map(|(_, catalog)| catalog.clone());
        let Some(catalog) = catalog else {
            return Err(DaemonError::UnknownProvider { id: id.to_owned() });
        };
        let (config, tokens) = match efr_config::provider_kind(id) {
            API => {
                let tokens = stored_key(&self.store, id)?;
                let base_url = self.openai.api_base_url.as_deref();
                (openai_config(OpenAiConfig::api(), &self.openai, base_url)?, tokens)
            }
            SUBSCRIPTION => {
                let tokens: Arc<dyn TokenSource> = self.token_source(id)?;
                let base_url = self.openai.subscription_base_url.as_deref();
                (openai_config(OpenAiConfig::subscription(), &self.openai, base_url)?, tokens)
            }
            ANTHROPIC => {
                let provider = AnthropicProvider::new(
                    provider_id,
                    self.anthropic.clone(),
                    self.http.clone(),
                    stored_key(&self.store, id)?,
                    Arc::clone(&self.clock),
                    Arc::clone(&self.rng),
                )
                .with_catalog(catalog.anthropic());
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
        .with_catalog(catalog.openai(backend(id)));
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
        efr_stdx::mark::hit("key.read");
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
