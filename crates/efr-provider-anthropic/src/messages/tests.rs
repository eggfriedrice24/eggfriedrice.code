use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_http::{HeaderMap, HttpClient, HttpConfig, Record, Recorder, RetryPolicy};
use efr_provider::{Message, ModelInfo, Provider, ProviderError, ProviderId, Request, StopReason};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request as SeenRequest, ResponseTemplate};

use super::AnthropicProvider;
use crate::catalog::entries_of;
use crate::testing::{
    FakeTokens, FixedRng, InstantClock, KEY, LogText, closed_base_url, model_entry, sse, start,
    text_answer,
};
use crate::{AnthropicConfig, Catalog, DEFAULT_EFFORT, DEFAULT_MODEL, ModelCatalog};

const MESSAGES_PATH: &str = "/v1/messages";

struct Setup {
    provider: AnthropicProvider,
    tokens: Arc<FakeTokens>,
    clock: Arc<InstantClock>,
}

fn setup(server: &MockServer, config: AnthropicConfig, tokens: FakeTokens) -> Setup {
    setup_with(server, config, tokens, |client| client)
}

fn setup_with(
    server: &MockServer,
    config: AnthropicConfig,
    tokens: FakeTokens,
    client: impl FnOnce(HttpClient) -> HttpClient,
) -> Setup {
    let clock = Arc::new(InstantClock::new());
    let rng = Arc::new(FixedRng(0));
    let http = HttpClient::new(&HttpConfig::default(), clock.clone(), rng.clone()).unwrap();
    let config = config.with_base_url(&format!("{}/v1", server.uri())).unwrap();
    let tokens = Arc::new(tokens);
    let provider = AnthropicProvider::new(
        ProviderId::new("anthropic-test").unwrap(),
        config,
        client(http),
        tokens.clone(),
        clock.clone(),
        rng,
    )
    .with_catalog(listed());
    Setup { provider, tokens, clock }
}

/// A catalog that lists [`DEFAULT_MODEL`], as the daemon gives the provider before a
/// turn: without the model's output limit, the body cannot be built.
fn listed() -> ModelCatalog {
    let catalog = ModelCatalog::new();
    let (entries, _) = entries_of(&json!([model_entry(DEFAULT_MODEL)]));
    catalog.apply(Catalog::from_backend("https://api.anthropic.test/v1", entries, 1, start()));
    catalog
}

fn answer(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("request-id", "req_011CVyqRZ")
        .set_body_raw(text_answer(text), "text/event-stream")
}

fn error_answer(status: u16, kind: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).insert_header("request-id", "req_011CVyqRZ").set_body_json(
        json!({
            "type": "error",
            "error": {"type": kind, "message": message},
            "request_id": "req_011CVyqRZ",
        }),
    )
}

fn request(text: &str) -> Request {
    let mut request = Request::new(DEFAULT_MODEL);
    request.system = Some("You are efr.".to_owned());
    request.messages = vec![Message::user(text)];
    request
}

async fn seen(server: &MockServer) -> Vec<SeenRequest> {
    server.received_requests().await.unwrap()
}

fn body_of(request: &SeenRequest) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

async fn mount(server: &MockServer, template: ResponseTemplate) {
    Mock::given(method("POST")).and(path(MESSAGES_PATH)).respond_with(template).mount(server).await;
}

/// Mounts `first` for the first `times` requests and `then` for every later one.
async fn mount_then(
    server: &MockServer,
    first: ResponseTemplate,
    times: u64,
    then: ResponseTemplate,
) {
    Mock::given(method("POST"))
        .and(path(MESSAGES_PATH))
        .respond_with(first)
        .up_to_n_times(times)
        .mount(server)
        .await;
    mount(server, then).await;
}

