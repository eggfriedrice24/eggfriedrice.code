use std::sync::Arc;

use efr_http::{HttpClient, HttpConfig};
use efr_provider::{ProviderError, SecretString};
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CatalogClient, check_key};
use crate::testing::{
    FakeTokens, FixedRng, InstantClock, KEY, closed_base_url, model_entry, models_page, start,
};
use crate::{AnthropicConfig, CatalogOrigin, DEFAULT_MODEL};

const MODELS_PATH: &str = "/v1/models";

fn http(clock: Arc<InstantClock>) -> HttpClient {
    HttpClient::new(&HttpConfig::default(), clock, Arc::new(FixedRng(0))).unwrap()
}

fn config(server: &MockServer) -> AnthropicConfig {
    AnthropicConfig::new().with_base_url(&format!("{}/v1", server.uri())).unwrap()
}

fn client(config: AnthropicConfig, tokens: FakeTokens) -> CatalogClient {
    let clock = Arc::new(InstantClock::new());
    CatalogClient::new(config, http(clock.clone()), Arc::new(tokens), clock, Arc::new(FixedRng(0)))
}

fn refusal(status: u16, kind: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "type": "error",
        "error": {"type": kind, "message": message},
        "request_id": "req_011CVyqRZ",
    }))
}

#[tokio::test]
async fn the_fetch_follows_the_pages_and_keeps_the_active_models() {
    let server = MockServer::start().await;
    let mut deprecated = model_entry("claude-sonnet-4-5");
    deprecated["lifecycle"] = json!("deprecated");
    let first = [model_entry("claude-fable-5-1"), model_entry(DEFAULT_MODEL)];
    let second = [deprecated, model_entry("claude-haiku-5-5")];
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(query_param("limit", "1000"))
        .and(query_param("after_id", DEFAULT_MODEL))
        .respond_with(ResponseTemplate::new(200).set_body_json(models_page(&second, false)))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(query_param("limit", "1000"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("anthropic-workspace-id", "wrkspc_01JXR"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(models_page(&first, true)))
        .expect(1)
        .mount(&server)
        .await;
    let config = config(&server).with_workspace_id("wrkspc_01JXR").unwrap();
    let client = client(config, FakeTokens::key());

    let catalog = client.fetch().await.unwrap();

    let ids: Vec<String> = catalog.models().into_iter().map(|model| model.id).collect();
    assert_eq!(ids, ["claude-fable-5-1", DEFAULT_MODEL, "claude-haiku-5-5"]);
    assert_eq!(catalog.left_out(), 1);
    assert_eq!(catalog.origin(), CatalogOrigin::Backend);
    assert_eq!(catalog.fetched_at(), start());
    assert_eq!(catalog.default_model().as_deref(), Some(DEFAULT_MODEL));
    assert_eq!(client.base_url(), format!("{}/v1", server.uri()));
    let seen = server.received_requests().await.unwrap();
    assert_eq!(seen[0].headers.get("anthropic-beta"), None, "no beta for the list");
}

#[tokio::test]
async fn a_page_that_names_no_next_one_fails_the_fetch() {
    let server = MockServer::start().await;
    let mut page = models_page(&[model_entry(DEFAULT_MODEL)], true);
    page["last_id"] = json!(null);
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(page))
        .mount(&server)
        .await;
    let client = client(config(&server), FakeTokens::key());

    let error = client.fetch().await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: None, .. }), "{error:?}");
}

#[tokio::test]
async fn a_page_that_names_itself_again_fails_the_fetch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(models_page(&[model_entry(DEFAULT_MODEL)], true)),
        )
        .mount(&server)
        .await;
    let client = client(config(&server), FakeTokens::key());

    let error = client.fetch().await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: None, .. }), "{error:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_body_without_data_is_not_a_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": []})))
        .mount(&server)
        .await;
    let client = client(config(&server), FakeTokens::key());

    let error = client.fetch().await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: None, .. }), "{error:?}");
}

#[tokio::test]
async fn a_body_that_is_not_json_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    let client = client(config(&server), FakeTokens::key());

    let error = client.fetch().await.unwrap_err();

    assert!(matches!(error, ProviderError::Decode { .. }), "{error:?}");
}

#[tokio::test]
async fn a_refused_key_fails_the_fetch_at_once_with_the_servers_message() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(refusal(401, "authentication_error", "invalid x-api-key"))
        .mount(&server)
        .await;
    let client = client(config(&server), FakeTokens::key());

    let error = client.fetch().await.unwrap_err();

    assert!(
        matches!(&error, ProviderError::Unauthorized { message: Some(text) } if text == "invalid x-api-key"),
        "{error:?}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_server_error_fails_the_fetch_with_its_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(refusal(503, "api_error", "unavailable"))
        .mount(&server)
        .await;
    let client = client(config(&server), FakeTokens::key());

    let error = client.fetch().await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: Some(503), .. }), "{error:?}");
}

