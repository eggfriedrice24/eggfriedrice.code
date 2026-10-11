//! The judge through the real providers, against a fake API on wiremock: the bytes that
//! each backend sends for a review, and the verdict that comes back.

use std::sync::Arc;

use efr_http::{HttpClient, HttpConfig};
use efr_provider::{EditTool, ModelInfo, Provider, ProviderId, SecretString, StaticToken};
use efr_provider_anthropic::{AnthropicConfig, AnthropicProvider};
use efr_provider_openai::{OpenAiConfig, OpenAiProvider};
use efr_stdx::rng::Rng;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{ALLOW, allowed, ids, input, record};
use crate::catalog::Vendor;
use crate::judge::{ClassifierJudge, PROMPT};
use efr_conversation::ExitJudge as _;
use efr_protocol::ChainStep;

/// Server-sent events, each `(type, data)` with the type set in the data.
fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(kind, data)| {
            let mut data = data.clone();
            data["type"] = json!(kind);
            format!("event: {kind}\ndata: {data}\n\n")
        })
        .collect()
}

fn streaming(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

/// A Responses answer with `text` as one assistant message.
fn responses_text(text: &str) -> String {
    let item = json!({
        "id": "msg_1", "type": "message", "status": "completed", "role": "assistant",
        "content": [{"type": "output_text", "annotations": [], "text": text}],
    });
    let response = |status: &str, output: Value| {
        json!({
            "id": "resp_1", "object": "response", "status": status, "model": "gpt-6-luna",
            "output": output,
            "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
        })
    };
    sse(&[
        ("response.created", json!({"response": response("in_progress", json!([]))})),
        (
            "response.output_text.delta",
            json!({"item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": text}),
        ),
        ("response.output_item.done", json!({"output_index": 0, "item": item})),
        ("response.completed", json!({"response": response("completed", json!([item]))})),
    ])
}

/// A Messages answer with `text` as one text block.
fn messages_text(text: &str) -> String {
    sse(&[
        (
            "message_start",
            json!({"message": {
                "id": "msg_1", "type": "message", "role": "assistant", "content": [],
                "model": "claude-haiku-5-5", "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": 10, "output_tokens": 1},
            }}),
        ),
        ("content_block_start", json!({"index": 0, "content_block": {"type": "text", "text": ""}})),
        ("content_block_delta", json!({"index": 0, "delta": {"type": "text_delta", "text": text}})),
        ("content_block_stop", json!({"index": 0})),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}),
        ),
        ("message_stop", json!({})),
    ])
}

fn http(clock: &TestClock) -> HttpClient {
    let rng: Arc<dyn Rng> = Arc::new(TestRng::new(7));
    HttpClient::new(&HttpConfig::default(), clock.shared(), rng).unwrap()
}

fn key() -> Arc<StaticToken> {
    Arc::new(StaticToken::new(SecretString::from("test-key".to_owned())))
}

/// The JSON bodies that the server got on `at`.
async fn bodies(server: &MockServer, at: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path() == at)
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

fn judge(provider: Arc<dyn Provider>, vendor: Vendor, clock: &TestClock) -> ClassifierJudge {
    let (_, settings) = tokio::sync::watch::channel(Arc::new(crate::Settings::default()));
    ClassifierJudge::new(provider, settings, vendor, clock.shared())
}

#[tokio::test]
async fn judge_wire_openai_end_to_end() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(streaming(responses_text(ALLOW)))
        .mount(&server)
        .await;
    let clock = TestClock::new();
    let config = OpenAiConfig::api().with_base_url(&format!("{}/v1", server.uri())).unwrap();
    let provider = OpenAiProvider::new(
        ProviderId::new("openai-api").unwrap(),
        config,
        http(&clock),
        key(),
        clock.shared(),
        Arc::new(TestRng::new(8)),
    );
    let judge = judge(Arc::new(provider), Vendor::OpenAi, &clock);
    let record = record();

    let outcome = judge.judge(input(&ids(1), &record)).await;
    assert_eq!(outcome.judgement, Some(allowed()));
    assert_eq!(outcome.model.as_deref(), Some("gpt-6-luna"));
    assert_eq!(outcome.chain_step, ChainStep::ClassifierModel);

    let bodies = bodies(&server, "/v1/responses").await;
    assert_eq!(bodies.len(), 1);
    let body = &bodies[0];
    assert_eq!(body["model"], json!("gpt-6-luna"));
    assert_eq!(body["instructions"], json!(PROMPT));
    assert_eq!(body["store"], json!(false));
    assert_eq!(body["tools"], json!([]));
    assert_eq!(body.get("tool_choice"), None);
    assert_eq!(body.get("prompt_cache_key"), None);
    assert_eq!(body.get("max_output_tokens"), None);
    assert_eq!(body["reasoning"], json!({"effort": "low"}));
    assert_eq!(body["text"]["format"]["type"], json!("json_schema"));
    assert_eq!(body["text"]["format"]["name"], json!("exit_verdict"));
    assert_eq!(body["text"]["format"]["strict"], json!(true));
    let sent = body["input"][0]["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<efr_protocol::ExitRecord>(sent).unwrap(), record);
}

