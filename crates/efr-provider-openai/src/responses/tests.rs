use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_http::{HeaderMap, HttpClient, HttpConfig, Record, Recorder, RetryPolicy};
use efr_provider::{
    ContentBlock, Message, ModelInfo, Provider, ProviderError, ProviderEvent, ProviderId, Request,
    StopReason, ToolDefinition,
};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request as SeenRequest, ResponseTemplate};

use super::OpenAiProvider;
use crate::testing::{FakeTokens, FixedRng, InstantClock, fixture};
use crate::{Backend, Catalog, Fetched, ModelCatalog, OpenAiConfig, WebSocketMode};
use efr_stdx::time::Clock as _;

const SUBSCRIPTION_PATH: &str = "/backend-api/codex/responses";
const API_PATH: &str = "/v1/responses";

struct Setup {
    provider: OpenAiProvider,
    tokens: Arc<FakeTokens>,
    clock: Arc<InstantClock>,
}

fn setup(server: &MockServer, config: OpenAiConfig, tokens: FakeTokens) -> Setup {
    setup_with(server, config, tokens, |client| client)
}

fn setup_with(
    server: &MockServer,
    config: OpenAiConfig,
    tokens: FakeTokens,
    client: impl FnOnce(HttpClient) -> HttpClient,
) -> Setup {
    let clock = Arc::new(InstantClock::new());
    let http =
        HttpClient::new(&HttpConfig::default(), clock.clone(), Arc::new(FixedRng(0))).unwrap();
    let base = match config.backend() {
        Backend::Subscription => "/backend-api/codex",
        _ => "/v1",
    };
    // NOTE: these tests are about the HTTP path; websocket/tests.rs tests the socket.
    let config = config
        .with_base_url(&format!("{}{base}", server.uri()))
        .unwrap()
        .with_websocket(WebSocketMode::Off);
    let tokens = Arc::new(tokens);
    let provider = OpenAiProvider::new(
        ProviderId::new("openai-test").unwrap(),
        config,
        client(http),
        tokens.clone(),
        clock.clone(),
        Arc::new(FixedRng(0)),
    );
    Setup { provider, tokens, clock }
}

fn subscription_tokens() -> FakeTokens {
    FakeTokens::new(&["eyJ.access.one"]).with_account_id("acct_7d1f")
}

fn sse_response(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("x-request-id", "req_8f2a")
        .set_body_raw(fixture(name), "text/event-stream")
}

fn request(text: &str) -> Request {
    let mut request = Request::new("gpt-5.5");
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

#[tokio::test]
async fn a_subscription_request_carries_the_account_and_the_originator() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .and(header("authorization", "Bearer eyJ.access.one"))
        .and(header("chatgpt-account-id", "acct_7d1f"))
        .and(header("originator", "efr"))
        .and(header("accept", "text/event-stream"))
        .and(header("content-type", "application/json"))
        .respond_with(sse_response("plain_text.sse"))
        .expect(1)
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let completion = setup.provider.complete(request("Which shell do I use?")).await.unwrap();

    assert_eq!(completion.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(completion.stop_reason, StopReason::EndTurn);
    assert_eq!(completion.usage.map(|usage| usage.cached_input_tokens), Some(1024));
    let body = body_of(&seen(&server).await[0]);
    assert_eq!(body["model"], json!("gpt-5.5"));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["store"], json!(false));
    assert_eq!(body["instructions"], json!("You are efr."));
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
}

#[tokio::test]
async fn a_subscription_request_sends_its_cache_key_as_the_session_too() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .and(header("session-id", "0192f0c1-conversation"))
        .respond_with(sse_response("plain_text.sse"))
        .expect(1)
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());
    let mut request = request("Hi");
    request.provider_options.insert("prompt_cache_key".to_owned(), json!("0192f0c1-conversation"));

    setup.provider.complete(request).await.unwrap();

    let body = body_of(&seen(&server).await[0]);
    assert_eq!(body["prompt_cache_key"], json!("0192f0c1-conversation"));
}

#[tokio::test]
async fn an_api_request_sends_its_cache_key_in_the_body_only() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .respond_with(sse_response("plain_text.sse"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::api(), FakeTokens::new(&["sk-test"]));
    let mut request = request("Hi");
    request.provider_options.insert("prompt_cache_key".to_owned(), json!("0192f0c1-conversation"));

    setup.provider.complete(request).await.unwrap();

    let seen = seen(&server).await;
    assert_eq!(seen[0].headers.get("session-id"), None);
    assert_eq!(body_of(&seen[0])["prompt_cache_key"], json!("0192f0c1-conversation"));
}