#[tokio::test]
async fn an_overloaded_api_is_asked_again_for_the_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(refusal(529, "overloaded_error", "Overloaded"))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(models_page(&[model_entry(DEFAULT_MODEL)], false)),
        )
        .mount(&server)
        .await;
    let clock = Arc::new(InstantClock::new());
    let client = CatalogClient::new(
        config(&server),
        http(clock.clone()),
        Arc::new(FakeTokens::key()),
        clock.clone(),
        Arc::new(FixedRng(0)),
    );

    let catalog = client.fetch().await.unwrap();

    assert_eq!(catalog.default_model().as_deref(), Some(DEFAULT_MODEL));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(clock.sleeps().len(), 2, "a backoff before each new try");
}

#[tokio::test]
async fn a_spend_cap_fails_the_fetch_after_one_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "type": "error",
            "error": {
                "type": "rate_limit_error",
                "message": "This request would exceed your organization's spend limit.",
                "details": {"error_code": "enforced_spend_limit_reached"},
            },
        })))
        .mount(&server)
        .await;
    let clock = Arc::new(InstantClock::new());
    let client = CatalogClient::new(
        config(&server),
        http(clock.clone()),
        Arc::new(FakeTokens::key()),
        clock.clone(),
        Arc::new(FixedRng(0)),
    );

    let error = client.fetch().await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: Some(429), .. }), "{error:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(clock.sleeps().is_empty());
}

#[tokio::test]
async fn a_list_answer_that_quotes_the_key_never_puts_it_in_the_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(refusal(401, "authentication_error", &format!("Bearer {KEY} is not valid")))
        .mount(&server)
        .await;
    let client = client(config(&server), FakeTokens::key());

    let error = client.fetch().await.unwrap_err();

    assert!(
        matches!(&error, ProviderError::Unauthorized { message: Some(text) } if text == "Bearer <the key> is not valid"),
        "{error:?}"
    );
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(KEY), "{shown}");
}

#[tokio::test]
async fn without_a_key_nothing_is_asked() {
    let server = MockServer::start().await;
    let client = client(config(&server), FakeTokens::none());

    let error = client.fetch().await.unwrap_err();

    assert!(matches!(error, ProviderError::NotLoggedIn), "{error:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn key() -> SecretString {
    SecretString::from(KEY)
}

#[tokio::test]
async fn a_key_that_lists_one_model_is_good() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .and(query_param("limit", "1"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("anthropic-workspace-id", "wrkspc_01JXR"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(models_page(&[model_entry(DEFAULT_MODEL)], true)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let config = config(&server).with_workspace_id("wrkspc_01JXR").unwrap();

    check_key(&http(Arc::new(InstantClock::new())), &config, &key()).await.unwrap();
}

#[tokio::test]
async fn a_refused_key_says_why_without_the_key() {
    let cases = [
        (
            refusal(401, "authentication_error", "invalid x-api-key"),
            "unauthorized: invalid x-api-key",
        ),
        (
            refusal(403, "permission_error", "This key cannot list models."),
            "api 403: This key cannot list models.",
        ),
        (
            refusal(
                400,
                "invalid_request_error",
                "anthropic-workspace-id is required when authenticating with an identity-linked API key",
            ),
            "api 400: anthropic-workspace-id is required when authenticating with an identity-linked API key",
        ),
        (ResponseTemplate::new(500), "api 500: Internal Server Error"),
        (refusal(429, "rate_limit_error", "slow down"), "api 429: slow down"),
    ];
    for (template, expected) in cases {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(MODELS_PATH))
            .respond_with(template)
            .mount(&server)
            .await;

        let error = check_key(&http(Arc::new(InstantClock::new())), &config(&server), &key())
            .await
            .unwrap_err();

        let shown = match &error {
            ProviderError::Unauthorized { message: Some(message) } => {
                format!("unauthorized: {message}")
            }
            ProviderError::Api { status: Some(status), message, .. } => {
                format!("api {status}: {message}")
            }
            other => format!("{other:?}"),
        };
        assert_eq!(shown, expected);
        let text = format!("{error} {error:?}");
        assert!(!text.contains(KEY), "{text}");
    }
}

#[tokio::test]
async fn a_key_check_without_an_answer_fails_in_transit_at_once() {
    let config = AnthropicConfig::new().with_base_url(&closed_base_url()).unwrap();
    let clock = Arc::new(InstantClock::new());

    let error = check_key(&http(clock.clone()), &config, &key()).await.unwrap_err();

    assert!(matches!(error, ProviderError::Transport { .. }), "{error:?}");
    assert!(clock.sleeps().is_empty(), "a person waits, so the check goes once");
    let text = format!("{error} {error:?}");
    assert!(!text.contains(KEY), "{text}");
}

#[tokio::test]
async fn a_key_check_goes_once_and_hides_a_key_that_the_answer_quotes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(MODELS_PATH))
        .respond_with(refusal(529, "overloaded_error", &format!("Overloaded for {KEY}")))
        .expect(1)
        .mount(&server)
        .await;
    let clock = Arc::new(InstantClock::new());

    let error = check_key(&http(clock.clone()), &config(&server), &key()).await.unwrap_err();

    match &error {
        ProviderError::Api { status: Some(529), message, .. } => {
            assert_eq!(message, "Overloaded for <the key>");
        }
        other => panic!("{other:?}"),
    }
    assert!(clock.sleeps().is_empty());
}
