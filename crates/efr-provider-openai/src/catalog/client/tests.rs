use std::sync::Arc;

use efr_http::{HttpClient, HttpConfig, RetryPolicy};
use efr_provider::ProviderError;
use pretty_assertions::assert_eq;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CatalogClient, Fetched};
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

    assert!(matches!(error, ProviderError::Unauthorized), "{error:?}");
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
