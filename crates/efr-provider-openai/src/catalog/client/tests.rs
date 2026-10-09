use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use efr_http::{HttpClient, HttpConfig, RetryPolicy};
use efr_provider::{AccessToken, ProviderError, SecretString, TokenSource};
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CatalogClient, Fetched, check_key};
use crate::OpenAiConfig;
use crate::catalog::{CLIENT_VERSION, Catalog, CatalogOrigin};
use crate::config::Backend;
use crate::testing::{FakeTokens, FixedRng, InstantClock, catalog_fixture, start};

const MODELS_PATH: &str = "/backend-api/codex/models";

struct Setup {
    client: CatalogClient,
    tokens: Arc<FakeTokens>,
}

fn setup(server: &MockServer, tokens: FakeTokens) -> Setup {
    let clock = Arc::new(InstantClock::new());
    let http =
        HttpClient::new(&HttpConfig::default(), clock.clone(), Arc::new(FixedRng(0))).unwrap();
    let config = OpenAiConfig::subscription()
        .with_base_url(&format!("{}/backend-api/codex", server.uri()))
        .unwrap()
        .with_originator("efr")
        .unwrap()
        .with_retry(RetryPolicy::none());
    let tokens = Arc::new(tokens);
    Setup { client: CatalogClient::new(config, http, tokens.clone(), clock), tokens }
}

fn tokens() -> FakeTokens {
    FakeTokens::new(&["eyJ.access.one", "eyJ.access.two"]).with_account_id("acct_7d1f")
}

fn catalog_answer(etag: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("etag", etag)
        .set_body_raw(catalog_fixture("models.json"), "application/json")
}

#[tokio::test]
async fn the_fetch_names_efr_and_its_own_version() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(query_param("client_version", CLIENT_VERSION))
        .and(header("authorization", "Bearer eyJ.access.one"))
        .and(header("originator", "efr"))
        .and(header("chatgpt-account-id", "acct_7d1f"))
        .respond_with(catalog_answer("\"v1\""))
        .expect(1)
        .mount(&server)
        .await;
    let setup = setup(&server, tokens());

    let fetched = setup.client.fetch(&Catalog::builtin(Backend::Subscription)).await.unwrap();

    let Fetched::Changed(catalog) = fetched else { panic!("{fetched:?}") };
    assert_eq!(catalog.origin(), CatalogOrigin::Backend);
    assert_eq!(catalog.fetched_at(), Some(start()));
    assert_eq!(catalog.etag(), Some("\"v1\""));
    assert_eq!(catalog.default_model().as_deref(), Some("gpt-7-luna"));
    let seen = server.received_requests().await.unwrap();
    assert!(seen[0].headers.get("if-none-match").is_none(), "the built-in table has no tag");
    let user_agent = seen[0].headers.get("user-agent").unwrap().to_str().unwrap();
    assert!(user_agent.starts_with("efr/"), "{user_agent}");
}

#[tokio::test]
async fn a_known_list_goes_with_its_tag_and_a_304_confirms_it() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(catalog_answer("\"v1\""))
        .mount(&server)
        .await;
    let setup = setup(&server, tokens());

    let Fetched::Changed(first) =
        setup.client.fetch(&Catalog::builtin(Backend::Subscription)).await.unwrap()
    else {
        panic!("no list");
    };
    let second = setup.client.fetch(&first).await.unwrap();

    assert_eq!(second, Fetched::NotModified);
}

#[tokio::test]
async fn a_refused_token_is_refreshed_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(header("authorization", "Bearer eyJ.access.one"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(header("authorization", "Bearer eyJ.access.two"))
        .respond_with(catalog_answer("\"v2\""))
        .mount(&server)
        .await;
    let setup = setup(&server, tokens());

    let fetched = setup.client.fetch(&Catalog::builtin(Backend::Subscription)).await.unwrap();

    assert!(matches!(fetched, Fetched::Changed(_)), "{fetched:?}");
    assert_eq!(setup.tokens.invalidations(), 1);
}

#[tokio::test]
async fn a_second_refusal_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(401))
        .expect(2)
        .mount(&server)
        .await;
    let setup = setup(&server, tokens());

    let error = setup.client.fetch(&Catalog::builtin(Backend::Subscription)).await.unwrap_err();

    assert!(matches!(error, ProviderError::Unauthorized { .. }), "{error:?}");
}

#[tokio::test]
async fn a_refusal_of_a_key_that_cannot_refresh_is_unauthorized_at_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let setup = setup(&server, tokens().without_refresh());

    let error = setup.client.fetch(&Catalog::builtin(Backend::Subscription)).await.unwrap_err();

    assert!(matches!(error, ProviderError::Unauthorized { .. }), "{error:?}");
    assert_eq!(setup.tokens.invalidations(), 0);
}