#[tokio::test]
async fn a_request_too_large_for_the_window_is_an_overflow_and_is_sent_once() {
    let refusals = [
        ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "message": "Your input exceeds the context window of this model. Please adjust your input and try again.",
                "type": "invalid_request_error",
                "param": "input",
                "code": "context_length_exceeded",
            },
        })),
        ResponseTemplate::new(413).set_body_string("Payload Too Large"),
    ];
    for refusal in refusals {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(SUBSCRIPTION_PATH))
            .respond_with(refusal.clone())
            .mount(&server)
            .await;
        let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

        let error = setup.provider.complete(request("Hi")).await.unwrap_err();

        assert!(error.is_context_overflow(), "{error:?}");
        assert_eq!(seen(&server).await.len(), 1, "an overflow is never transient");
        assert!(setup.clock.sleeps().is_empty());
    }
}

#[tokio::test]
async fn a_stream_that_fails_for_the_window_is_an_overflow() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("context_length_exceeded.sse"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::ContextOverflow { status: None, code, .. } => {
            assert_eq!(code.as_deref(), Some("context_length_exceeded"));
        }
        other => panic!("not an overflow: {other:?}"),
    }
    assert_eq!(seen(&server).await.len(), 1);
}

#[tokio::test]
async fn an_api_request_sends_neither_the_account_nor_the_originator() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(sse_response("plain_text.sse"))
        .mount(&server)
        .await;
    let tokens = FakeTokens::new(&["sk-test"]).with_account_id("acct_ignored");
    let setup = setup(&server, OpenAiConfig::api(), tokens);

    let mut request = request("Which shell do I use?");
    request.max_output_tokens = Some(256);
    setup.provider.complete(request).await.unwrap();

    let seen = seen(&server).await;
    assert_eq!(seen[0].headers.get("chatgpt-account-id"), None);
    assert_eq!(seen[0].headers.get("originator"), None);
    assert_eq!(body_of(&seen[0])["max_output_tokens"], json!(256));
}

#[tokio::test]
async fn a_tool_call_comes_back_as_a_canonical_call_with_its_raw_items() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("tool_call.sse"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());
    let mut request = request("List the files.");
    request.tools = vec![ToolDefinition {
        name: "shell".to_owned(),
        description: "Run a command.".to_owned(),
        input_schema: json!({"type": "object"}),
        grammar: None,
    }];

    let completion = setup.provider.complete(request).await.unwrap();

    assert_eq!(completion.stop_reason, StopReason::ToolUse);
    assert_eq!(
        completion.message.content,
        vec![ContentBlock::ToolCall {
            call_id: "call_Qm8sX2vR7nL4kP1a".to_owned(),
            name: "shell".to_owned(),
            input: json!({"command": "ls -la"}),
            freeform: false,
        }]
    );
    let raw = completion.message.provider_raw.unwrap();
    assert_eq!(raw[0]["type"], json!("reasoning"));
    assert_eq!(raw[1]["type"], json!("function_call"));
    let tools = &body_of(&seen(&server).await[0])["tools"];
    assert_eq!(tools[0]["type"], json!("function"));
    assert_eq!(tools[0]["name"], json!("shell"));
}

