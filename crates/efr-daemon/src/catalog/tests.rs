use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use efr_config::{ModelLimits, Settings};
use efr_http::{HttpClient, HttpConfig, RetryPolicy};
use efr_protocol::{CatalogOrigin as WireOrigin, ModelInfo, ModelSource};
use efr_provider::{AccessToken, ProviderError, SecretString, StaticToken, TokenSource};
use efr_provider_openai::{
    Backend, CLIENT_VERSION, Catalog, CatalogClient, CatalogOrigin, ModelCatalog, OpenAiConfig,
};
use efr_stdx::time::Clock as _;
use efr_test_support::{TestClock, TestRng, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{
    CATALOG_FILE, Clamp, Models, REFRESH_INTERVAL, default_model, effective_models, load,
    retry_wait, status,
};
use crate::providers::API;

const MODELS_PATH: &str = "/backend-api/codex/models";

/// A catalog in the backend's form: a model with the best priority and no larger window,
/// a model with a larger window, and a hidden one.
fn backend_list() -> Value {
    json!({"models": [
        {
            "slug": "gpt-7-sol", "visibility": "list", "priority": 2,
            "context_window": 300_000, "max_context_window": 900_000,
            "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}],
            "default_reasoning_level": "low", "apply_patch_tool_type": "freeform",
            "prefer_websockets": true,
        },
        {
            "slug": "gpt-7-luna", "visibility": "list", "priority": 1,
            "context_window": 128_000,
            "supported_reasoning_levels": [{"effort": "low"}],
            "default_reasoning_level": "low",
        },
        {"slug": "gpt-7-review", "visibility": "hide", "priority": 0, "context_window": 1},
    ]})
}

/// Writes a cache file of `list` from the backend at `base_url` to `path`.
fn write_cache_file(path: &Path, base_url: &str, list: &Value) {
    let mut file = json!({
        "version": 1,
        "base_url": base_url,
        "client_version": CLIENT_VERSION,
        "fetched_at": "2026-10-04T10:00:00Z",
        "etag": "\"cached\"",
    });
    file["models"] = list["models"].clone();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&file).unwrap()).unwrap();
}

/// The catalog of [`backend_list`], as a cache of the default backend.
fn cached_catalog() -> Catalog {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CATALOG_FILE);
    write_cache_file(&path, efr_provider_openai::SUBSCRIPTION_BASE_URL, &backend_list());
    efr_provider_openai::read_cache(
        &path,
        Backend::Subscription,
        efr_provider_openai::SUBSCRIPTION_BASE_URL,
    )
    .unwrap()
    .unwrap()
}