#[tokio::test]
async fn an_error_answer_or_a_body_without_models_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_raw("{\"data\": []}", "application/json"))
        .mount(&server)
        .await;
    let setup = setup(&server, tokens());
    let builtin = Catalog::builtin(Backend::Subscription);

    let error = setup.client.fetch(&builtin).await.unwrap_err();
    assert!(matches!(error, ProviderError::Api { status: Some(503), .. }), "{error:?}");
    let error = setup.client.fetch(&builtin).await.unwrap_err();
    assert!(matches!(error, ProviderError::Api { status: Some(200), .. }), "{error:?}");
}

#[tokio::test]
async fn without_a_login_nothing_is_sent() {
    let server = MockServer::start().await;
    let setup = setup(&server, FakeTokens::new(&[]));

    let error = setup.client.fetch(&Catalog::builtin(Backend::Subscription)).await.unwrap_err();

    assert!(matches!(error, ProviderError::NotLoggedIn), "{error:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn the_url_keeps_the_base_path() {
    let clock = Arc::new(InstantClock::new());
    let http =
        HttpClient::new(&HttpConfig::default(), clock.clone(), Arc::new(FixedRng(0))).unwrap();
    let config = OpenAiConfig::subscription();
    let client = CatalogClient::new(config, http, Arc::new(tokens()), clock);
    assert_eq!(
        client.url().unwrap().as_str(),
        format!("https://chatgpt.com/backend-api/codex/models?client_version={CLIENT_VERSION}")
    );
    assert_eq!(client.base_url(), "https://chatgpt.com/backend-api/codex");
}

#[tokio::test]
async fn after_a_second_refusal_a_refusal_forces_no_refresh_until_a_fetch_works() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(catalog_answer("\"v1\""))
        .mount(&server)
        .await;
    let tokens = FakeTokens::new(&["eyJ.access.one", "eyJ.access.two", "eyJ.access.three"])
        .with_account_id("acct_7d1f");
    let setup = setup(&server, tokens);
    let builtin = Catalog::builtin(Backend::Subscription);

    let first = setup.client.fetch(&builtin).await.unwrap_err();
    let second = setup.client.fetch(&builtin).await.unwrap_err();

    assert!(matches!(first, ProviderError::Unauthorized { .. }), "{first:?}");
    assert!(matches!(second, ProviderError::Unauthorized { .. }), "{second:?}");
    // The first fetch refreshed once; the second one sent one request and no refresh.
    assert_eq!(setup.tokens.invalidations(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);

    // A fetch that works lets the next 401 refresh again.
    setup.client.fetch(&builtin).await.unwrap();
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(401))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&server)
        .await;
    setup.client.fetch(&builtin).await.unwrap();
    assert_eq!(setup.tokens.invalidations(), 2);
}

/// A source whose token a model call refreshed while a fetch was on its way: the first
/// read gives the old token, every later read the new one.
#[derive(Debug, Default)]
struct RefreshedMeanwhile {
    reads: AtomicUsize,
    invalidations: AtomicUsize,
}

#[async_trait::async_trait]
impl TokenSource for RefreshedMeanwhile {
    async fn access_token(&self) -> Result<AccessToken, ProviderError> {
        let token = match self.reads.fetch_add(1, Ordering::SeqCst) {
            0 => "eyJ.access.old",
            _ => "eyJ.access.new",
        };
        Ok(AccessToken::new(SecretString::from(token)).with_account_id("acct_7d1f"))
    }

    async fn invalidate(&self) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_refusal_of_a_token_that_was_refreshed_meanwhile_keeps_the_new_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(header("authorization", "Bearer eyJ.access.old"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(header("authorization", "Bearer eyJ.access.new"))
        .respond_with(catalog_answer("\"v1\""))
        .mount(&server)
        .await;
    let setup = setup(&server, tokens());
    let source = Arc::new(RefreshedMeanwhile::default());
    let client = CatalogClient::new(
        setup.client.config.clone(),
        setup.client.http.clone(),
        source.clone(),
        Arc::new(InstantClock::new()),
    );

    let fetched = client.fetch(&Catalog::builtin(Backend::Subscription)).await.unwrap();

    assert!(matches!(fetched, Fetched::Changed(_)), "{fetched:?}");
    assert_eq!(source.invalidations.load(Ordering::SeqCst), 0);
}

const API_MODELS_PATH: &str = "/v1/models";

/// A key, as a test spells it. Only the fakes ever see it.
const KEY: &str = "sk-proj-efr-test-key-9f3c";

/// The API key backend at `server`, naming an organization and a project.
fn api_config(server: &MockServer) -> OpenAiConfig {
    OpenAiConfig::api()
        .with_base_url(&format!("{}/v1", server.uri()))
        .unwrap()
        .with_organization("org-AbC")
        .unwrap()
        .with_project("proj_AbC")
        .unwrap()
        .with_retry(RetryPolicy::none())
}

fn http() -> HttpClient {
    HttpClient::new(&HttpConfig::default(), Arc::new(InstantClock::new()), Arc::new(FixedRng(0)))
        .unwrap()
}

fn api_list(ids: &[&str]) -> ResponseTemplate {
    let data: Vec<_> = ids
        .iter()
        .map(|id| json!({ "id": id, "object": "model", "created": 1, "owned_by": "openai" }))
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({ "object": "list", "data": data }))
}

#[tokio::test]
async fn the_api_list_cuts_the_built_in_table_down_to_the_ids_of_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(API_MODELS_PATH))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("openai-organization", "org-AbC"))
        .and(header("openai-project", "proj_AbC"))
        .respond_with(api_list(&["gpt-5.5", "gpt-6-luna", "text-embedding-3-small"]))
        .expect(1)
        .mount(&server)
        .await;
    let client = CatalogClient::new(
        api_config(&server),
        http(),
        Arc::new(FakeTokens::new(&["sk-test"]).without_refresh()),
        Arc::new(InstantClock::new()),
    );

    let fetched = client.fetch(&Catalog::builtin(Backend::Api)).await.unwrap();

    let Fetched::Changed(catalog) = fetched else { panic!("{fetched:?}") };
    let ids: Vec<String> = catalog.models().into_iter().map(|model| model.id).collect();
    assert_eq!(ids, ["gpt-6-luna", "gpt-5.5"], "the table's order, only the listed ids");
    assert_eq!(catalog.origin(), CatalogOrigin::Backend);
    assert_eq!(catalog.default_model().as_deref(), Some("gpt-6-luna"));
    let seen = server.received_requests().await.unwrap();
    assert_eq!(seen[0].url.query(), None, "the API gets no client_version");
}