#[tokio::test]
async fn encrypted_reasoning_goes_back_unchanged_on_the_next_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("reasoning_round_trip.sse"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("plain_text.sse"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let first = setup.provider.complete(request("How do I update this machine?")).await.unwrap();
    assert_eq!(first.message.text(), "Use pacman: sudo pacman -Syu.");
    assert_eq!(
        first.message.content[0],
        ContentBlock::Reasoning {
            text: "**Checking the package manager** The machine runs Arch, so pacman.\n\n**Answering** No command is needed."
                .to_owned()
        }
    );
    let raw = first.message.provider_raw.clone().unwrap();

    // The conversation stores the message and sends it back with the next prompt; the
    // store returns object members sorted, which must not matter.
    let stored: Message =
        serde_json::from_str(&serde_json::to_string(&first.message).unwrap()).unwrap();
    let mut next = request("How do I update this machine?");
    next.messages.push(stored);
    next.messages.push(Message::user("And clean the cache?"));
    setup.provider.complete(next).await.unwrap();

    let input = body_of(&seen(&server).await[1])["input"].clone();
    assert_eq!(input.as_array().map(Vec::len), Some(4));
    assert_eq!(Value::Array(input.as_array().unwrap()[1..3].to_vec()), raw);
    assert_eq!(input[1]["id"], json!("rs_2c3d4e5f60718293a4b5c6d7e8f90a1b"));
    assert_eq!(
        input[1]["encrypted_content"],
        json!(
            "gAAAAABo4cF4reasoning_state_8Jk2Lm4Np6Qr8St0Uv2Wx4Yz6Ab8Cd0Ef2Gh4Ij6Kl8Mn0Op2Qr4St6Uv8Wx0Yz2Ab4Cd6Ef8Gh0"
        )
    );
    assert_eq!(input[3]["content"][0]["text"], json!("And clean the cache?"));
}

#[tokio::test]
async fn an_error_event_fails_the_stream_after_the_text_so_far() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("error_event.sse"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let events: Vec<_> = setup.provider.stream(request("Hi")).await.unwrap().collect().await;

    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(&events[0], Ok(ProviderEvent::TextDelta { text }) if text == "Checking"));
    assert!(
        matches!(&events[1], Err(ProviderError::Api { code: Some(code), .. }) if code == "server_error"),
        "{events:?}"
    );
}

#[tokio::test]
async fn a_401_refreshes_the_token_once_and_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .and(header("authorization", "Bearer eyJ.access.expired"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_raw(fixture("unauthorized.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .and(header("authorization", "Bearer eyJ.access.fresh"))
        .respond_with(sse_response("plain_text.sse"))
        .expect(1)
        .mount(&server)
        .await;
    let tokens =
        FakeTokens::new(&["eyJ.access.expired", "eyJ.access.fresh"]).with_account_id("acct_7d1f");
    let setup = setup(&server, OpenAiConfig::subscription(), tokens);

    let completion = setup.provider.complete(request("Hi")).await.unwrap();

    assert_eq!(completion.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(setup.tokens.invalidations(), 1);
    assert_eq!(setup.tokens.fetches(), 2);
    let seen = seen(&server).await;
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].body, seen[1].body);
}

#[tokio::test]
async fn a_second_401_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_raw(fixture("unauthorized.json"), "application/json"),
        )
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    assert!(matches!(error, ProviderError::Unauthorized { .. }), "{error:?}");
    assert_eq!(setup.tokens.invalidations(), 1);
    assert_eq!(seen(&server).await.len(), 2);
}

#[tokio::test]
async fn a_401_for_a_key_that_cannot_refresh_fails_at_once_with_the_servers_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .respond_with(ResponseTemplate::new(401).set_body_raw(
            r#"{"error":{"message":"You have insufficient permissions for this operation. Missing scopes: api.responses.write.","type":"invalid_request_error","param":null,"code":null}}"#,
            "application/json",
        ))
        .expect(1)
        .mount(&server)
        .await;
    let tokens = FakeTokens::new(&["sk-proj-test"]).without_refresh();
    let setup = setup(&server, OpenAiConfig::api(), tokens);

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    let message = "You have insufficient permissions for this operation. Missing scopes: \
                   api.responses.write.";
    assert!(
        matches!(&error, ProviderError::Unauthorized { message: Some(text) } if text == message),
        "{error:?}"
    );
    assert_eq!(setup.tokens.invalidations(), 0);
    assert_eq!(seen(&server).await.len(), 1);
}

#[tokio::test]
async fn no_token_means_no_request() {
    let server = MockServer::start().await;
    let setup = setup(&server, OpenAiConfig::subscription(), FakeTokens::new(&[]));

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    assert!(matches!(error, ProviderError::NotLoggedIn), "{error:?}");
    assert!(seen(&server).await.is_empty());
}

#[tokio::test]
async fn a_server_error_is_not_sent_twice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(ResponseTemplate::new(500).set_body_raw(
            r#"{"error":{"message":"The server had an error.","type":"server_error","code":null}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::Api { status: Some(500), code, message } => {
            assert_eq!(code.as_deref(), Some("server_error"));
            assert_eq!(message, "The server had an error.");
        }
        other => panic!("not an API error: {other:?}"),
    }
    assert_eq!(seen(&server).await.len(), 1);
    assert!(setup.clock.sleeps().is_empty());
}

