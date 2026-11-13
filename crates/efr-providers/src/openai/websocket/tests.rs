use std::sync::Arc;
use std::time::Duration;

use efr_http::{HttpClient, HttpConfig, RetryPolicy};
use efr_provider::{
    HostedTool, Message, Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream,
    Request, Warmup, WebAccess,
};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{IDLE, PING_FIRST, PROBE_AFTER, PROBE_TIMEOUT};
use crate::openai::testing::{
    FakeTokens, FixedRng, ManualClock, ResponsesServer, Socket, Step, fixture, sse_events,
};
use crate::openai::{
    Backend, Catalog, Fetched, ModelCatalog, OpenAiConfig, OpenAiProvider, WebSocketMode,
};
use efr_stdx::time::Clock as _;

mod perf;

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
        Arc::new(FixedRng(0)),
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
    "web_search.sse",
    "web_search_failed.sse",
    "web_search_queries.sse",
    "web_search_no_action.sse",
    "web_search_steps.sse",
    "web_search_stored.sse",
];

#[tokio::test]
async fn the_websocket_answer_equals_the_http_answer_for_every_fixture() {
    for name in FIXTURES {
        let events = sse_events(&fixture(name));
        let socket = Socket::serving(vec![vec![Step::Send(events)]]);
        let over_socket = setup(vec![socket], Vec::new(), WebSocketMode::On).await;
        let over_http = setup(Vec::new(), vec![fixture(name)], WebSocketMode::Off).await;

        let model = "gpt-6.1-sol";
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

    let completion = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

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
    assert_eq!(create["model"], "gpt-6.1-sol");
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
    let mut request = request("gpt-6.1-sol", "hi");

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

/// A request of the conversation with the web tool of `access`, or without one.
fn web_request(access: Option<WebAccess>) -> Request {
    let mut request = request("gpt-6.1-sol", "Does ROB RUN run on Linux?");
    request.hosted =
        access.map(|access| HostedTool::web_search(access, Some(20))).into_iter().collect();
    request
}

#[tokio::test]
async fn the_next_call_after_a_web_search_continues_the_answer_with_only_the_new_items() {
    let socket = Socket::serving(vec![
        vec![Step::Send(sse_events(&fixture("web_search.sse")))],
        vec![Step::Send(answer("resp_2", "Yes, with Proton 9."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = web_request(Some(WebAccess::Live));

    let first = setup.provider.complete(request.clone()).await.unwrap();
    let raw = first.message.provider_raw.clone().unwrap();
    assert_eq!(raw[0]["type"], "web_search_call");
    request.messages.push(first.message);
    request.messages.push(Message::user("And on the Steam Deck?"));
    let second = setup.provider.complete(request).await.unwrap();

    assert_eq!(second.message.text(), "Yes, with Proton 9.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 1);
    let create = &sockets[0].messages[0];
    assert_eq!(create["tools"], json!([{"type": "web_search", "external_web_access": true}]));
    let next = &sockets[0].messages[1];
    assert_eq!(next["previous_response_id"], "resp_web1");
    assert_eq!(next["input"], json!([user_item("And on the Steam Deck?")]));
    assert!(setup.server.posts().is_empty());
}

#[rstest::rstest]
#[case::stored_to_live(Some(WebAccess::Live))]
#[case::web_turned_off(None)]
#[tokio::test]
async fn a_change_of_the_web_plan_starts_a_new_chain(#[case] next: Option<WebAccess>) {
    let socket = Socket::serving(vec![
        vec![Step::Send(sse_events(&fixture("web_search.sse")))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = web_request(Some(WebAccess::Stored));

    let first = setup.provider.complete(request.clone()).await.unwrap();
    request.messages.push(first.message);
    request.messages.push(Message::user("And on the Steam Deck?"));
    request.hosted = web_request(next).hosted;
    setup.provider.complete(request).await.unwrap();

    let next_create = &setup.server.sockets()[0].messages[1];
    assert!(next_create.get("previous_response_id").is_none());
    let input = next_create["input"].as_array().unwrap();
    if next.is_some() {
        // The web items go back with the tool: the user item, the four output items,
        // the new user item.
        assert_eq!(input.len(), 6);
        assert_eq!(input[1]["type"], "web_search_call");
    } else {
        // Without the tool the answer goes as its text.
        assert_eq!(input.len(), 3);
        assert!(!next_create.to_string().contains("web_search"), "{next_create}");
    }
}

#[tokio::test]
async fn a_changed_setting_sends_the_whole_input() {
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-6.1-sol", "hi");

    let first = setup.provider.complete(request.clone()).await.unwrap();
    request.messages.push(first.message);
    request.messages.push(Message::user("think harder"));
    request.effort = Some("high".to_owned());
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
    let request_one = request("gpt-6.1-sol", "hi");

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
    let mut request = request("gpt-6.1-sol", "hi");

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

    let first = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    let second = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

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

    let first = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    let second = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

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

    let events = collect(setup.provider.stream(request("gpt-6.1-sol", "hi")).await.unwrap()).await;

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
        "status": 500,
        "error": {"type": "server_error", "message": "the websocket service failed"},
    });
    let socket = Socket::serving(vec![vec![Step::Send(vec![error])]]);
    let posts = vec![fixture("plain_text.sse"), fixture("plain_text.sse")];
    let setup = setup(vec![socket], posts, WebSocketMode::Auto).await;

    let first = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

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
    let mut request = request("gpt-6.1-sol", "hi");

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
        (WebSocketMode::Auto, "gpt-6.1-sol", true, true),
        (WebSocketMode::Auto, "o3", true, false),
        (WebSocketMode::Auto, "gpt-6.1-sol", false, false),
        (WebSocketMode::On, "o3", true, true),
        (WebSocketMode::Off, "gpt-6.1-sol", true, false),
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
            "slug": "gpt-6.1-sol", "visibility": "list", "priority": 1,
            "supported_reasoning_levels": [], "prefer_websockets": prefers,
        }]});
        let (entries, _) = crate::openai::catalog::entries_of(&body).unwrap();
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
    provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    assert_eq!(setup.server.posts().len(), 1, "the catalog says HTTP");
    assert_eq!(setup.server.sockets().len(), 0);

    fetch(true);
    provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
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

    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.clock.advance(IDLE);
    eventually(|| setup.server.sockets()[0].closed).await;
    let second = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

    assert_eq!(second.message.text(), "Again.");
    assert_eq!(setup.server.sockets().len(), 2);
}

#[tokio::test]
async fn a_call_after_a_quiet_spell_pings_and_reuses_a_connection_that_answers() {
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-6.1-sol", "hi");

    let first = setup.provider.complete(request.clone()).await.unwrap();
    setup.clock.advance(PROBE_AFTER);
    request.messages.push(first.message);
    request.messages.push(Message::user("and now?"));
    let second = setup.provider.complete(request).await.unwrap();

    assert_eq!(second.message.text(), "Again.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 1);
    assert_eq!(sockets[0].messages[1]["previous_response_id"], "resp_1");
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_call_after_a_quiet_spell_sends_its_request_before_the_pong() {
    // The server reads each ping and never answers it, so a call that waits for the
    // pong never sends its request.
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ])
    .without_pongs();
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-6.1-sol", "hi");

    let first = setup.provider.complete(request.clone()).await.unwrap();
    setup.clock.advance(PROBE_AFTER);
    request.messages.push(first.message);
    request.messages.push(Message::user("and now?"));
    let (second, ()) = tokio::join!(setup.provider.complete(request), async {
        eventually(|| setup.server.sockets()[0].messages.len() == 2).await;
    });

    assert_eq!(second.unwrap().message.text(), "Again.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 1);
    assert_eq!(sockets[0].messages[1]["previous_response_id"], "resp_1");
    assert!(setup.server.posts().is_empty());
    // The answer showed that the connection works, so its ping time no longer runs.
    assert!(!setup.clock.waits_for(PROBE_TIMEOUT));
}

#[tokio::test]
async fn a_quiet_connection_that_answers_its_ping_keeps_a_slow_call_off_http() {
    // Past its script the server reads the second request, answers the ping and sends
    // nothing.
    let socket = Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))]]);
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.clock.advance(PROBE_AFTER);
    let silence = async {
        eventually(|| {
            setup.server.sockets()[0].messages.len() == 2 && !setup.clock.waits_for(PROBE_TIMEOUT)
        })
        .await;
        setup.clock.advance(PROBE_TIMEOUT);
        eventually(|| setup.clock.waits_for(super::READ_TIMEOUT - PROBE_TIMEOUT)).await;
        setup.clock.advance(super::READ_TIMEOUT);
    };
    let (stream, ()) = tokio::join!(setup.provider.stream(request("gpt-6.1-sol", "hi")), silence);
    let events = collect(stream.unwrap()).await;

    // The pong showed a live server that has the request, so the silence fails the
    // call at the read timeout and the call does not go out again.
    assert_eq!(events.len(), 1);
    assert!(events[0].as_ref().unwrap_err().starts_with("Transport"), "{events:?}");
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_quiet_connection_that_breaks_before_its_pong_and_any_event_sends_the_call_over_http() {
    let socket =
        Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))], vec![Step::Drop]])
            .without_pongs();
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.clock.advance(PROBE_AFTER);
    let second = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

    // No pong and no event came, so the server started no answer: HTTP sends the call.
    assert_eq!(second.message.text(), "Your shell is zsh 5.9.");
    assert_eq!(setup.server.posts().len(), 1);
}

