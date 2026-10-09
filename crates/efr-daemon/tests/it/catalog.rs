//! The model catalog over a `TestDaemon` with the real subscription provider: efrd
//! fetches the backend's list in the background, `models.list` and the turns follow
//! it, the cache file brings it back after a restart while the backend is down, and a
//! fetch that hangs never holds up a prompt.

use std::time::Duration;

use efr_protocol::{
    AdminStatus, AdminStatusResult, CatalogOrigin, Event, Method, ModelsList, ModelsListResult,
    PromptSendResult,
};
use efr_test_daemon::{
    ModelsAnswer, ResponsesAnswer, ResponsesServer, TTY, TestDaemon, events_until,
};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

/// A catalog with a better model than the built-in table knows, one with a larger
/// window, a model whose tool form efr does not know, and a model that asks for a
/// newer Codex.
fn catalog() -> Value {
    json!({"models": [
        {
            "slug": "gpt-test-sol", "display_name": "GPT-Test-Sol", "visibility": "list",
            "priority": 1, "context_window": 300_000, "max_context_window": 900_000,
            "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}],
            "default_reasoning_level": "low", "apply_patch_tool_type": "freeform",
            "prefer_websockets": true, "minimal_client_version": "0.0.1",
        },
        {
            "slug": "gpt-test-luna", "visibility": "list", "priority": 2,
            "context_window": 128_000,
            "supported_reasoning_levels": [{"effort": "low"}],
        },
        {
            "slug": "gpt-test-odd-tool", "visibility": "list", "priority": 0,
            "apply_patch_tool_type": "grammar", "supported_reasoning_levels": [],
        },
        {
            "slug": "gpt-test-codex", "visibility": "list", "priority": 3,
            "minimal_client_version": "999.0.0", "supported_reasoning_levels": [],
        },
    ]})
}

async fn models(daemon: &TestDaemon) -> ModelsListResult {
    let client = daemon.client().await.unwrap();
    client.call(Method::ModelsList(ModelsList::default())).await.unwrap()
}

async fn origin(daemon: &TestDaemon) -> Option<CatalogOrigin> {
    models(daemon).await.catalog.map(|catalog| catalog.origin)
}

#[tokio::test]
async fn the_backend_list_applies_and_survives_a_restart_while_the_backend_is_down() {
    let server = ResponsesServer::start().await;
    server.set_models(ModelsAnswer::catalog(&catalog(), "\"cat-1\""));
    let mut daemon =
        TestDaemon::builder().subscription(&server).persistent().start().await.unwrap();

    Wait::new("the catalog from the backend")
        .until_some_async(async || {
            (origin(&daemon).await == Some(CatalogOrigin::Backend)).then_some(())
        })
        .await
        .unwrap();
    let list = models(&daemon).await;
    let ids: Vec<&str> = list.models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(
        ids,
        ["gpt-test-sol", "gpt-test-luna", "gpt-test-codex"],
        "the unknown tool form is left out, a Codex version is not"
    );
    assert!(list.models[0].default, "the best priority is the default");
    assert_eq!(list.models[0].context_window, Some(300_000));
    assert_eq!(list.models[0].max_context_window, Some(900_000));
    assert!(list.models[0].prefer_websockets);
    let request = server.models_requests()[0].clone();
    assert_eq!(request.client_version, None, "efr has no Codex version to send");
    assert_eq!(request.originator.as_deref(), Some("efr"), "efr names itself");
    assert_eq!(request.if_none_match, None);
    let cache = daemon.dirs().dirs().state().join("model_catalog.json");
    Wait::new("the cache file").until(|| cache.exists()).await.unwrap();

    server.set_models(ModelsAnswer::status(503));
    daemon.restart().await.unwrap();

    Wait::new("the fetch after the restart")
        .until(|| server.models_requests().len() >= 2)
        .await
        .unwrap();
    assert_eq!(
        server.models_requests()[1].if_none_match.as_deref(),
        Some("\"cat-1\""),
        "the cached list goes with its tag"
    );
    let list = models(&daemon).await;
    assert_eq!(list.catalog.map(|catalog| catalog.origin), Some(CatalogOrigin::Cache));
    assert_eq!(
        list.models[0].id, "gpt-test-sol",
        "the cached list stays while the backend is down"
    );
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    assert_eq!(status.catalog.map(|catalog| catalog.origin), Some(CatalogOrigin::Cache));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_304_confirms_the_cached_list() {
    let server = ResponsesServer::start().await;
    server.set_models(ModelsAnswer::catalog(&catalog(), "\"cat-1\""));
    let mut daemon =
        TestDaemon::builder().subscription(&server).persistent().start().await.unwrap();
    let cache = daemon.dirs().dirs().state().join("model_catalog.json");
    Wait::new("the cache file").until(|| cache.exists()).await.unwrap();

    server.set_models(ModelsAnswer::status(304));
    daemon.restart().await.unwrap();

    Wait::new("the list confirmed by the backend")
        .until_some_async(async || {
            (origin(&daemon).await == Some(CatalogOrigin::Backend)).then_some(())
        })
        .await
        .unwrap();
    assert_eq!(models(&daemon).await.models[0].id, "gpt-test-sol");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_prompt_never_waits_for_a_fetch_that_hangs() {
    let server = ResponsesServer::start().await;
    server.set_models(
        ModelsAnswer::catalog(&catalog(), "\"cat-1\"").after(Duration::from_secs(3600)),
    );
    server.push(ResponsesAnswer::text("Hello while the catalog hangs."));
    let daemon = TestDaemon::builder().subscription(&server).start().await.unwrap();
    Wait::new("the fetch that hangs").until(|| !server.models_requests().is_empty()).await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let sent: PromptSendResult = client.call(daemon.prompt(1, "hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let events = events_until(&mut follow, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();

    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    assert_eq!(server.received()[0].body["model"], "gpt-6.1-sol", "the built-in default");
    assert_eq!(origin(&daemon).await, Some(CatalogOrigin::Builtin));
    drop((follow, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_turn_after_the_fetch_uses_the_backends_default_and_tool_form() {
    let server = ResponsesServer::start().await;
    server.set_models(ModelsAnswer::catalog(&catalog(), "\"cat-1\""));
    server.push(ResponsesAnswer::text("Hello from the new default."));
    let daemon = TestDaemon::builder().subscription(&server).start().await.unwrap();
    Wait::new("the catalog from the backend")
        .until_some_async(async || {
            (origin(&daemon).await == Some(CatalogOrigin::Backend)).then_some(())
        })
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let sent: PromptSendResult = client.call(daemon.prompt(1, "hello", TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    events_until(&mut follow, |event| {
        matches!(event, Event::TurnCompleted { .. } | Event::TurnFailed { .. })
    })
    .await
    .unwrap();

    let body = &server.received()[0].body;
    assert_eq!(body["model"], "gpt-test-sol");
    let patch = body["tools"].as_array().unwrap().iter().find(|tool| tool["name"] == "apply_patch");
    assert_eq!(patch.map(|tool| tool["type"].clone()), Some(json!("custom")));
    drop((follow, client));
    daemon.stop().await.unwrap();
}