#[tokio::test]
async fn an_unavailable_server_is_tried_again_on_the_clock() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "2"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("plain_text.sse"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let completion = setup.provider.complete(request("Hi")).await.unwrap();

    assert_eq!(completion.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(setup.clock.sleeps(), vec![Duration::from_secs(2)]);
    assert_eq!(seen(&server).await.len(), 2);
}

#[tokio::test]
async fn a_rate_limit_reports_the_wait_from_the_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120").set_body_raw(
            r#"{"error":{"message":"Rate limit reached.","type":"requests","code":"rate_limit_exceeded"}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;
    let config = OpenAiConfig::api().with_retry(RetryPolicy::none());
    let setup = setup(&server, config, FakeTokens::new(&["sk-test"]));

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Some(Duration::from_secs(120)));
        }
        other => panic!("not a rate limit: {other:?}"),
    }
}

#[tokio::test]
async fn a_used_up_subscription_reports_when_it_resets() {
    let server = MockServer::start().await;
    let resets_at = crate::testing::start().as_second() + 3600;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {
                "type": "usage_limit_reached",
                "message": "You have hit your usage limit.",
                "plan_type": "plus",
                "resets_at": resets_at,
            },
        })))
        .mount(&server)
        .await;
    let config = OpenAiConfig::subscription().with_retry(RetryPolicy::none());
    let setup = setup(&server, config, subscription_tokens());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Some(Duration::from_secs(3600)));
        }
        other => panic!("not a rate limit: {other:?}"),
    }
}

#[tokio::test]
async fn an_exhausted_quota_is_not_a_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {
                "message": "You exceeded your current quota.",
                "type": "insufficient_quota",
                "code": "insufficient_quota",
            },
        })))
        .mount(&server)
        .await;
    let config = OpenAiConfig::api().with_retry(RetryPolicy::none());
    let setup = setup(&server, config, FakeTokens::new(&["sk-test"]));

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::Api { status: Some(429), code, message } => {
            assert_eq!(code.as_deref(), Some("insufficient_quota"));
            assert_eq!(message, "You exceeded your current quota.");
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[tokio::test]
async fn an_exhausted_quota_is_never_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1").set_body_json(
            json!({ "error": { "message": "You exceeded your current quota.", "type": "insufficient_quota" } }),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::api(), FakeTokens::new(&["sk-test"]));

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    assert!(matches!(error, ProviderError::Api { status: Some(429), .. }), "{error:?}");
    assert_eq!(seen(&server).await.len(), 1, "the default policy did not try again");
    assert!(setup.clock.sleeps().is_empty(), "{:?}", setup.clock.sleeps());
}

#[tokio::test]
async fn a_rate_limit_is_sent_again_after_its_wait() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2").set_body_json(
            json!({ "error": { "message": "Rate limit reached.", "code": "rate_limit_exceeded" } }),
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .respond_with(sse_response("plain_text.sse"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::api(), FakeTokens::new(&["sk-test"]));

    let completion = setup.provider.complete(request("Hi")).await.unwrap();

    assert_eq!(completion.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(seen(&server).await.len(), 2);
    assert_eq!(setup.clock.sleeps(), [Duration::from_secs(2)]);
}

#[tokio::test]
async fn an_api_request_names_the_organization_and_the_project() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(API_PATH))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("openai-organization", "org-AbC"))
        .and(header("openai-project", "proj_AbC"))
        .respond_with(sse_response("plain_text.sse"))
        .expect(1)
        .mount(&server)
        .await;
    let config =
        OpenAiConfig::api().with_organization("org-AbC").unwrap().with_project("proj_AbC").unwrap();
    let setup = setup(&server, config, FakeTokens::new(&["sk-test"]));

    setup.provider.complete(request("Hi")).await.unwrap();

    let seen = seen(&server).await;
    assert!(seen[0].headers.get("originator").is_none(), "the API takes no originator");
}

#[tokio::test]
async fn a_subscription_request_never_names_an_organization() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("plain_text.sse"))
        .expect(1)
        .mount(&server)
        .await;
    let config = OpenAiConfig::subscription()
        .with_organization("org-AbC")
        .unwrap()
        .with_project("proj_AbC")
        .unwrap();
    let setup = setup(&server, config, subscription_tokens());

    setup.provider.complete(request("Hi")).await.unwrap();

    let seen = seen(&server).await;
    assert!(seen[0].headers.get("openai-organization").is_none());
    assert!(seen[0].headers.get("openai-project").is_none());
}