#[tokio::test]
async fn an_api_answer_without_a_data_list_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(API_MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "models": [] })))
        .mount(&server)
        .await;
    let client = CatalogClient::new(
        api_config(&server),
        http(),
        Arc::new(FakeTokens::new(&["sk-test"]).without_refresh()),
        Arc::new(InstantClock::new()),
    );

    let error = client.fetch(&Catalog::builtin(Backend::Api)).await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: Some(200), .. }), "{error:?}");
}

#[tokio::test]
async fn a_key_check_lists_the_models_with_the_key_and_the_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(API_MODELS_PATH))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(header("openai-organization", "org-AbC"))
        .and(header("openai-project", "proj_AbC"))
        .respond_with(api_list(&["gpt-5.5"]))
        .expect(1)
        .mount(&server)
        .await;

    check_key(&http(), &api_config(&server), &SecretString::from(KEY)).await.unwrap();
}

#[tokio::test]
async fn a_refused_key_is_unauthorized_with_the_servers_message_and_never_the_key() {
    let server = MockServer::start().await;
    let message =
        format!("Incorrect API key provided: {KEY}. You can find your API key at the dashboard.");
    let body = json!({ "error": {
        "message": message, "type": "invalid_request_error", "code": "invalid_api_key",
    }});
    Mock::given(method("GET"))
        .and(path(API_MODELS_PATH))
        .respond_with(ResponseTemplate::new(401).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;

    let error =
        check_key(&http(), &api_config(&server), &SecretString::from(KEY)).await.unwrap_err();

    let ProviderError::Unauthorized { message: Some(message) } = &error else {
        panic!("{error:?}")
    };
    assert_eq!(
        message,
        "Incorrect API key provided: <the key>. You can find your API key at the dashboard."
    );
    for shown in [error.to_string(), format!("{error:?}")] {
        assert!(!shown.contains(KEY), "{shown}");
    }
}

#[tokio::test]
async fn a_forbidden_key_or_a_server_error_is_an_api_error_with_its_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(API_MODELS_PATH))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({ "error": {
            "message": "Country, region, or territory not supported",
            "type": "request_forbidden",
            "code": "unsupported_country_region_territory",
        }})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(API_MODELS_PATH))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let config = api_config(&server);
    let key = SecretString::from(KEY);

    let forbidden = check_key(&http(), &config, &key).await.unwrap_err();
    let failed = check_key(&http(), &config, &key).await.unwrap_err();

    match forbidden {
        ProviderError::Api { status: Some(403), code, message } => {
            assert_eq!(code.as_deref(), Some("unsupported_country_region_territory"));
            assert_eq!(message, "Country, region, or territory not supported");
        }
        other => panic!("{other:?}"),
    }
    match failed {
        ProviderError::Api { status: Some(500), message, .. } => {
            assert_eq!(message, "the key check failed: 500 Internal Server Error");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_key_check_without_an_answer_is_a_transport_error() {
    // NOTE: nothing listens on the discard port of the loopback.
    let config = OpenAiConfig::api().with_base_url("http://127.0.0.1:9/v1").unwrap();

    let error = check_key(&http(), &config, &SecretString::from(KEY)).await.unwrap_err();

    assert!(matches!(error, ProviderError::Transport { .. }), "{error:?}");
    assert!(!format!("{error:?}").contains(KEY));
}
