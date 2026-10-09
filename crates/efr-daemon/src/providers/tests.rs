use std::sync::Arc;

use efr_config::{CacheTtlChoice, ModelLimits, Settings, WebSocketChoice};
use efr_credentials::{CredentialId, CredentialRecord, FileStore, OAuthTokens, SecretStore as _};
use efr_http::{HttpClient, HttpConfig};
use efr_protocol::{CatalogOrigin, LoginKind, SecretText};
use efr_provider::{ExposeSecret as _, ProviderError, SecretString, TokenSource as _};
use efr_provider_anthropic::CacheTtl;
use efr_provider_openai::{Backend, Catalog, ModelCatalog, OpenAiConfig, WebSocketMode};
use efr_test_support::{TestClock, TestRng};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use crate::catalog::ProviderCatalog;
use crate::providers::{
    ANTHROPIC, API, KeyLogin, ProviderFactory, ProviderParts, Providers, SUBSCRIPTION,
    StoredApiKey, anthropic_config, openai_config, provider_status,
};
use crate::testing::OneAnswerFactory;
use crate::{DaemonError, KeyProblem};

fn store(dir: &std::path::Path) -> Arc<FileStore> {
    Arc::new(FileStore::new(dir.join("secrets")))
}

fn http(clock: &TestClock) -> HttpClient {
    HttpClient::new(&HttpConfig::default(), clock.shared(), Arc::new(TestRng::new(1))).unwrap()
}

fn parts(
    clock: &TestClock,
    dir: &std::path::Path,
    factory: Option<Arc<dyn ProviderFactory>>,
) -> ProviderParts {
    ProviderParts {
        http: http(clock),
        clock: clock.shared(),
        rng: Arc::new(TestRng::new(2)),
        factory,
        issuer: None,
        catalog: ProviderCatalog::OpenAi(ModelCatalog::new(Catalog::builtin(
            Backend::Subscription,
        ))),
        catalog_cache: dir.join("state").join("model_catalog.json"),
    }
}

#[test]
fn status_reports_a_login_and_its_expiry() {
    let expires_at = Timestamp::from_second(1_800_000_000).unwrap();
    let mut tokens = OAuthTokens::new(SecretString::from("access"));
    tokens.expires_at = Some(expires_at);
    let oauth = CredentialRecord::OAuth(tokens);
    let key = CredentialRecord::ApiKey { key: SecretString::from("sk-ant-api03-efr-test-a1b2") };

    let subscription = provider_status(SUBSCRIPTION, Some(&oauth), true);
    let anthropic = provider_status(ANTHROPIC, Some(&key), false);
    let missing = provider_status(API, None, false);

    assert_eq!(subscription.provider, "openai-subscription");
    assert!(subscription.logged_in);
    assert!(subscription.active);
    assert_eq!(subscription.login, Some(LoginKind::Subscription));
    assert_eq!(subscription.expires_at, Some(expires_at));
    assert_eq!(subscription.key_hint, None);
    assert!(anthropic.logged_in);
    assert!(!anthropic.active);
    assert_eq!(anthropic.login, Some(LoginKind::ApiKey));
    assert_eq!(anthropic.expires_at, None);
    assert_eq!(anthropic.key_hint.as_deref(), Some("sk-ant-...a1b2"));
    assert!(!missing.logged_in);
    assert_eq!(missing.login, None);
    assert_eq!(missing.key_hint, None);
}

#[test]
fn the_openai_settings_reach_the_provider_config() {
    let mut settings = Settings::default().openai;
    settings.originator = "efr-test".to_owned();
    let mut next = ModelLimits::new("gpt-next");
    next.context_window = Some(1_000_000);
    next.max_output_tokens = Some(64_000);
    settings.models = Some(vec!["m1".into(), next.into()]);

    let config =
        openai_config(OpenAiConfig::subscription(), &settings, Some("http://127.0.0.1:9/codex/"))
            .unwrap();

    assert_eq!(config.originator(), "efr-test");
    assert_eq!(config.base_url(), "http://127.0.0.1:9/codex");
    let ids: Vec<&str> = config.models().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["m1", "gpt-next"], "the configured models lie over the catalog");
    assert_eq!(config.models()[1].context_window, Some(1_000_000));
    assert_eq!(config.models()[1].max_output_tokens, Some(64_000));
    assert_eq!(config.reasoning_effort(), None, "each turn sends its own effort");
}