#[tokio::test]
async fn a_model_the_subscription_refuses_is_an_unknown_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "detail": "The 'gpt-4.1' model is not supported when using Codex with a ChatGPT account.",
        })))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());
    let mut request = request("Hi");
    request.model = "gpt-4.1".to_owned();

    let error = setup.provider.complete(request).await.unwrap_err();

    assert!(
        matches!(&error, ProviderError::UnknownModel { model } if model == "gpt-4.1"),
        "{error:?}"
    );
}

#[tokio::test]
async fn an_error_body_that_is_not_json_becomes_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(ResponseTemplate::new(403).set_body_string("<html>Forbidden</html>"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let error = setup.provider.complete(request("Hi")).await.unwrap_err();

    match error {
        ProviderError::Api { status: Some(403), code: None, message } => {
            assert_eq!(message, "<html>Forbidden</html>");
        }
        other => panic!("not an API error: {other:?}"),
    }
}

#[tokio::test]
async fn a_stream_that_ends_before_completion_is_incomplete() {
    let server = MockServer::start().await;
    let truncated: String =
        fixture("plain_text.sse").split("event: response.completed").next().unwrap().to_owned();
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_raw(truncated, "text/event-stream"))
        .mount(&server)
        .await;
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());

    let events: Vec<_> = setup.provider.stream(request("Hi")).await.unwrap().collect().await;

    assert!(matches!(events.last(), Some(Err(ProviderError::Incomplete))), "{events:?}");
    assert_eq!(events.iter().filter(|event| event.is_ok()).count(), 3);
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
async fn a_recorded_request_shows_no_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("plain_text.sse"))
        .mount(&server)
        .await;
    let recorder = Arc::new(HeaderRecorder::default());
    let for_client = recorder.clone();
    let setup =
        setup_with(&server, OpenAiConfig::subscription(), subscription_tokens(), |client| {
            client.with_recorder(for_client)
        });

    setup.provider.complete(request("Hi")).await.unwrap();

    let requests = recorder.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["authorization"], "Bearer [REDACTED]");
    assert_eq!(requests[0]["chatgpt-account-id"], "[REDACTED]");
    assert_eq!(requests[0]["originator"], "efr");
}

#[tokio::test]
async fn the_provider_names_itself_and_its_models() {
    let server = MockServer::start().await;
    let config = OpenAiConfig::subscription().with_models(vec![ModelInfo::new("gpt-test")]);
    let setup = setup(&server, config, subscription_tokens());
    assert_eq!(setup.provider.id().as_str(), "openai-test");
    let models = setup.provider.models();
    assert_eq!(models.first().map(|model| model.id.as_str()), Some("gpt-6.1-sol"));
    assert_eq!(models.last(), Some(&ModelInfo::new("gpt-test")), "the config's model comes last");
    assert_eq!(setup.provider.config().originator(), "efr");
    let debug = format!("{:?}", setup.provider);
    assert!(debug.contains("OpenAiProvider"), "{debug}");
}

#[tokio::test]
async fn a_new_catalog_decides_the_tool_form_from_the_next_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(SUBSCRIPTION_PATH))
        .respond_with(sse_response("plain_text.sse"))
        .mount(&server)
        .await;
    let catalog = ModelCatalog::new(Catalog::builtin(Backend::Subscription));
    let setup = setup(&server, OpenAiConfig::subscription(), subscription_tokens());
    let provider = setup.provider.with_catalog(catalog.clone());
    let mut request = request("hello");
    request.model = "gpt-7-sol".to_owned();
    request.tools = vec![ToolDefinition::freeform(
        "apply_patch",
        "Edit files.",
        efr_provider::ToolGrammar::lark("start: \"x\""),
    )];

    provider.complete(request.clone()).await.unwrap();
    let body = serde_json::json!({"models": [{
        "slug": "gpt-7-sol", "visibility": "list", "priority": 1,
        "apply_patch_tool_type": "freeform", "supported_reasoning_levels": [],
    }]});
    let (entries, _) = crate::catalog::entries_of(&body).unwrap();
    let fetched = Catalog::from_backend(
        Backend::Subscription,
        "https://backend.test/codex",
        entries,
        None,
        setup.clock.now(),
    );
    catalog.apply(Fetched::Changed(fetched), setup.clock.now());
    provider.complete(request).await.unwrap();

    let sent = seen(&server).await;
    assert_eq!(body_of(&sent[0])["tools"][0]["type"], "function", "unknown before the fetch");
    assert_eq!(body_of(&sent[1])["tools"][0]["type"], "custom", "the catalog says freeform");
}
