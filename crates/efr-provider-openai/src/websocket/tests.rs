use std::sync::Arc;

use efr_http::{HttpClient, HttpConfig, RetryPolicy};
use efr_provider::{Message, Provider, ProviderEvent, ProviderId, ProviderStream, Request};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::IDLE;
use crate::testing::{
    FakeTokens, FixedRng, ManualClock, ResponsesServer, Socket, Step, fixture, sse_events,
};
use crate::{Backend, Catalog, Fetched, ModelCatalog, OpenAiConfig, OpenAiProvider, WebSocketMode};
use efr_stdx::time::Clock as _;

const KEY: &str = "0192f0c1-conversation";

struct Setup {
    provider: OpenAiProvider,
    server: ResponsesServer,
    clock: Arc<ManualClock>,
}

async fn setup(sockets: Vec<Socket>, posts: Vec<String>, mode: WebSocketMode) -> Setup {
    let server = ResponsesServer::start(sockets, posts).await;
    let clock = Arc::new(ManualClock::new());
    let http =
        HttpClient::new(&HttpConfig::default(), clock.clone(), Arc::new(FixedRng(0))).unwrap();
    let mut retry = RetryPolicy::default();
    retry.max_attempts = 1;
    let config = OpenAiConfig::subscription()
        .with_base_url(&format!("{}/backend-api/codex", server.uri()))
        .unwrap()
        .with_retry(retry)
        .with_websocket(mode);
    let tokens = FakeTokens::new(&["eyJ.access.one"]).with_account_id("acct_7d1f");
    let provider = OpenAiProvider::new(
        ProviderId::new("openai-test").unwrap(),
        config,
        http,
        Arc::new(tokens),
        clock.clone(),
    );
    Setup { provider, server, clock }
}

fn request(model: &str, text: &str) -> Request {
    let mut request = Request::new(model);
    request.system = Some("You are efr.".to_owned());
    request.messages = vec![Message::user(text)];
    request.provider_options.insert("prompt_cache_key".to_owned(), json!(KEY));
    request
}

/// Every item of `stream`, with errors as their debug text.
async fn collect(stream: ProviderStream) -> Vec<Result<ProviderEvent, String>> {
    stream.map(|item| item.map_err(|error| format!("{error:?}"))).collect().await
}

/// Waits until `done` holds, letting the other tasks run.
async fn eventually(mut done: impl FnMut() -> bool) {
    for _ in 0..100_000 {
        if done() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the condition never held");
}

fn created(id: &str) -> Value {
    json!({"type": "response.created", "response": {"id": id, "status": "in_progress", "output": []}})
}

fn delta(id: &str, text: &str) -> Value {
    json!({
        "type": "response.output_text.delta",
        "item_id": format!("msg_{id}"),
        "output_index": 0,
        "content_index": 0,
        "delta": text,
    })
}

fn message_item(id: &str, text: &str) -> Value {
    json!({
        "id": format!("msg_{id}"),
        "type": "message",
        "role": "assistant",
        "status": "completed",
        "content": [{"type": "output_text", "text": text, "annotations": []}],
    })
}

fn completed(id: &str) -> Value {
    json!({
        "type": "response.completed",
        "response": {
            "id": id,
            "status": "completed",
            "output": [],
            "usage": {"input_tokens": 10, "output_tokens": 2},
        },
    })
}

/// The events of a whole answer `text` with the id `id`.
fn answer(id: &str, text: &str) -> Vec<Value> {
    vec![
        created(id),
        delta(id, text),
        json!({"type": "response.output_item.done", "output_index": 0, "item": message_item(id, text)}),
        completed(id),
    ]
}

fn user_item(text: &str) -> Value {
    json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]})
}

const FIXTURES: &[&str] = &[
    "plain_text.sse",
    "tool_call.sse",
    "custom_tool_call.sse",
    "reasoning_round_trip.sse",
    "incomplete_max_tokens.sse",
    "error_event.sse",
    "failed_rate_limit.sse",
    "context_length_exceeded.sse",
];