#[tokio::test]
async fn a_model_call_carries_the_key_the_version_and_the_beta() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES_PATH))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("anthropic-beta", "thinking-binding-controls-2026-08-01"))
        .and(header("accept", "text/event-stream"))
        .and(header("content-type", "application/json"))
        .respond_with(answer("Your shell is zsh 5.9."))
        .expect(1)
        .mount(&server)
        .await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let completion = setup.provider.complete(request("Which shell do I use?")).await.unwrap();

    assert_eq!(completion.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(completion.stop_reason, StopReason::EndTurn);
    let usage = completion.usage.unwrap();
    assert_eq!((usage.input_tokens, usage.cached_input_tokens), (4012, 4000));
    assert_eq!(usage.output_tokens, 9, "the counts of message_delta win");
    let sent = seen(&server).await;
    assert_eq!(sent[0].headers.get("anthropic-workspace-id"), None);
    assert_eq!(sent[0].headers.get("x-api-key"), None, "one credential header only");
    let body = body_of(&sent[0]);
    assert_eq!(body["model"], json!(DEFAULT_MODEL));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["output_config"], json!({"effort": DEFAULT_EFFORT}));
}

#[tokio::test]
async fn a_workspace_id_goes_with_every_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES_PATH))
        .and(header("anthropic-workspace-id", "wrkspc_01JXR"))
        .respond_with(answer("Hi."))
        .expect(1)
        .mount(&server)
        .await;
    let config = AnthropicConfig::new().with_workspace_id("wrkspc_01JXR").unwrap();
    let setup = setup(&server, config, FakeTokens::key());

    setup.provider.complete(request("Hi")).await.unwrap();
}

#[tokio::test]
async fn no_provider_option_reaches_the_body() {
    let server = MockServer::start().await;
    mount(&server, answer("Hi.")).await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());
    let mut request = request("Hi");
    request.provider_options.insert("prompt_cache_key".to_owned(), json!("0192f0c1"));
    request.provider_options.insert("reasoning_effort".to_owned(), json!("high"));

    setup.provider.complete(request).await.unwrap();

    let text = String::from_utf8(seen(&server).await[0].body.clone()).unwrap();
    assert!(!text.contains("prompt_cache_key"), "{text}");
    assert!(!text.contains("0192f0c1"), "{text}");
    assert!(!text.contains("reasoning_effort"), "{text}");
}

#[tokio::test]
async fn the_output_limit_comes_from_the_catalog_with_the_configs_models_over_it() {
    let server = MockServer::start().await;
    mount(&server, answer("Hi.")).await;
    let config = AnthropicConfig::new().with_models(vec![
        ModelInfo::new(DEFAULT_MODEL).with_context_window(300_000),
        ModelInfo::new("claude-private-1"),
    ]);
    let provider = setup(&server, config, FakeTokens::key()).provider;

    let models = provider.models();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].context_window, Some(300_000));
    assert_eq!(models[0].max_output_tokens, Some(128_000));
    assert_eq!(models[1].id, "claude-private-1");

    provider.complete(request("Hi")).await.unwrap();
    assert_eq!(body_of(&seen(&server).await[0])["max_tokens"], json!(128_000));
}

#[tokio::test]
async fn a_401_fails_at_once_with_the_servers_message() {
    let server = MockServer::start().await;
    mount(&server, error_answer(401, "authentication_error", "invalid x-api-key")).await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    assert!(
        matches!(&error, ProviderError::Unauthorized { message: Some(text) } if text == "invalid x-api-key"),
        "{error:?}"
    );
    assert_eq!(seen(&server).await.len(), 1);
    assert!(setup.clock.sleeps().is_empty());
    assert_eq!(setup.tokens.invalidations(), 0);
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(KEY), "{shown}");
}

#[tokio::test]
async fn an_answer_that_quotes_the_key_never_puts_it_in_the_error() {
    let server = MockServer::start().await;
    let quoted = format!("Authorization: Bearer {KEY} is not valid here");
    for (status, kind) in [(401, "authentication_error"), (400, "invalid_request_error")] {
        server.reset().await;
        mount(&server, error_answer(status, kind, &quoted)).await;
        let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

        let error = setup.provider.complete(request("Hi")).await.unwrap_err();

        let shown = format!("{error} {error:?}");
        assert!(!shown.contains(KEY), "{shown}");
        assert!(shown.contains("Bearer <the key> is not valid here"), "{shown}");
    }
}