#[tokio::test]
async fn a_quiet_connection_that_died_without_a_close_fails_its_ping_and_the_call_goes_over_http() {
    let sockets = vec![
        Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello.")), Step::Hang]]),
        Socket::serving(vec![vec![Step::Send(answer("resp_3", "Fresh."))]]),
    ];
    let setup = setup(sockets, vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.clock.advance(PROBE_AFTER);
    let pong_never_comes = async {
        eventually(|| setup.clock.waits_for(PROBE_TIMEOUT)).await;
        setup.clock.advance(PROBE_TIMEOUT);
    };
    let (second, ()) =
        tokio::join!(setup.provider.complete(request("gpt-6.1-sol", "hi")), pong_never_comes);
    let third = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

    // The request went out with the ping, but the dead connection gave neither a pong
    // nor an event, so HTTP sent the call. This server never reads again; a server
    // that is only late can still read the request (see the next tests).
    assert_eq!(second.unwrap().message.text(), "Your shell is zsh 5.9.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets[0].messages.len(), 1);
    assert_eq!(setup.server.posts().len(), 1);
    // A dead connection does not pause WebSockets: the next call opens a new one.
    assert_eq!(third.message.text(), "Fresh.");
    assert_eq!(sockets.len(), 2);
}

#[tokio::test]
async fn a_long_quiet_connection_sends_its_request_only_after_the_pong() {
    // The server stops reading for longer than the ping may take, then reads again.
    let late = Arc::new(tokio::sync::Notify::new());
    let socket = Socket::serving(vec![vec![
        Step::Send(answer("resp_1", "Hello.")),
        Step::Wait(Arc::clone(&late)),
    ]]);
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.clock.advance(PING_FIRST);
    let stall = async {
        eventually(|| setup.clock.waits_for(PROBE_TIMEOUT)).await;
        setup.clock.advance(PROBE_TIMEOUT);
    };
    let (second, ()) = tokio::join!(setup.provider.complete(request("gpt-6.1-sol", "hi")), stall);
    assert_eq!(second.unwrap().message.text(), "Your shell is zsh 5.9.");
    assert_eq!(setup.server.posts().len(), 1);

    // The server reads again: it finds the ping and the close, not the request that
    // HTTP sent, so the model runs one time.
    late.notify_one();
    // NOTE: the fake reads on its own task in real time.
    for _ in 0..400 {
        let socket = &setup.server.sockets()[0];
        if socket.closed || socket.messages.len() > 1 {
            break;
        }
        efr_stdx::time::SystemClock.sleep(Duration::from_millis(5)).await;
    }
    let socket = &setup.server.sockets()[0];
    assert!(socket.closed, "the connection closed");
    assert_eq!(socket.messages.len(), 1);
}

#[tokio::test]
async fn a_quiet_connection_that_breaks_right_after_its_pong_fails_the_call_without_http() {
    for _ in 0..10 {
        // The server answers the ping, reads the request and drops the connection.
        let socket =
            Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))], vec![Step::Drop]]);
        let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

        setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
        setup.clock.advance(PROBE_AFTER);
        let second = setup.provider.complete(request("gpt-6.1-sol", "hi")).await;

        // The pong showed a live server that has the request: the break is the call's
        // failure, and the call does not go out again.
        assert_eq!(setup.server.sockets()[0].messages.len(), 2);
        assert!(second.is_err(), "{second:?}");
        assert!(setup.server.posts().is_empty());
    }
}