#[tokio::test]
async fn the_websocket_answer_equals_the_http_answer_for_every_fixture() {
    for name in FIXTURES {
        let events = sse_events(&fixture(name));
        let socket = Socket::serving(vec![vec![Step::Send(events)]]);
        let over_socket = setup(vec![socket], Vec::new(), WebSocketMode::On).await;
        let over_http = setup(Vec::new(), vec![fixture(name)], WebSocketMode::Off).await;

        let model = "gpt-5.5";
        let by_socket =
            collect(over_socket.provider.stream(request(model, "hi")).await.unwrap()).await;
        let by_http = collect(over_http.provider.stream(request(model, "hi")).await.unwrap()).await;

        assert_eq!(by_socket, by_http, "{name}");
        assert_eq!(over_socket.server.sockets().len(), 1, "{name}");
        assert!(over_socket.server.posts().is_empty(), "{name}");
        assert_eq!(over_http.server.posts().len(), 1, "{name}");
        assert!(over_http.server.sockets().is_empty(), "{name}");
    }
}

#[tokio::test]
async fn the_handshake_carries_the_headers_of_the_http_path_and_the_create_its_body() {
    let socket = Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))]]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;

    let completion = setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();

    assert_eq!(completion.message.text(), "Hello.");
    let sockets = setup.server.sockets();
    let socket = &sockets[0];
    assert_eq!(socket.path, "/backend-api/codex/responses");
    assert_eq!(socket.headers["authorization"], "Bearer eyJ.access.one");
    assert_eq!(socket.headers["chatgpt-account-id"], "acct_7d1f");
    assert_eq!(socket.headers["originator"], "efr");
    assert_eq!(socket.headers["session-id"], KEY);
    assert_eq!(socket.headers["openai-beta"], "responses_websockets=2026-02-06");
    assert!(socket.headers["user-agent"].starts_with("efr/"));
    let create = &socket.messages[0];
    assert_eq!(create["type"], "response.create");
    assert_eq!(create["model"], "gpt-5.5");
    assert_eq!(create["stream"], true);
    assert_eq!(create["store"], false);
    assert_eq!(create["instructions"], "You are efr.");
    assert_eq!(create["prompt_cache_key"], KEY);
    assert_eq!(create["input"], json!([user_item("hi")]));
    assert!(create.get("previous_response_id").is_none());
}

