use std::time::Duration;

use efr_http::RetryPolicy;
use efr_provider::ModelInfo;
use pretty_assertions::assert_eq;

use super::{
    API_BASE_URL, Backend, DEFAULT_ORIGINATOR, OpenAiConfig, ReasoningMode, SUBSCRIPTION_BASE_URL,
    WebSocketMode,
};
use crate::OpenAiError;

#[test]
fn the_websocket_follows_the_catalog_unless_the_switch_says_otherwise() {
    let prefers = ModelInfo::new("gpt-5.5").with_prefer_websockets(true);
    let plain = ModelInfo::new("o3");
    let config = OpenAiConfig::subscription();
    assert_eq!(config.websocket(), WebSocketMode::Auto);
    assert!(config.uses_websocket(Some(&prefers)));
    assert!(!config.uses_websocket(Some(&plain)));
    assert!(!config.uses_websocket(None), "a model outside the catalog uses HTTP");

    let on = config.clone().with_websocket(WebSocketMode::On);
    assert!(on.uses_websocket(Some(&plain)));
    assert!(on.uses_websocket(None));
    let off = config.with_websocket(WebSocketMode::Off);
    assert!(!off.uses_websocket(Some(&prefers)));
}

#[test]
fn an_organization_or_a_project_must_fit_a_header() {
    let config = OpenAiConfig::api();
    assert_eq!(config.organization(), None);
    assert_eq!(config.project(), None);
    let named =
        config.clone().with_organization("org-AbC").unwrap().with_project("proj_1").unwrap();
    assert_eq!(named.organization(), Some("org-AbC"));
    assert_eq!(named.project(), Some("proj_1"));
    assert!(matches!(
        config.clone().with_organization("org\nAbC"),
        Err(OpenAiError::InvalidOrganization { .. })
    ));
    assert!(matches!(config.with_project("proj\r1"), Err(OpenAiError::InvalidProject { .. })));
}

#[test]
fn the_subscription_defaults_follow_codex() {
    let config = OpenAiConfig::subscription();
    assert_eq!(config.backend(), Backend::Subscription);
    assert_eq!(config.base_url(), SUBSCRIPTION_BASE_URL);
    assert_eq!(config.responses_url(), "https://chatgpt.com/backend-api/codex/responses");
    assert_eq!(config.originator(), DEFAULT_ORIGINATOR);
    assert!(config.models().is_empty(), "the catalog lists the models");
    assert_eq!(config.retry(), &RetryPolicy::default());
    assert_eq!(config.reasoning(), ReasoningMode::ByModel);
    assert_eq!(config.reasoning_effort(), None);
    assert_eq!(config.reasoning_summary(), Some("auto"));
    assert!(config.parallel_tool_calls());
}

#[test]
fn the_api_defaults_list_no_models() {
    let config = OpenAiConfig::api();
    assert_eq!(config.backend(), Backend::Api);
    assert_eq!(config.base_url(), API_BASE_URL);
    assert_eq!(config.responses_url(), "https://api.openai.com/v1/responses");
    assert!(config.models().is_empty());
}

#[test]
fn a_base_url_loses_its_trailing_slashes() {
    let config = OpenAiConfig::api().with_base_url("http://127.0.0.1:4000/v1//").unwrap();
    assert_eq!(config.base_url(), "http://127.0.0.1:4000/v1");
    assert_eq!(config.responses_url(), "http://127.0.0.1:4000/v1/responses");
}

#[test]
fn a_base_url_must_be_http_or_https() {
    let error = OpenAiConfig::api().with_base_url("ftp://example.com").unwrap_err();
    assert!(matches!(error, OpenAiError::InvalidBaseUrl { .. }), "{error:?}");
}

#[test]
fn the_setters_replace_each_setting() {
    let mut retry = RetryPolicy::none();
    retry.max_attempts = 2;
    let config = OpenAiConfig::subscription()
        .with_originator("efr-test")
        .unwrap()
        .with_models(vec![ModelInfo::new("gpt-test")])
        .with_retry(retry.clone())
        .with_reasoning(ReasoningMode::Never)
        .with_reasoning_effort(Some("high".to_owned()))
        .with_reasoning_summary(None)
        .with_parallel_tool_calls(false);
    assert_eq!(config.originator(), "efr-test");
    assert_eq!(config.models(), &[ModelInfo::new("gpt-test")]);
    assert_eq!(config.retry(), &retry);
    assert_eq!(config.retry().initial_backoff, Duration::from_millis(500));
    assert_eq!(config.reasoning(), ReasoningMode::Never);
    assert_eq!(config.reasoning_effort(), Some("high"));
    assert_eq!(config.reasoning_summary(), None);
    assert!(!config.parallel_tool_calls());
}