fn builtin() -> Catalog {
    Catalog::builtin(Backend::Subscription)
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

fn window(models: &[ModelInfo], id: &str) -> Option<u64> {
    models.iter().find(|model| model.id == id).unwrap().context_window
}

fn limits(id: &str, window: u64) -> efr_config::ModelEntry {
    let mut limits = ModelLimits::new(id);
    limits.context_window = Some(window);
    limits.into()
}

#[test]
fn the_default_is_the_configured_model_then_the_best_priority_then_the_first_configured() {
    let mut settings = Settings::default();
    assert_eq!(default_model(&settings, &builtin()), "gpt-6.1-sol");
    assert_eq!(default_model(&settings, &cached_catalog()), "gpt-7-luna");

    settings.openai.models = Some(vec!["gpt-next".into()]);
    assert_eq!(
        default_model(&settings, &cached_catalog()),
        "gpt-7-luna",
        "a configured list no longer picks the default"
    );

    settings.model.name = Some("gpt-7-sol".to_owned());
    assert_eq!(default_model(&settings, &cached_catalog()), "gpt-7-sol");
}

#[test]
fn the_effective_list_is_the_catalog_then_the_configured_ids() {
    let mut settings = Settings::default();
    settings.openai.models = Some(vec!["gpt-next".into(), "gpt-7-sol".into()]);

    let (models, clamps) = effective_models(&settings, &cached_catalog());

    assert_eq!(ids(&models), ["gpt-7-luna", "gpt-7-sol", "gpt-next"], "hidden models stay out");
    assert!(clamps.is_empty());
    assert_eq!(
        models[1],
        ModelInfo {
            id: "gpt-7-sol".to_owned(),
            efforts: vec!["low".to_owned(), "high".to_owned()],
            default_effort: Some("low".to_owned()),
            default: false,
            source: ModelSource::Builtin,
            context_window: Some(300_000),
            max_context_window: Some(900_000),
            prefer_websockets: true,
        }
    );
    assert_eq!(models[2].source, ModelSource::Config);
    assert_eq!(models.iter().filter(|model| model.default).count(), 1);
    assert!(models[0].default);
}

#[test]
fn a_configured_window_raises_or_lowers_the_window_up_to_the_largest() {
    let mut settings = Settings::default();
    settings.openai.models = Some(vec![
        limits("gpt-6.1-sol", 600_000),
        limits("gpt-6-sol", 100_000),
        limits("gpt-6-luna", 900_000),
        limits("gpt-5.5", 300_000),
        limits("gpt-next", 2_000_000),
    ]);

    let (models, clamps) = effective_models(&settings, &builtin());

    assert_eq!(window(&models, "gpt-6.1-sol"), Some(600_000), "raised");
    assert_eq!(window(&models, "gpt-6-sol"), Some(100_000), "lowered");
    assert_eq!(window(&models, "gpt-6-luna"), Some(872_000), "cut down to the largest");
    assert_eq!(window(&models, "gpt-5.5"), Some(272_000), "gpt-5.5 cannot grow");
    assert_eq!(window(&models, "gpt-next"), Some(2_000_000), "an unknown model has no limit");
    assert_eq!(
        clamps,
        [
            Clamp { model: "gpt-6-luna".to_owned(), asked: 900_000, max: 872_000 },
            Clamp { model: "gpt-5.5".to_owned(), asked: 300_000, max: 272_000 },
        ]
    );
}

#[test]
fn a_clamp_is_warned_about_once() {
    let mut settings = Settings::default();
    settings.openai.models = Some(vec![limits("gpt-6-luna", 900_000)]);
    let models = Models::fixed(ModelCatalog::new(builtin()));

    let first = models.effective(&settings);
    let second = models.effective(&settings);

    assert_eq!(first, second);
    assert_eq!(window(&first, "gpt-6-luna"), Some(872_000));
    assert_eq!(models.warned.lock().unwrap().len(), 1);
}

#[test]
fn the_status_names_the_origin_and_the_time() {
    let builtin = status(&builtin());
    assert_eq!(builtin.origin, WireOrigin::Builtin);
    assert_eq!(builtin.fetched_at, None);
    let cached = status(&cached_catalog());
    assert_eq!(cached.origin, WireOrigin::Cache);
    assert_eq!(cached.fetched_at, Some("2026-10-04T10:00:00Z".parse().unwrap()));
}

#[tokio::test]
async fn an_offline_start_reads_the_cache_then_falls_back_to_the_builtin_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join(CATALOG_FILE);
    let settings = Settings::default();

    assert_eq!(load(&settings, &path).await.origin(), CatalogOrigin::Builtin, "no cache");

    write_cache_file(&path, efr_provider_openai::SUBSCRIPTION_BASE_URL, &backend_list());
    let cached = load(&settings, &path).await;
    assert_eq!(cached.origin(), CatalogOrigin::Cache);
    assert_eq!(cached.default_model().as_deref(), Some("gpt-7-luna"));

    let mut elsewhere = Settings::default();
    elsewhere.openai.subscription_base_url = Some("http://127.0.0.1:9/codex".to_owned());
    assert_eq!(
        load(&elsewhere, &path).await.origin(),
        CatalogOrigin::Builtin,
        "a cache of another backend is not used"
    );

    let mut api = Settings::default();
    api.model.provider = API.to_owned();
    assert_eq!(
        load(&api, &path).await.origin(),
        CatalogOrigin::Builtin,
        "the cache of the subscription is not the API's"
    );

    std::fs::write(&path, "{ broken").unwrap();
    assert_eq!(load(&settings, &path).await.origin(), CatalogOrigin::Builtin, "a broken file");

    let nothing = json!({"models": [{"slug": "gpt-8", "apply_patch_tool_type": "grammar"}]});
    write_cache_file(&path, efr_provider_openai::SUBSCRIPTION_BASE_URL, &nothing);
    assert_eq!(
        load(&settings, &path).await.origin(),
        CatalogOrigin::Builtin,
        "a cache that offers no model"
    );
}