#[tokio::test]
async fn a_connection_that_breaks_after_the_request_went_out_fails_the_call_without_http() {
    let socket = Socket::serving(vec![vec![Step::Drop]]);
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    let events = collect(setup.provider.stream(request("gpt-6.1-sol", "hi")).await.unwrap()).await;

    assert_eq!(events.len(), 1);
    let error = events[0].as_ref().unwrap_err();
    assert!(error.starts_with("Transport"), "{error}");
    // The server may have started the model, so the call does not go out again.
    assert_eq!(setup.server.sockets()[0].messages.len(), 1);
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_server_that_says_nothing_after_the_request_fails_the_call_at_the_read_timeout() {
    let socket = Socket::serving(vec![vec![Step::Hang]]);
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    let silence = async {
        eventually(|| setup.clock.waits_for(super::READ_TIMEOUT)).await;
        setup.clock.advance(super::READ_TIMEOUT);
    };
    let (stream, ()) = tokio::join!(setup.provider.stream(request("gpt-6.1-sol", "hi")), silence);
    let events = collect(stream.unwrap()).await;

    assert_eq!(events.len(), 1);
    assert!(events[0].as_ref().unwrap_err().starts_with("Transport"), "{events:?}");
    assert!(setup.server.posts().is_empty());
}

/// An `error` event with `status` and the error object `error`.
fn error_event(status: u16, error: Value) -> Value {
    json!({"type": "error", "status": status, "error": error})
}

#[tokio::test]
async fn an_error_status_before_the_answer_fails_the_call_as_on_http_without_a_pause() {
    let overflow = error_event(
        400,
        json!({"type": "invalid_request_error", "code": "context_length_exceeded", "message": "Your input exceeds the context window of this model."}),
    );
    let mut limited =
        error_event(429, json!({"type": "rate_limit_exceeded", "message": "Rate limit reached."}));
    limited["headers"] = json!({"retry-after": "7"});
    let sockets = vec![
        Socket::serving(vec![vec![Step::Send(vec![overflow])]]),
        Socket::serving(vec![vec![Step::Send(vec![limited])]]),
        Socket::serving(vec![vec![Step::Send(answer("resp_3", "Fresh."))]]),
    ];
    let setup = setup(sockets, vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    let overflow = setup.provider.stream(request("gpt-6.1-sol", "hi")).await.err().unwrap();
    let limited = setup.provider.stream(request("gpt-6.1-sol", "hi")).await.err().unwrap();
    let third = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

    assert!(overflow.is_context_overflow(), "{overflow:?}");
    assert!(
        matches!(limited, ProviderError::RateLimited { retry_after: Some(wait) } if wait == Duration::from_secs(7)),
        "{limited:?}"
    );
    // The server answered each request, so neither went out a second time over HTTP,
    // and WebSockets stay on: the third call opens a new connection.
    assert!(setup.server.posts().is_empty());
    assert_eq!(third.message.text(), "Fresh.");
    assert_eq!(setup.server.sockets().len(), 3);
}

#[tokio::test]
async fn a_second_refused_token_in_a_row_pauses_websockets() {
    let refused =
        || error_event(401, json!({"type": "invalid_request_error", "message": "token rejected"}));
    // (the first socket, the second socket): refused in the event or at the upgrade.
    let cases = [
        (
            Socket::serving(vec![vec![Step::Send(vec![refused()])]]),
            Socket::serving(vec![vec![Step::Send(vec![refused()])]]),
        ),
        (Socket::refused(401), Socket::refused(401)),
    ];
    for (first, second) in cases {
        let posts = vec![fixture("plain_text.sse"); 3];
        let setup = setup(vec![first, second], posts, WebSocketMode::Auto).await;

        for _ in 0..3 {
            setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
        }

        // The first refusal tries the socket again on the next call; the second one
        // pauses it, so the third call goes straight over HTTP.
        assert_eq!(setup.server.sockets().len(), 2);
        assert_eq!(setup.server.posts().len(), 3);
    }
}

#[tokio::test]
async fn a_second_connection_for_one_conversation_does_not_cut_the_answer_of_the_first() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let rest = vec![
        json!({"type": "response.output_item.done", "output_index": 0, "item": message_item("resp_1", "World.")}),
        completed("resp_1"),
    ];
    let server = ResponsesServer::start(
        vec![
            Socket::serving(vec![vec![
                Step::Send(vec![created("resp_1"), delta("resp_1", "World.")]),
                Step::Wait(Arc::clone(&gate)),
                Step::Send(rest),
            ]]),
            Socket::serving(vec![vec![Step::Send(answer("resp_2", "Two."))]]),
        ],
        Vec::new(),
    )
    .await;
    let clock = Arc::new(ManualClock::new());
    let http =
        HttpClient::new(&HttpConfig::default(), clock.clone(), Arc::new(FixedRng(0))).unwrap();
    let url = format!("{}/responses", server.uri());
    let connect = || async {
        let request = efr_http::HttpRequest::get(&url).unwrap();
        http.websocket(&request).await.map_err(|error| super::ConnectError {
            reason: error.to_string(),
            health: super::Health::Broken,
        })
    };
    let sockets = super::Sockets::new(clock.clone());
    let body = crate::openai::convert::request_body(
        &request("gpt-6.1-sol", "hi"),
        &OpenAiConfig::subscription(),
        None,
    );
    let stream = |attempt: super::Attempt| match attempt {
        super::Attempt::Answered(stream) => stream,
        _ => panic!("the socket did not serve the call"),
    };

    // The later call finds no connection, so it opens one, and waits in its handshake.
    let (open, opened) = tokio::sync::oneshot::channel::<()>();
    let later = sockets.stream(
        KEY,
        &body,
        || async {
            opened.await.unwrap();
            connect().await
        },
        &efr_provider::NoNotices,
        crate::openai::timing::Timing::start(),
        tracing::Span::none(),
    );
    let mut later = std::pin::pin!(later);
    assert!(futures::poll!(&mut later).is_pending());
    // The first call opens its own connection, and its answer starts.
    let first = stream(
        sockets
            .stream(
                KEY,
                &body,
                connect,
                &efr_provider::NoNotices,
                crate::openai::timing::Timing::start(),
                tracing::Span::none(),
            )
            .await,
    );
    // The later call's connection opens while the first answer runs, and serves it.
    open.send(()).unwrap();
    let later = collect(stream(later.await)).await;
    gate.notify_one();
    let first = collect(first).await;

    assert!(later.last().unwrap().is_ok(), "{later:?}");
    assert!(first.iter().all(Result::is_ok), "{first:?}");
    assert!(matches!(first.last(), Some(Ok(ProviderEvent::Done { .. }))), "{first:?}");
    assert_eq!(server.sockets().len(), 2);
}

/// The notices of one model call, in order.
#[derive(Debug, Default)]
struct Heard(std::sync::Mutex<Vec<efr_provider::CallNotice>>);

impl efr_provider::CallNotices for Heard {
    fn notice(&self, notice: efr_provider::CallNotice) {
        self.0.lock().unwrap().push(notice);
    }
}

#[tokio::test]
async fn a_new_connection_tells_its_connect_and_a_reused_one_tells_nothing() {
    use efr_provider::CallNotice;
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-6.1-sol", "hi");

    let first = Heard::default();
    let stream = setup.provider.stream_with_notices(request.clone(), &first).await.unwrap();
    let answered = efr_provider::Completion::collect(stream).await.unwrap();
    assert_eq!(first.0.into_inner().unwrap(), [CallNotice::Connecting, CallNotice::Connected]);
    request.messages.push(answered.message);
    request.messages.push(Message::user("and now?"));
    let second = Heard::default();
    let stream = setup.provider.stream_with_notices(request, &second).await.unwrap();
    collect(stream).await;

    assert_eq!(second.0.into_inner().unwrap(), []);
    assert_eq!(setup.server.sockets().len(), 1);
}

fn warmup() -> Warmup {
    Warmup::new("gpt-6.1-sol", Some(KEY.to_owned()))
}

#[tokio::test]
async fn a_warm_up_opens_the_socket_and_sends_no_request_so_the_call_takes_it() {
    let socket = Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))]]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;

    let warmed = setup.provider.warm(&warmup()).await;
    assert!(warmed);
    eventually(|| setup.server.sockets().len() == 1).await;
    // The socket waits for the call: no model ran.
    assert!(setup.server.sockets()[0].messages.is_empty());
    let first = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

    assert_eq!(first.message.text(), "Hello.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 1);
    assert_eq!(sockets[0].messages.len(), 1);
    assert_eq!(sockets[0].pings, 0);
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_call_waits_for_the_handshake_of_its_warm_up_and_opens_no_second_socket() {
    let socket = Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))]]);
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;

    let warmup = warmup();
    let (warmed, first) = tokio::join!(
        setup.provider.warm(&warmup),
        setup.provider.complete(request("gpt-6.1-sol", "hi"))
    );

    assert!(warmed);
    assert_eq!(first.unwrap().message.text(), "Hello.");
    assert_eq!(setup.server.sockets().len(), 1);
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_call_that_waits_for_the_handshake_of_its_warm_up_tells_its_connect() {
    use efr_provider::CallNotice;
    let socket = Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))]]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;

    let warmup = warmup();
    let heard = Heard::default();
    let call = async {
        // NOTE: the warm-up starts its handshake first.
        eventually(|| setup.provider.sockets().is_opening(KEY)).await;
        let stream = setup.provider.stream_with_notices(request("gpt-6.1-sol", "hi"), &heard).await;
        collect(stream.unwrap()).await
    };
    let (warmed, answer) = tokio::join!(setup.provider.warm(&warmup), call);

    assert!(warmed);
    assert!(answer.last().unwrap().is_ok(), "{answer:?}");
    assert_eq!(heard.0.into_inner().unwrap(), [CallNotice::Connecting, CallNotice::Connected]);
    assert_eq!(setup.server.sockets().len(), 1);
}