#[tokio::test]
async fn a_401_for_a_source_that_can_refresh_refreshes_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(MESSAGES_PATH))
        .and(header("authorization", "Bearer token-two"))
        .respond_with(answer("Hi."))
        .mount(&server)
        .await;
    mount(&server, error_answer(401, "authentication_error", "token expired")).await;
    let setup =
        setup(&server, AnthropicConfig::new(), FakeTokens::refreshing(&["token-one", "token-two"]));

    let completion = setup.provider.complete(request("Hi")).await.unwrap();

    assert_eq!(completion.message.text(), "Hi.");
    assert_eq!(setup.tokens.invalidations(), 1);
    assert_eq!(seen(&server).await.len(), 2);
}

#[tokio::test]
async fn a_rate_limit_is_sent_again_after_the_wait_it_names() {
    let server = MockServer::start().await;
    let limited =
        error_answer(429, "rate_limit_error", "slow down").insert_header("retry-after", "2");
    mount_then(&server, limited, 1, answer("Hi.")).await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let completion = setup.provider.complete(request("Hi")).await.unwrap();

    assert_eq!(completion.message.text(), "Hi.");
    assert_eq!(setup.clock.sleeps(), vec![Duration::from_secs(2)]);
    assert_eq!(seen(&server).await.len(), 2);
}

#[tokio::test]
async fn a_rate_limit_that_lasts_is_reported_with_its_wait() {
    let server = MockServer::start().await;
    let limited =
        error_answer(429, "rate_limit_error", "slow down").insert_header("retry-after", "120");
    mount(&server, limited).await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    assert!(
        matches!(error, ProviderError::RateLimited { retry_after: Some(wait) } if wait == Duration::from_secs(120)),
        "{error:?}"
    );
    assert_eq!(seen(&server).await.len(), 1, "a wait above the policy's longest ends the loop");
}

#[tokio::test]
async fn a_spend_cap_is_sent_once() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(429).set_body_json(json!({
            "type": "error",
            "error": {
                "type": "rate_limit_error",
                "message": "This request would exceed your organization's spend limit.",
                "details": {"error_code": "enforced_spend_limit_reached"},
            },
        })),
    )
    .await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: Some(429), .. }), "{error:?}");
    assert_eq!(seen(&server).await.len(), 1);
    assert!(setup.clock.sleeps().is_empty());
}

#[tokio::test]
async fn an_overloaded_api_is_tried_with_backoff_and_then_reported() {
    let server = MockServer::start().await;
    mount(&server, error_answer(529, "overloaded_error", "Overloaded")).await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::Overloaded { status: Some(529), message } => {
            assert_eq!(message, "Overloaded")
        }
        other => panic!("not overloaded: {other:?}"),
    }
    assert_eq!(seen(&server).await.len(), 4, "the default policy's four attempts");
    let waits = [250, 500, 1000].map(Duration::from_millis);
    assert_eq!(setup.clock.sleeps(), waits, "half of each base wait with a jitter of zero");
}

#[tokio::test]
async fn a_server_error_is_sent_again_before_the_stream_starts() {
    for (status, kind) in [(500, "api_error"), (504, "timeout_error"), (529, "overloaded_error")] {
        let server = MockServer::start().await;
        mount_then(&server, error_answer(status, kind, "try again"), 1, answer("Hi.")).await;
        let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

        let completion = setup.provider.complete(request("Hi")).await.unwrap();

        assert_eq!(completion.message.text(), "Hi.", "{status}");
        assert_eq!(seen(&server).await.len(), 2, "{status}");
        assert_eq!(setup.clock.sleeps().len(), 1, "{status}");
    }
}

#[tokio::test]
async fn a_server_error_after_the_last_try_is_an_api_error() {
    let server = MockServer::start().await;
    mount(&server, error_answer(500, "api_error", "Internal server error")).await;
    let config = AnthropicConfig::new().with_retry(RetryPolicy::none());
    let setup = setup(&server, config, FakeTokens::key());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::Api { status: Some(500), code, message } => {
            assert_eq!(code.as_deref(), Some("api_error"));
            assert_eq!(message, "Internal server error");
        }
        other => panic!("not an API error: {other:?}"),
    }
    assert_eq!(seen(&server).await.len(), 1);
}