#[tokio::test]
async fn judge_wire_anthropic_end_to_end() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(streaming(messages_text(ALLOW)))
        .mount(&server)
        .await;
    let clock = TestClock::new();
    let haiku = ModelInfo::new("claude-haiku-5-5")
        .with_max_output_tokens(64_000)
        .with_efforts(["low", "medium", "high"], Some("medium"))
        .with_edit_tool(EditTool::Replace);
    let config = AnthropicConfig::new()
        .with_base_url(&format!("{}/v1", server.uri()))
        .unwrap()
        .with_models(vec![haiku]);
    let provider = AnthropicProvider::new(
        ProviderId::new("anthropic-api").unwrap(),
        config,
        http(&clock),
        key(),
        clock.shared(),
        Arc::new(TestRng::new(8)),
    );
    let judge = judge(Arc::new(provider), Vendor::Anthropic, &clock);
    let record = record();

    let outcome = judge.judge(input(&ids(1), &record)).await;
    assert_eq!(outcome.judgement, Some(allowed()));
    assert_eq!(outcome.model.as_deref(), Some("claude-haiku-5-5"));

    let bodies = bodies(&server, "/v1/messages").await;
    assert_eq!(bodies.len(), 1);
    let body = &bodies[0];
    assert_eq!(body["model"], json!("claude-haiku-5-5"));
    assert_eq!(body["max_tokens"], json!(64_000));
    assert_eq!(body.get("tools"), None);
    assert_eq!(
        body["system"],
        json!([{"type": "text", "text": PROMPT, "cache_control": {"type": "ephemeral", "ttl": "5m"}}])
    );
    assert_eq!(body["output_config"]["effort"], json!("low"));
    assert_eq!(body["output_config"]["format"]["type"], json!("json_schema"));
    assert_eq!(body["output_config"]["format"]["schema"]["additionalProperties"], json!(false));
    let message = &body["messages"][0];
    assert_eq!(message["role"], json!("user"));
    assert_eq!(message["content"][0].get("cache_control"), None);
    let sent = message["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<efr_protocol::ExitRecord>(sent).unwrap(), record);
}

#[tokio::test]
async fn a_claude_model_without_a_cap_is_missing_and_the_turn_model_reviews() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(streaming(messages_text(ALLOW)))
        .mount(&server)
        .await;
    let clock = TestClock::new();
    // The list holds the turn's model with its cap, and the small model without one.
    let models = vec![
        ModelInfo::new("claude-haiku-5-5"),
        ModelInfo::new("gpt-6.1-sol").with_max_output_tokens(128_000),
    ];
    let config = AnthropicConfig::new()
        .with_base_url(&format!("{}/v1", server.uri()))
        .unwrap()
        .with_models(models);
    let provider = AnthropicProvider::new(
        ProviderId::new("anthropic-api").unwrap(),
        config,
        http(&clock),
        key(),
        clock.shared(),
        Arc::new(TestRng::new(8)),
    );
    let judge = judge(Arc::new(provider), Vendor::Anthropic, &clock);
    let record = record();

    let outcome = judge.judge(input(&ids(1), &record)).await;
    assert_eq!(outcome.chain_step, ChainStep::TurnModel);
    assert_eq!(outcome.skipped, ["claude-haiku-5-5: missing"]);
    assert_eq!(bodies(&server, "/v1/messages").await.len(), 1);
}

/// The raw bodies that the server got on `at`, as text.
async fn raw_bodies(server: &MockServer, at: &str) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path() == at)
        .map(|request| String::from_utf8(request.body).unwrap())
        .collect()
}

/// The byte offsets of the `rationale` and the `verdict` members of the schema in
/// `body`, and the `required` list of the schema as it is on the wire.
fn schema_order(body: &str) -> (usize, usize, Value) {
    let properties = body.find(r#""properties":{"#).unwrap();
    let rationale = body[properties..].find(r#""rationale":{"#).unwrap();
    let verdict = body[properties..].find(r#""verdict":{"#).unwrap();
    let value: Value = serde_json::from_str(body).unwrap();
    let schema = value
        .pointer("/text/format/schema")
        .or_else(|| value.pointer("/output_config/format/schema"))
        .unwrap();
    (rationale, verdict, schema["required"].clone())
}

#[tokio::test]
async fn the_wire_schema_puts_the_rationale_before_the_verdict() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(streaming(responses_text(ALLOW)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(streaming(messages_text(ALLOW)))
        .mount(&server)
        .await;
    let clock = TestClock::new();
    let base = format!("{}/v1", server.uri());
    let openai = OpenAiProvider::new(
        ProviderId::new("openai-api").unwrap(),
        OpenAiConfig::api().with_base_url(&base).unwrap(),
        http(&clock),
        key(),
        clock.shared(),
        Arc::new(TestRng::new(8)),
    );
    let haiku = ModelInfo::new("claude-haiku-5-5")
        .with_max_output_tokens(64_000)
        .with_efforts(["low", "medium", "high"], Some("medium"));
    let anthropic = AnthropicProvider::new(
        ProviderId::new("anthropic-api").unwrap(),
        AnthropicConfig::new().with_base_url(&base).unwrap().with_models(vec![haiku]),
        http(&clock),
        key(),
        clock.shared(),
        Arc::new(TestRng::new(8)),
    );
    let record = record();
    judge(Arc::new(openai), Vendor::OpenAi, &clock).judge(input(&ids(1), &record)).await;
    judge(Arc::new(anthropic), Vendor::Anthropic, &clock).judge(input(&ids(2), &record)).await;

    let required = json!(["rationale", "category", "risk", "user_authorization", "verdict"]);
    for at in ["/v1/responses", "/v1/messages"] {
        let bodies = raw_bodies(&server, at).await;
        assert_eq!(bodies.len(), 1, "{at}");
        let (rationale, verdict, listed) = schema_order(&bodies[0]);
        assert!(rationale < verdict, "{at}: {}", bodies[0]);
        assert_eq!(listed, required, "{at}");
    }
}