/// A token source without a login.
#[derive(Debug)]
struct NoLogin;

#[async_trait]
impl TokenSource for NoLogin {
    async fn access_token(&self) -> Result<AccessToken, ProviderError> {
        Err(ProviderError::NotLoggedIn)
    }

    async fn invalidate(&self) {}
}

struct Fetching {
    models: Arc<Models>,
    clock: TestClock,
    cache: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn fetching(server: &MockServer, start: Catalog, tokens: Arc<dyn TokenSource>) -> Fetching {
    let clock = TestClock::new();
    let http =
        HttpClient::new(&HttpConfig::default(), clock.shared(), Arc::new(TestRng::new(1))).unwrap();
    let config = OpenAiConfig::subscription()
        .with_base_url(&format!("{}/backend-api/codex", server.uri()))
        .unwrap()
        .with_retry(RetryPolicy::none());
    let client = CatalogClient::new(config, http, tokens, clock.shared());
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("state").join(CATALOG_FILE);
    let models = Models::fetched(ModelCatalog::new(start), client, cache.clone(), clock.shared());
    Fetching { models: Arc::new(models), clock, cache, _dir: dir }
}

fn token() -> Arc<dyn TokenSource> {
    Arc::new(StaticToken::new(SecretString::from("sk-catalog")))
}

fn base_url(server: &MockServer) -> String {
    format!("{}/backend-api/codex", server.uri())
}

#[tokio::test]
async fn a_fetched_list_applies_and_goes_to_the_cache_and_a_304_keeps_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(query_param("client_version", CLIENT_VERSION))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .set_body_json(backend_list()),
        )
        .mount(&server)
        .await;
    let setup = fetching(&server, builtin(), token());
    let refresh = setup.models.refresh.as_ref().unwrap();

    let wait = setup.models.fetch(refresh).await;

    assert_eq!(wait, REFRESH_INTERVAL);
    let current = setup.models.current();
    assert_eq!(current.origin(), CatalogOrigin::Backend);
    assert_eq!(current.default_model().as_deref(), Some("gpt-7-luna"));
    let cached =
        efr_provider_openai::read_cache(&setup.cache, Backend::Subscription, &base_url(&server))
            .unwrap()
            .unwrap();
    assert_eq!(cached.etag(), Some("\"v1\""));
    assert_eq!(cached.models(), current.models());

    setup.clock.advance(Duration::from_secs(3600));
    let wait = setup.models.fetch(refresh).await;

    assert_eq!(wait, REFRESH_INTERVAL);
    let confirmed = setup.models.current();
    assert_eq!(confirmed.models(), current.models(), "a 304 keeps the list");
    assert_eq!(confirmed.fetched_at(), Some(setup.clock.now()), "with the time of the answer");
    let cached =
        efr_provider_openai::read_cache(&setup.cache, Backend::Subscription, &base_url(&server))
            .unwrap()
            .unwrap();
    assert_eq!(cached.fetched_at(), Some(setup.clock.now()));
    let seen = server.received_requests().await.unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].headers.get("if-none-match").unwrap(), "\"v1\"");
}

#[tokio::test]
async fn a_failed_fetch_keeps_the_list_and_tries_again_soon() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let setup = fetching(&server, cached_catalog(), token());
    let refresh = setup.models.refresh.as_ref().unwrap();

    let wait = setup.models.fetch(refresh).await;

    assert_eq!(wait, Duration::from_secs(15));
    assert_eq!(setup.models.current().origin(), CatalogOrigin::Cache, "the cache stays");
    assert!(!setup.cache.exists(), "nothing is written");
}