#[tokio::test]
async fn a_warm_up_pings_a_long_quiet_socket_so_the_call_after_the_pong_sends_no_ping() {
    let socket = Socket::serving(vec![
        vec![Step::Send(answer("resp_1", "Hello."))],
        vec![Step::Send(answer("resp_2", "Again."))],
    ]);
    let setup = setup(vec![socket], Vec::new(), WebSocketMode::Auto).await;
    let mut request = request("gpt-6.1-sol", "hi");
    let first = setup.provider.complete(request.clone()).await.unwrap();
    setup.clock.advance(PING_FIRST);

    let warmed = setup.provider.warm(&warmup()).await;
    assert!(warmed);
    // NOTE: whether the pong came first or the call took over the open ping, the call
    // sends no ping of its own.
    eventually(|| setup.server.sockets()[0].pings == 1).await;
    request.messages.push(first.message);
    request.messages.push(Message::user("and now?"));
    let second = setup.provider.complete(request).await.unwrap();

    assert_eq!(second.message.text(), "Again.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 1);
    assert_eq!(sockets[0].pings, 1, "the call sent no ping of its own");
    assert_eq!(sockets[0].messages[1]["previous_response_id"], "resp_1");
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_call_that_takes_over_the_ping_of_a_warm_up_waits_the_whole_probe_time_from_its_request()
{
    // The server reads the ping and the request, then says nothing, as on a path that
    // stalls.
    let socket =
        Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))], vec![Step::Hang]])
            .without_pongs();
    let setup = setup(vec![socket], vec![fixture("plain_text.sse")], WebSocketMode::Auto).await;
    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.clock.advance(PROBE_AFTER);
    assert!(setup.provider.warm(&warmup()).await);
    eventually(|| setup.server.sockets()[0].pings == 1).await;
    // Enter comes near the end of the probe time of the warm-up's ping.
    let late = Duration::from_millis(100);
    setup.clock.advance(PROBE_TIMEOUT - late);

    let stall = async {
        eventually(|| setup.server.sockets()[0].messages.len() == 2).await;
        setup.clock.advance(late);
        // The request has the whole probe time: the call stays on the socket.
        eventually(|| setup.clock.waits_for(PROBE_TIMEOUT - late)).await;
        assert!(setup.server.posts().is_empty());
        setup.clock.advance(PROBE_TIMEOUT - late);
    };
    let (second, ()) = tokio::join!(setup.provider.complete(request("gpt-6.1-sol", "hi")), stall);

    assert_eq!(second.unwrap().message.text(), "Your shell is zsh 5.9.");
    assert_eq!(setup.server.posts().len(), 1);
}