#[tokio::test]
async fn a_refused_request_is_sent_once_in_its_class() {
    let cases = [
        (
            error_answer(
                400,
                "invalid_request_error",
                "prompt is too long: 1000001 tokens > 1000000 maximum",
            ),
            "overflow",
        ),
        (error_answer(413, "request_too_large", "Request exceeds the maximum size"), "api 413"),
        (error_answer(404, "not_found_error", "model: claude-opus-5-5"), "unknown model"),
        (error_answer(403, "permission_error", "no access"), "api 403"),
    ];
    for (template, expected) in cases {
        let server = MockServer::start().await;
        mount(&server, template).await;
        let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

        let error = setup.provider.complete(request("Hi")).await.unwrap_err();

        let class = match &error {
            ProviderError::ContextOverflow { status: Some(400), .. } => "overflow".to_owned(),
            ProviderError::Api { status: Some(status), .. } => format!("api {status}"),
            ProviderError::UnknownModel { model } if model == DEFAULT_MODEL => {
                "unknown model".to_owned()
            }
            other => format!("{other:?}"),
        };
        assert_eq!(class, expected);
        assert_eq!(seen(&server).await.len(), 1, "{expected}");
        assert!(setup.clock.sleeps().is_empty(), "{expected}");
    }
}

#[tokio::test]
async fn an_error_event_after_the_answer_started_is_never_sent_again() {
    let server = MockServer::start().await;
    let stream = sse(&[
        ("message_start", json!({"message": {"usage": {"input_tokens": 3}}})),
        ("content_block_start", json!({"index": 0, "content_block": {"type": "text", "text": ""}})),
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "text_delta", "text": "Half"}}),
        ),
        ("error", json!({"error": {"type": "overloaded_error", "message": "Overloaded"}})),
    ]);
    mount(&server, ResponseTemplate::new(200).set_body_raw(stream, "text/event-stream")).await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let events: Vec<_> = setup.provider.stream(request("Hi")).await.unwrap().collect().await;

    assert_eq!(events.len(), 2, "{events:?}");
    assert!(events[0].is_ok());
    assert!(matches!(events[1], Err(ProviderError::Overloaded { status: None, .. })), "{events:?}");
    assert_eq!(seen(&server).await.len(), 1);
    assert!(setup.clock.sleeps().is_empty());
}

#[tokio::test]
async fn a_stream_that_ends_before_message_stop_is_incomplete() {
    let server = MockServer::start().await;
    let truncated: String =
        text_answer("Hi.").split("event: message_stop").next().unwrap().to_owned();
    mount(&server, ResponseTemplate::new(200).set_body_raw(truncated, "text/event-stream")).await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let events: Vec<_> = setup.provider.stream(request("Hi")).await.unwrap().collect().await;

    assert!(matches!(events.last(), Some(Err(ProviderError::Incomplete))), "{events:?}");
}

#[tokio::test]
async fn no_key_means_no_request() {
    let server = MockServer::start().await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::none());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    assert!(matches!(error, ProviderError::NotLoggedIn), "{error:?}");
    assert!(seen(&server).await.is_empty());
}

#[tokio::test]
async fn a_call_that_cannot_connect_is_tried_again_and_fails_in_transit() {
    let clock = Arc::new(InstantClock::new());
    let rng = Arc::new(FixedRng(0));
    let http = HttpClient::new(&HttpConfig::default(), clock.clone(), rng.clone()).unwrap();
    let config = AnthropicConfig::new().with_base_url(&closed_base_url()).unwrap();
    let id = ProviderId::new("anthropic-test").unwrap();
    let provider =
        AnthropicProvider::new(id, config, http, Arc::new(FakeTokens::key()), clock.clone(), rng)
            .with_catalog(listed());

    let error = provider.complete(request("Hi")).await.unwrap_err();

    assert!(matches!(error, ProviderError::Transport { .. }), "{error:?}");
    assert_eq!(clock.sleeps().len(), 3, "no connection, so the server never saw it");
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(KEY), "{shown}");
}