#[tokio::test]
async fn the_next_call_reuses_the_connection_and_sends_only_the_new_items() {
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-5.5", "hi");

    let first = setup.provider.complete(request.clone()).await.unwrap();
    request.messages.push(first.message);
    request.messages.push(Message::user("and now?"));
    let second = setup.provider.complete(request).await.unwrap();

    assert_eq!(second.message.text(), "Again.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 1);
    let next = &sockets[0].messages[1];
    assert_eq!(next["previous_response_id"], "resp_1");
    assert_eq!(next["input"], json!([user_item("and now?")]));
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_changed_setting_sends_the_whole_input() {
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-5.5", "hi");

    let first = setup.provider.complete(request.clone()).await.unwrap();
    request.messages.push(first.message);
    request.messages.push(Message::user("think harder"));
    request.provider_options.insert("reasoning_effort".to_owned(), json!("high"));
    setup.provider.complete(request).await.unwrap();

    let next = &setup.server.sockets()[0].messages[1];
    assert!(next.get("previous_response_id").is_none());
    assert_eq!(next["input"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_history_rebuilt_by_a_compaction_sends_the_whole_input() {
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let request_one = request("gpt-5.5", "hi");

    setup.provider.complete(request_one.clone()).await.unwrap();
    // A compaction replaces the history with its summary and the newest prompt.
    let mut rebuilt = request_one;
    rebuilt.messages =
        vec![Message::user("Summary: the user said hi."), Message::user("what next?")];
    setup.provider.complete(rebuilt).await.unwrap();

    let next = &setup.server.sockets()[0].messages[1];
    assert!(next.get("previous_response_id").is_none());
    assert_eq!(
        next["input"],
        json!([user_item("Summary: the user said hi."), user_item("what next?")])
    );
}

#[tokio::test]
async fn a_dropped_stream_interrupts_the_answer_and_the_connection_serves_the_next_call() {
    let mut interrupted = completed("resp_1");
    interrupted["type"] = json!("response.incomplete");
    interrupted["response"]["status"] = json!("incomplete");
    interrupted["response"]["incomplete_details"] = json!({"reason": "interrupted"});
    let accepted = json!({"type": "response.interrupt.accepted", "response_id": "resp_1"});
    let socket = Socket::serving(vec![
        vec![
            Step::Send(vec![created("resp_1"), delta("resp_1", "Wor")]),
            Step::AfterNext(vec![accepted, interrupted]),
        ],
        vec![Step::Send(answer("resp_2", "Done."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-5.5", "hi");

    let mut stream = setup.provider.stream(request.clone()).await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first, ProviderEvent::TextDelta { text: "Wor".to_owned() });
    drop(stream);
    eventually(|| !setup.provider.sockets().is_busy(KEY)).await;
    request.messages.push(Message::assistant("Wor"));
    request.messages.push(Message::user("go on"));
    let completion = setup.provider.complete(request).await.unwrap();

    assert_eq!(completion.message.text(), "Done.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 1);
    assert_eq!(
        sockets[0].messages[1],
        json!({"type": "response.interrupt", "response_id": "resp_1", "mode": "discard_partial_items"})
    );
    let next = &sockets[0].messages[2];
    assert_eq!(next["type"], "response.create");
    assert!(next.get("previous_response_id").is_none());
}

#[tokio::test]
async fn a_refused_upgrade_sends_the_call_over_http_and_pauses_websockets() {
    let posts = vec![fixture("plain_text.sse"), fixture("plain_text.sse")];
    let setup = setup(vec![Socket::refused(426)], posts, WebSocketMode::Auto).await;

    let first = setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();
    let second = setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();

    assert_eq!(first.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(second.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(setup.server.posts().len(), 2);
    // The second call did not try the socket again.
    assert_eq!(setup.server.sockets().len(), 1);
    assert!(setup.server.posts()[0].get("type").is_none());
}

#[tokio::test]
async fn a_close_before_the_answer_sends_the_call_over_http_and_the_next_call_reconnects() {
    let sockets = vec![
        Socket::serving(vec![vec![Step::Close]]),
        Socket::serving(vec![vec![Step::Send(answer("resp_2", "Back."))]]),
    ];
    let setup = setup(sockets, vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    let first = setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();
    let second = setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();

    assert_eq!(first.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(second.message.text(), "Back.");
    assert_eq!(setup.server.posts().len(), 1);
    assert_eq!(setup.server.sockets().len(), 2);
}

#[tokio::test]
async fn a_close_after_the_answer_started_fails_the_call_as_on_http() {
    let socket = Socket::serving(vec![vec![
        Step::Send(vec![created("resp_1"), delta("resp_1", "Wor")]),
        Step::Close,
    ]]);
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    let events = collect(setup.provider.stream(request("gpt-5.5", "hi")).await.unwrap()).await;

    assert_eq!(
        events,
        vec![Ok(ProviderEvent::TextDelta { text: "Wor".to_owned() }), Err("Incomplete".to_owned())]
    );
    // The model may have run, so the call is not sent a second time.
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn an_error_event_before_the_answer_sends_the_call_over_http_and_pauses_websockets() {
    let error = json!({
        "type": "error",
        "status": 400,
        "error": {"type": "invalid_request_error", "message": "websockets are not on for you"},
    });
    let socket = Socket::serving(vec![vec![Step::Send(vec![error])]]);
    let posts = vec![fixture("plain_text.sse"), fixture("plain_text.sse")];
    let setup = setup(vec![socket], posts, WebSocketMode::Auto).await;

    let first = setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();
    setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();

    assert_eq!(first.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(setup.server.posts().len(), 2);
    assert_eq!(setup.server.sockets().len(), 1);
}

#[tokio::test]
async fn a_lost_previous_answer_sends_the_call_over_http_without_a_pause() {
    let lost = json!({
        "type": "error",
        "status": 400,
        "error": {"type": "invalid_request_error", "code": "previous_response_not_found", "message": "gone"},
    });
    let sockets = vec![
        Socket::serving(vec![
            vec![Step::Send(answer("resp_1", "Hello."))],
            vec![Step::Send(vec![lost])],
        ]),
        Socket::serving(vec![vec![Step::Send(answer("resp_3", "Fresh."))]]),
    ];
    let setup = setup(sockets, vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;
    let mut request = request("gpt-5.5", "hi");

    let first = setup.provider.complete(request.clone()).await.unwrap();
    request.messages.push(first.message);
    request.messages.push(Message::user("next"));
    let second = setup.provider.complete(request.clone()).await.unwrap();
    let third = setup.provider.complete(request).await.unwrap();

    assert_eq!(second.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(third.message.text(), "Fresh.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 2);
    assert_eq!(sockets[0].messages[1]["previous_response_id"], "resp_1");
    assert!(sockets[1].messages[0].get("previous_response_id").is_none());
    // The HTTP body never names a previous answer: with `store: false` only the
    // socket holds it.
    assert!(setup.server.posts()[0].get("previous_response_id").is_none());
}

#[tokio::test]
async fn the_switch_and_the_model_choose_the_transport() {
    // (mode, model, with a conversation key, goes over the socket)
    let cases = [
        (WebSocketMode::Auto, "gpt-5.5", true, true),
        (WebSocketMode::Auto, "o3", true, false),
        (WebSocketMode::Auto, "gpt-5.5", false, false),
        (WebSocketMode::On, "o3", true, true),
        (WebSocketMode::Off, "gpt-5.5", true, false),
    ];
    for (mode, model, keyed, socket) in cases {
        let sockets = vec![Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hi."))]])];
        let setup = setup(sockets, vec![fixture("plain_text.sse")], mode).await;
        let mut request = request(model, "hi");
        if !keyed {
            request.provider_options.remove("prompt_cache_key");
        }

        setup.provider.complete(request).await.unwrap();

        let case = format!("{mode:?} {model} keyed={keyed}");
        assert_eq!(setup.server.sockets().len(), usize::from(socket), "{case}");
        assert_eq!(setup.server.posts().len(), usize::from(!socket), "{case}");
    }
}

#[tokio::test]
async fn the_catalogs_prefer_websockets_chooses_the_transport_from_the_next_call() {
    let sockets = vec![Socket::serving(vec![vec![Step::Send(answer("resp_2", "Hi."))]])];
    let setup = setup(sockets, vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;
    let catalog = ModelCatalog::new(Catalog::builtin(Backend::Subscription));
    let provider = setup.provider.with_catalog(catalog.clone());
    let fetch = |prefers: bool| {
        let body = json!({"models": [{
            "slug": "gpt-5.5", "visibility": "list", "priority": 1,
            "supported_reasoning_levels": [], "prefer_websockets": prefers,
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
    };

    fetch(false);
    provider.complete(request("gpt-5.5", "hi")).await.unwrap();
    assert_eq!(setup.server.posts().len(), 1, "the catalog says HTTP");
    assert_eq!(setup.server.sockets().len(), 0);

    fetch(true);
    provider.complete(request("gpt-5.5", "hi")).await.unwrap();
    assert_eq!(setup.server.posts().len(), 1);
    assert_eq!(setup.server.sockets().len(), 1, "the new catalog says WebSocket");
}

#[tokio::test]
async fn an_idle_connection_closes_and_the_next_call_opens_a_new_one() {
    let sockets = vec![
        Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))]]),
        Socket::serving(vec![vec![Step::Send(answer("resp_2", "Again."))]]),
    ];
    let setup = setup(sockets, Vec::new(), WebSocketMode::Auto).await;

    setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();
    setup.clock.advance(IDLE);
    eventually(|| setup.server.sockets()[0].closed).await;
    let second = setup.provider.complete(request("gpt-5.5", "hi")).await.unwrap();

    assert_eq!(second.message.text(), "Again.");
    assert_eq!(setup.server.sockets().len(), 2);
}