#[test]
fn the_websocket_switch_reaches_the_provider_config() {
    let mut settings = Settings::default().openai;
    let mode = |settings: &efr_config::OpenAiSettings| {
        openai_config(OpenAiConfig::subscription(), settings, None).unwrap().websocket()
    };

    assert_eq!(settings.websocket, WebSocketChoice::Auto);
    assert_eq!(mode(&settings), WebSocketMode::Auto);
    settings.websocket = WebSocketChoice::On;
    assert_eq!(mode(&settings), WebSocketMode::On);
    settings.websocket = WebSocketChoice::Off;
    assert_eq!(mode(&settings), WebSocketMode::Off);
}

#[test]
fn the_config_default_originator_is_the_provider_default() {
    assert_eq!(Settings::default().openai.originator, efr_provider_openai::DEFAULT_ORIGINATOR);
}

#[test]
fn an_originator_that_is_not_a_header_value_is_refused() {
    let mut settings = Settings::default().openai;
    settings.originator = "bad\nvalue".to_owned();

    assert!(openai_config(OpenAiConfig::subscription(), &settings, None).is_err());
}

#[tokio::test]
async fn the_api_key_is_read_from_its_credential_at_each_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let id = CredentialId::new(API).unwrap();
    let tokens = StoredApiKey { store: store.clone(), id: id.clone() };

    assert!(matches!(tokens.access_token().await, Err(ProviderError::NotLoggedIn)));

    store.save(&id, &CredentialRecord::ApiKey { key: SecretString::from("sk-test") }).unwrap();
    let token = tokens.access_token().await.unwrap();
    assert_eq!(token.secret().expose_secret(), "sk-test");
    assert_eq!(token.account_id(), None);
    assert!(!tokens.refreshable(), "a key cannot refresh, so a 401 fails at once");
}

#[tokio::test]
async fn the_configured_provider_is_built_without_touching_the_network() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let mut config = Settings::default();
    config.model.provider = API.to_owned();

    let providers =
        Providers::build(&config, store(dir.path()), parts(&clock, dir.path(), None)).unwrap();

    assert_eq!(providers.active().id().as_str(), "openai-api");
    assert!(providers.models().fetches(), "the API key backend fetches the ids of its key");
    let status = providers.status().await;
    assert_eq!(
        status.iter().map(|s| (s.provider.as_str(), s.logged_in, s.active)).collect::<Vec<_>>(),
        [
            ("openai-subscription", false, false),
            ("openai-api", false, true),
            ("anthropic-api", false, false)
        ]
    );
}

#[tokio::test]
async fn a_key_login_stores_the_key_and_a_logout_deletes_it() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let mut config = Settings::default();
    config.model.provider = API.to_owned();
    let store = store(dir.path());
    let providers =
        Providers::build(&config, store.clone(), parts(&clock, dir.path(), None)).unwrap();
    let key = SecretText::new("sk-ant-api03-efr-test-a1b2");

    let login = providers.login_api_key(ANTHROPIC, &key, false).await.unwrap();

    assert_eq!(login, KeyLogin { hint: "sk-ant-...a1b2".to_owned(), active: false });
    let id = CredentialId::new(ANTHROPIC).unwrap();
    let Some(CredentialRecord::ApiKey { key: stored }) = store.load(&id).unwrap() else {
        panic!("no key stored")
    };
    assert_eq!(stored.expose_secret(), "sk-ant-api03-efr-test-a1b2");
    let status = providers.status().await;
    assert_eq!(status[2].key_hint.as_deref(), Some("sk-ant-...a1b2"));

    assert!(providers.logout(ANTHROPIC).await.unwrap());
    assert!(store.load(&id).unwrap().is_none());
    assert!(!providers.logout(ANTHROPIC).await.unwrap(), "nothing left to delete");
}

#[tokio::test]
async fn a_key_login_refuses_other_providers_and_bad_keys_before_it_stores_anything() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let store = store(dir.path());
    let providers =
        Providers::build(&Settings::default(), store.clone(), parts(&clock, dir.path(), None))
            .unwrap();
    let key = SecretText::new("sk-proj-efr-test-key-9f3c");

    let subscription = providers.login_api_key(SUBSCRIPTION, &key, false).await.unwrap_err();
    let unknown = providers.login_api_key("gemini", &key, false).await.unwrap_err();
    let admin = providers
        .login_api_key(API, &SecretText::new("sk-admin-efr-test-key"), false)
        .await
        .unwrap_err();
    let logout = providers.logout("gemini").await.unwrap_err();

    assert!(matches!(subscription, DaemonError::NoApiKeyLogin { .. }), "{subscription:?}");
    assert!(matches!(unknown, DaemonError::NoSuchProvider { .. }), "{unknown:?}");
    assert!(
        matches!(admin, DaemonError::InvalidApiKey { problem: KeyProblem::AdminKey, .. }),
        "{admin:?}"
    );
    assert!(matches!(logout, DaemonError::NoSuchProvider { .. }), "{logout:?}");
    for id in [SUBSCRIPTION, API, ANTHROPIC] {
        assert!(store.load(&CredentialId::new(id).unwrap()).unwrap().is_none(), "{id}");
    }
}