#[derive(Debug, Default)]
struct HeaderRecorder {
    requests: Mutex<Vec<HeaderMap>>,
}

impl Recorder for HeaderRecorder {
    fn record(&self, record: &Record<'_>) {
        if let Record::Request { headers, .. } = record {
            self.requests.lock().unwrap().push((*headers).clone());
        }
    }
}

#[tokio::test]
async fn a_recorded_request_shows_no_key() {
    let server = MockServer::start().await;
    mount(&server, answer("Hi.")).await;
    let recorder = Arc::new(HeaderRecorder::default());
    let for_client = recorder.clone();
    let config = AnthropicConfig::new().with_workspace_id("wrkspc_01JXR").unwrap();
    let setup =
        setup_with(&server, config, FakeTokens::key(), |client| client.with_recorder(for_client));

    setup.provider.complete(request("Hi")).await.unwrap();

    let requests = recorder.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["authorization"], "Bearer [REDACTED]");
    assert_eq!(requests[0]["anthropic-version"], "2023-06-01");
    assert_eq!(requests[0]["anthropic-workspace-id"], "wrkspc_01JXR");
    assert!(!format!("{requests:?}").contains(KEY));
}

#[tokio::test]
async fn the_log_names_the_request_and_the_phases_but_never_the_key() {
    let server = MockServer::start().await;
    mount_then(&server, error_answer(529, "overloaded_error", "Overloaded"), 1, answer("Hi."))
        .await;
    let log = LogText::default();
    let _guard = tracing::subscriber::set_default(log.subscriber());
    let answered = setup(&server, AnthropicConfig::new(), FakeTokens::key());
    let refused = MockServer::start().await;
    mount(&refused, error_answer(401, "authentication_error", "invalid x-api-key")).await;
    let refusing = setup(&refused, AnthropicConfig::new(), FakeTokens::key());

    answered.provider.complete(request("Hi")).await.unwrap();
    refusing.provider.complete(request("Hi")).await.unwrap_err();

    let text = log.text();
    assert!(text.contains("request_id=\"req_011CVyqRZ\""), "{text}");
    assert!(text.contains("the provider refused the model call"), "{text}");
    assert!(text.contains("phase=provider_accepted transport=http"), "{text}");
    assert!(text.contains("phase=provider_first_event transport=http"), "{text}");
    assert!(!text.contains(KEY), "{text}");
}

#[tokio::test]
async fn a_model_without_a_known_output_limit_is_never_called() {
    let server = MockServer::start().await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::key());

    let error = setup.provider.complete(Request::new("claude-unlisted-1")).await.unwrap_err();

    assert!(
        matches!(&error, ProviderError::UnknownModel { model } if model == "claude-unlisted-1"),
        "{error:?}"
    );
    assert!(seen(&server).await.is_empty());
}

#[tokio::test]
async fn an_unlisted_model_without_a_login_is_not_logged_in() {
    let server = MockServer::start().await;
    let setup = setup(&server, AnthropicConfig::new(), FakeTokens::none());

    let error = setup.provider.complete(Request::new("claude-unlisted-1")).await.unwrap_err();

    assert!(matches!(error, ProviderError::NotLoggedIn), "{error:?}");
    assert!(seen(&server).await.is_empty());
}

#[tokio::test]
async fn the_provider_names_itself_and_has_no_model_without_a_list() {
    let clock = Arc::new(InstantClock::new());
    let rng = Arc::new(FixedRng(0));
    let http = HttpClient::new(&HttpConfig::default(), clock.clone(), rng.clone()).unwrap();
    let id = ProviderId::new("anthropic-test").unwrap();
    let provider = AnthropicProvider::new(
        id,
        AnthropicConfig::new(),
        http,
        Arc::new(FakeTokens::key()),
        clock,
        rng,
    );

    assert_eq!(provider.id().as_str(), "anthropic-test");
    assert!(provider.models().is_empty());
    assert!(provider.config().base_url().ends_with("/v1"));
    let debug = format!("{provider:?}");
    assert!(debug.contains("AnthropicProvider"), "{debug}");
}
