use std::sync::Arc;

use efr_config::{ModelLimits, Settings, WebSocketChoice};
use efr_credentials::{CredentialId, CredentialRecord, FileStore, OAuthTokens, SecretStore as _};
use efr_http::{HttpClient, HttpConfig};
use efr_provider::{ExposeSecret as _, ProviderError, SecretString, TokenSource as _};
use efr_provider_openai::{Backend, Catalog, OpenAiConfig, WebSocketMode};
use efr_test_support::{TestClock, TestRng};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use crate::providers::{
    API, ProviderFactory, ProviderParts, Providers, SUBSCRIPTION, StoredApiKey, openai_config,
    provider_status,
};
use crate::testing::OneAnswerFactory;

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
        catalog: Catalog::builtin(Backend::Subscription),
        catalog_cache: dir.join("state").join("model_catalog.json"),
    }
}

#[test]
fn status_reports_a_login_and_its_expiry() {
    let expires_at = Timestamp::from_second(1_800_000_000).unwrap();
    let mut tokens = OAuthTokens::new(SecretString::from("access"));
    tokens.expires_at = Some(expires_at);
    let oauth = CredentialRecord::OAuth(tokens);
    let key = CredentialRecord::ApiKey { key: SecretString::from("sk-test") };

    let subscription = provider_status(SUBSCRIPTION, Some(&oauth));
    let api = provider_status(API, Some(&key));
    let missing = provider_status(API, None);

    assert_eq!(subscription.provider, "openai-subscription");
    assert!(subscription.logged_in);
    assert_eq!(subscription.expires_at, Some(expires_at));
    assert!(api.logged_in);
    assert_eq!(api.expires_at, None);
    assert!(!missing.logged_in);
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
    assert!(!providers.models().fetches(), "the API key backend never fetches");
    let status = providers.status().await;
    assert_eq!(
        status.iter().map(|s| (s.provider.as_str(), s.logged_in)).collect::<Vec<_>>(),
        [("openai-subscription", false), ("openai-api", false)]
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