#[tokio::test]
async fn a_warm_up_closes_a_quiet_socket_that_gives_no_pong_and_the_call_opens_a_new_one() {
    let sockets = vec![
        Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello.")), Step::Hang]]),
        Socket::serving(vec![vec![Step::Send(answer("resp_2", "Fresh."))]]),
    ];
    let setup = setup(sockets, Vec::new(), WebSocketMode::Auto).await;
    setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();
    setup.clock.advance(PROBE_AFTER);

    let warmed = setup.provider.warm(&warmup()).await;
    assert!(warmed);
    eventually(|| setup.clock.waits_for(PROBE_TIMEOUT)).await;
    setup.clock.advance(PROBE_TIMEOUT);
    eventually(|| !setup.provider.sockets().is_busy(KEY) && !setup.provider.sockets().has(KEY))
        .await;
    // The next warm-up opens a new socket before the call.
    let warmed = setup.provider.warm(&warmup()).await;
    assert!(warmed);
    eventually(|| setup.server.sockets().len() == 2).await;
    let second = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

    assert_eq!(second.message.text(), "Fresh.");
    let sockets = setup.server.sockets();
    assert_eq!(sockets.len(), 2);
    assert_eq!(sockets[1].messages.len(), 1);
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_warm_up_that_cannot_connect_leaves_the_call_free_to_open_the_socket() {
    let sockets = vec![
        Socket::refused(503),
        Socket::serving(vec![vec![Step::Send(answer("resp_1", "Hello."))]]),
    ];
    let setup = setup(sockets, Vec::new(), WebSocketMode::Auto).await;

    assert!(setup.provider.warm(&warmup()).await);
    eventually(|| setup.server.sockets().len() == 1).await;
    let first = setup.provider.complete(request("gpt-6.1-sol", "hi")).await.unwrap();

    assert_eq!(first.message.text(), "Hello.");
    assert_eq!(setup.server.sockets().len(), 2);
    assert!(setup.server.posts().is_empty());
}

#[tokio::test]
async fn a_warm_up_opens_nothing_without_a_conversation_or_for_a_call_over_http() {
    let setup = setup(Vec::new(), Vec::new(), WebSocketMode::Auto).await;
    let without = Warmup::new("gpt-6.1-sol", None);
    assert!(setup.provider.warm(&without).await);
    let off = setup_off().await;
    let warmup = warmup();
    assert!(!off.provider.warm(&warmup).await);

    assert!(setup.server.sockets().is_empty());
    assert!(off.server.sockets().is_empty());
    assert!(setup.server.posts().is_empty());
    assert!(off.server.posts().is_empty());
}

async fn setup_off() -> Setup {
    setup(Vec::new(), Vec::new(), WebSocketMode::Off).await
}

#[tokio::test]
async fn a_warm_up_asks_for_the_token_so_one_that_ends_soon_is_refreshed() {
    let server = ResponsesServer::start(Vec::new(), Vec::new()).await;
    let clock = Arc::new(ManualClock::new());
    let http =
        HttpClient::new(&HttpConfig::default(), clock.clone(), Arc::new(FixedRng(0))).unwrap();
    let config = OpenAiConfig::subscription()
        .with_base_url(&format!("{}/backend-api/codex", server.uri()))
        .unwrap()
        .with_websocket(WebSocketMode::Off);
    let tokens = Arc::new(FakeTokens::new(&["eyJ.access.one"]));
    let provider = OpenAiProvider::new(
        ProviderId::new("openai-test").unwrap(),
        config,
        http,
        tokens.clone(),
        clock,
        Arc::new(FixedRng(0)),
    );

    let warmup = warmup();
    assert!(!provider.warm(&warmup).await);

    assert_eq!(tokens.fetches(), 1);
    assert!(server.posts().is_empty());
}