#[test]
fn the_organization_and_the_project_reach_the_provider_config() {
    let mut settings = Settings::default().openai;
    settings.organization = Some("org-AbC".to_owned());
    settings.project = Some("proj_AbC".to_owned());

    let config = openai_config(OpenAiConfig::api(), &settings, None).unwrap();

    assert_eq!(config.organization(), Some("org-AbC"));
    assert_eq!(config.project(), Some("proj_AbC"));
}

#[test]
fn the_anthropic_settings_reach_the_provider_config() {
    let mut settings = Settings::default().anthropic;
    let mut lower = ModelLimits::new("claude-opus-5-5");
    lower.context_window = Some(272_000);
    settings.models = Some(vec![lower.into(), "claude-next".into()]);
    settings.base_url = Some("http://127.0.0.1:9/v1/".to_owned());
    settings.workspace_id = Some("wrkspc_efr".to_owned());

    let config = anthropic_config(&settings).unwrap();

    assert_eq!(config.base_url(), "http://127.0.0.1:9/v1");
    assert_eq!(config.workspace_id(), Some("wrkspc_efr"));
    assert_eq!(config.cache_ttl(), CacheTtl::Auto, "auto is the default");
    let ids: Vec<&str> = config.models().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        ["claude-opus-5-5", "claude-next"],
        "the configured models lie over the catalog"
    );
    assert_eq!(config.models()[0].context_window, Some(272_000));

    for (choice, ttl) in [
        (CacheTtlChoice::FiveMinutes, CacheTtl::FiveMinutes),
        (CacheTtlChoice::OneHour, CacheTtl::OneHour),
    ] {
        settings.cache_ttl = choice;
        assert_eq!(anthropic_config(&settings).unwrap().cache_ttl(), ttl);
    }
}

#[test]
fn the_anthropic_provider_is_built_and_fetches_its_catalog_without_a_list_yet() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let mut config = Settings::default();
    config.model.provider = ANTHROPIC.to_owned();
    let mut parts = parts(&clock, dir.path(), None);
    parts.catalog = ProviderCatalog::Anthropic(efr_provider_anthropic::ModelCatalog::new());

    let providers = Providers::build(&config, store(dir.path()), parts).unwrap();

    assert_eq!(providers.active().id().as_str(), "anthropic-api");
    assert!(providers.active().models().is_empty(), "no list until a fetch");
    let models = providers.models();
    assert!(models.fetches(), "efrd fetches the list of the Anthropic API");
    let status = models.status();
    assert_eq!(status.provider.as_deref(), Some("anthropic-api"));
    assert_eq!(status.origin, CatalogOrigin::Missing);
    assert_eq!(models.default_model(&config), efr_provider_anthropic::DEFAULT_MODEL);
}

#[test]
fn a_provider_that_efrd_does_not_know_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let mut config = Settings::default();
    config.model.provider = "gemini-api".to_owned();

    let error = Providers::build(&config, store(dir.path()), parts(&clock, dir.path(), None))
        .map(|_| ())
        .unwrap_err();

    assert!(
        matches!(&error, DaemonError::UnknownProvider { id } if id == "gemini-api"),
        "{error:?}"
    );
}

#[test]
fn the_subscription_fetches_its_catalog_and_its_provider_reads_it() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let config = Settings::default();

    let providers =
        Providers::build(&config, store(dir.path()), parts(&clock, dir.path(), None)).unwrap();

    assert!(providers.models().fetches());
    let models = providers.active().models();
    assert_eq!(models.first().map(|model| model.id.as_str()), Some("gpt-6.1-sol"));
    assert!(models.iter().all(|model| model.freeform_tools));
}

#[test]
fn a_daemon_with_its_own_provider_never_fetches_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let config = Settings::default();

    let providers = Providers::build(
        &config,
        store(dir.path()),
        parts(&clock, dir.path(), Some(Arc::new(OneAnswerFactory))),
    )
    .unwrap();

    assert!(!providers.models().fetches());
    assert_eq!(providers.models().default_model(&config), "gpt-6.1-sol");
}
