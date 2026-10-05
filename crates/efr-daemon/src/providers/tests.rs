use std::sync::Arc;

use efr_config::Settings;
use efr_credentials::{CredentialId, CredentialRecord, FileStore, OAuthTokens, SecretStore as _};
use efr_http::{HttpClient, HttpConfig};
use efr_protocol::{ModelInfo, ModelSource};
use efr_provider::{ExposeSecret as _, ProviderError, SecretString, TokenSource as _};
use efr_provider_openai::OpenAiConfig;
use efr_test_support::{TestClock, TestRng};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use crate::providers::{
    API, Providers, SUBSCRIPTION, StoredApiKey, default_model, effective_models, openai_config,
    provider_status,
};

fn known_models(config: &Settings) -> Vec<String> {
    effective_models(config).into_iter().map(|model| model.id).collect()
}

fn store(dir: &std::path::Path) -> Arc<FileStore> {
    Arc::new(FileStore::new(dir.join("secrets")))
}

fn http(clock: &TestClock) -> HttpClient {
    HttpClient::new(&HttpConfig::default(), clock.shared(), Arc::new(TestRng::new(1))).unwrap()
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
fn the_model_is_the_configured_one_then_the_first_listed_then_the_default() {
    let mut config = Settings::default();
    assert_eq!(default_model(&config), efr_provider_openai::DEFAULT_SUBSCRIPTION_MODEL);

    config.openai.models = Some(vec!["gpt-6-sol".to_owned(), "gpt-5.5".to_owned()]);
    assert_eq!(default_model(&config), "gpt-6-sol");

    config.model.name = Some("gpt-6-luna".to_owned());
    assert_eq!(default_model(&config), "gpt-6-luna");
}

#[test]
fn the_effective_models_are_the_builtin_list_then_the_configured_ids() {
    let mut config = Settings::default();
    let builtin = effective_models(&config);
    assert!(builtin.iter().all(|model| model.source == ModelSource::Builtin));
    let default: Vec<&str> =
        builtin.iter().filter(|model| model.default).map(|model| model.id.as_str()).collect();
    assert_eq!(default, [efr_provider_openai::DEFAULT_SUBSCRIPTION_MODEL]);
    let gpt_5_5 = builtin.iter().find(|model| model.id == "gpt-5.5").unwrap();
    assert_eq!(gpt_5_5.efforts, ["low", "medium", "high", "xhigh"]);
    assert_eq!(gpt_5_5.default_effort.as_deref(), Some("medium"));

    config.openai.models = Some(vec!["gpt-next".to_owned(), "gpt-5.5".to_owned()]);
    let models = effective_models(&config);
    assert_eq!(models.len(), builtin.len() + 1, "an id the list holds is not added again");
    let added = models.last().unwrap();
    assert_eq!(
        added,
        &ModelInfo {
            id: "gpt-next".to_owned(),
            efforts: Vec::new(),
            default_effort: None,
            default: true,
            source: ModelSource::Config,
        },
        "the first configured id is the default when model.name is unset"
    );
    assert_eq!(models.iter().filter(|model| model.default).count(), 1);
}

#[test]
fn an_unknown_default_model_is_named_and_marks_no_model() {
    let mut config = Settings::default();
    config.model.name = Some("gpt-9".to_owned());

    let known = known_models(&config);

    assert!(!effective_models(&config).iter().any(|model| model.default));
    assert_eq!(
        config.unknown_model(&known.iter().map(String::as_str).collect::<Vec<_>>()),
        Some("gpt-9")
    );
}

#[test]
fn the_api_provider_lists_only_the_configured_ids() {
    let mut config = Settings::default();
    config.model.provider = API.to_owned();
    assert!(effective_models(&config).is_empty());

    config.openai.models = Some(vec!["gpt-4.1".to_owned()]);
    assert_eq!(known_models(&config), ["gpt-4.1"]);
}

#[test]
fn the_openai_settings_reach_the_provider_config() {
    let mut settings = Settings::default().openai;
    settings.originator = "efr-test".to_owned();
    settings.models = Some(vec!["m1".to_owned(), "gpt-5.5".to_owned()]);

    let config =
        openai_config(OpenAiConfig::subscription(), &settings, Some("http://127.0.0.1:9/codex/"))
            .unwrap();

    assert_eq!(config.originator(), "efr-test");
    assert_eq!(config.base_url(), "http://127.0.0.1:9/codex");
    let ids: Vec<&str> = config.models().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids.last(), Some(&"m1"), "a configured id is added to the built-in list");
    assert_eq!(ids.len(), efr_provider_openai::subscription_models().len() + 1);
    assert_eq!(config.reasoning_effort(), None, "each turn sends its own effort");
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
}

#[tokio::test]
async fn the_configured_provider_is_built_without_touching_the_network() {
    let dir = tempfile::tempdir().unwrap();
    let clock = TestClock::new();
    let mut config = Settings::default();
    config.model.provider = API.to_owned();

    let providers = Providers::build(
        &config,
        store(dir.path()),
        http(&clock),
        clock.shared(),
        Arc::new(TestRng::new(2)),
        None,
        None,
    )
    .unwrap();

    assert_eq!(providers.active().id().as_str(), "openai-api");
    let status = providers.status().await;
    assert_eq!(
        status.iter().map(|s| (s.provider.as_str(), s.logged_in)).collect::<Vec<_>>(),
        [("openai-subscription", false), ("openai-api", false)]
    );
}