#[test]
fn the_wait_after_a_failed_fetch_grows_to_five_minutes() {
    let waits: Vec<u64> = (1..=7).map(|failures| retry_wait(failures).as_secs()).collect();
    assert_eq!(waits, [15, 30, 60, 120, 300, 300, 300]);
    assert_eq!(retry_wait(u32::MAX), Duration::from_secs(300));
}

#[tokio::test]
async fn failed_fetches_wait_longer_each_time_and_a_good_one_starts_over() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(5)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(backend_list()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let setup = fetching(&server, builtin(), token());
    let refresh = setup.models.refresh.as_ref().unwrap();

    let mut waits = Vec::new();
    for _ in 0..7 {
        waits.push(setup.models.fetch(refresh).await.as_secs());
    }

    assert_eq!(waits, [15, 30, 60, 120, 300, REFRESH_INTERVAL.as_secs(), 15]);
    assert_eq!(setup.models.current().origin(), CatalogOrigin::Backend, "the good list stays");
}

#[tokio::test]
async fn without_a_login_nothing_is_asked_until_the_next_round() {
    let server = MockServer::start().await;
    let setup = fetching(&server, builtin(), Arc::new(NoLogin));
    let refresh = setup.models.refresh.as_ref().unwrap();

    assert_eq!(setup.models.fetch(refresh).await, REFRESH_INTERVAL);
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(setup.models.current().origin(), CatalogOrigin::Builtin);
}

#[tokio::test]
async fn a_list_that_offers_nothing_that_efr_can_use_keeps_the_current_one() {
    let server = MockServer::start().await;
    let unusable = json!({"models": [
        {"slug": "gpt-8", "visibility": "list", "apply_patch_tool_type": "grammar"},
    ]});
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(unusable))
        .mount(&server)
        .await;
    let setup = fetching(&server, builtin(), token());
    let refresh = setup.models.refresh.as_ref().unwrap();

    setup.models.fetch(refresh).await;

    assert_eq!(setup.models.current().origin(), CatalogOrigin::Builtin);
    assert!(!setup.cache.exists());
}

#[tokio::test]
async fn the_task_fetches_at_start_and_again_when_asked() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(backend_list()))
        .mount(&server)
        .await;
    let setup = fetching(&server, builtin(), token());
    let stop = CancellationToken::new();
    let task = tokio::spawn(Arc::clone(&setup.models).serve(stop.clone()));
    let requests = || async { server.received_requests().await.unwrap().len() };

    Wait::new("the first fetch")
        .until_some_async(async || (requests().await == 1).then_some(()))
        .await
        .unwrap();
    setup.models.refresh_now();
    Wait::new("the fetch after a login")
        .until_some_async(async || (requests().await == 2).then_some(()))
        .await
        .unwrap();
    Wait::new("the hourly wait").until(|| setup.clock.pending_sleeps() == 1).await.unwrap();
    assert_eq!(setup.clock.requested_sleeps().last(), Some(&REFRESH_INTERVAL));
    setup.clock.advance(REFRESH_INTERVAL);
    Wait::new("the hourly fetch")
        .until_some_async(async || (requests().await == 3).then_some(()))
        .await
        .unwrap();

    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn a_fetch_that_hangs_never_holds_up_a_reader() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3600)))
        .mount(&server)
        .await;
    let setup = fetching(&server, builtin(), token());
    let stop = CancellationToken::new();
    let task = tokio::spawn(Arc::clone(&setup.models).serve(stop.clone()));
    Wait::new("the fetch that hangs")
        .until_some_async(async || {
            (!server.received_requests().await.unwrap().is_empty()).then_some(())
        })
        .await
        .unwrap();

    let settings = Settings::default();
    let models = setup.models.effective(&settings);

    assert_eq!(models.first().map(|model| model.id.as_str()), Some("gpt-6.1-sol"));
    assert_eq!(setup.models.default_model(&settings), "gpt-6.1-sol");
    stop.cancel();
    task.await.unwrap();
}
