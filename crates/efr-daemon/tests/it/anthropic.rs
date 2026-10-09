//! The `anthropic-api` provider end to end: the real Anthropic provider against the
//! local Messages API (`MessagesServer`). efrd has no list of Claude models until it
//! fetches one, so the first prompt waits for the fetch; the list then comes back from
//! its own cache file after a restart. A turn without a key says to log in.
//!
//! One conversation runs a Claude turn with a call of the `edit` tool and its approval,
//! a second turn, and an auto compaction before the second turn's call. Every request
//! body keeps the request before it as its prefix (outside the compaction), carries
//! the prompt cache markers S, A, P and T with their times to live, the `drop_block`
//! beta, the effort `medium` and no member of another provider; the turn's usage
//! carries the cache writes.

use efr_protocol::{
    AdminLogout, AdminLogoutResult, ApprovalDecision, ApprovalRespond, ApprovalRespondResult,
    CatalogOrigin, CompactionTrigger, ErrorCode, Event, EventEnvelope, Method, ModelsList,
    ModelsListResult, PromptSendResult, Usage,
};
use efr_test_daemon::{
    ANTHROPIC_API_KEY, ANTHROPIC_CREDENTIAL, MessagesAnswer, MessagesServer, TTY, TestDaemon,
    command_id, events_until,
};
use efr_test_support::Wait;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::support::{files_holding, logged, logs};

/// The model of every answer.
const MODEL: &str = "claude-opus-5-5";

/// The beta that `drop_block` needs.
const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The window of the model in the list: small, so one turn can pass the trigger of the
/// auto compaction (76%, 152000 tokens).
const WINDOW: u64 = 200_000;

/// The members that a Messages body may have; anything else, such as OpenAI's
/// `prompt_cache_key`, is a 400 at the API.
const BODY_MEMBERS: &[&str] = &[
    "model",
    "stream",
    "max_tokens",
    "system",
    "tools",
    "tool_choice",
    "thinking",
    "output_config",
    "messages",
];

/// The model in the list, with the small [`WINDOW`].
fn model() -> Value {
    let mut model = MessagesServer::model(MODEL);
    model["max_input_tokens"] = json!(WINDOW);
    model
}

/// The `message_start` of an answer with its usage: `input` tokens after the last
/// marker, `write_5m` and `write_1h` written to the cache, `read` read from it.
fn message_start(input: u64, write_5m: u64, write_1h: u64, read: u64) -> Value {
    json!({"message": {
        "id": "msg_efr", "type": "message", "role": "assistant", "content": [],
        "model": MODEL, "stop_reason": null, "stop_sequence": null,
        "usage": {
            "input_tokens": input,
            "cache_creation_input_tokens": write_5m + write_1h,
            "cache_read_input_tokens": read,
            "cache_creation": {
                "ephemeral_5m_input_tokens": write_5m,
                "ephemeral_1h_input_tokens": write_1h,
            },
            "output_tokens": 1,
        },
    }})
}

/// The `message_delta` that ends an answer with `stop_reason` and `output` tokens.
fn message_delta(stop_reason: &str, output: u64) -> Value {
    json!({
        "delta": {"stop_reason": stop_reason, "stop_sequence": null},
        "usage": {"output_tokens": output},
    })
}

/// An answer that thinks, signs its thinking and calls `edit` with `input`.
fn think_then_edit(call_id: &str, input: &Value) -> MessagesAnswer {
    let partial = input.to_string();
    let (head, tail) = partial.split_at(partial.len() / 2);
    let delta = |index: u32, delta: Value| json!({"index": index, "delta": delta});
    MessagesAnswer::events(&[
        ("message_start", message_start(4_000, 0, 3_000, 0)),
        (
            "content_block_start",
            json!({"index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        ),
        (
            "content_block_delta",
            delta(0, json!({"type": "thinking_delta", "thinking": "The greeting calls old."})),
        ),
        (
            "content_block_delta",
            delta(0, json!({"type": "signature_delta", "signature": "sig-efr-1"})),
        ),
        ("content_block_stop", json!({"index": 0})),
        (
            "content_block_start",
            json!({"index": 1, "content_block": {
                "type": "tool_use", "id": call_id, "name": "edit", "input": {},
            }}),
        ),
        (
            "content_block_delta",
            delta(1, json!({"type": "input_json_delta", "partial_json": head})),
        ),
        (
            "content_block_delta",
            delta(1, json!({"type": "input_json_delta", "partial_json": tail})),
        ),
        ("content_block_stop", json!({"index": 1})),
        ("message_delta", message_delta("tool_use", 50)),
        ("message_stop", json!({})),
    ])
}

/// A text answer with the usage of a long conversation: most of the input read from
/// the cache, a part written for five minutes.
fn long_answer(text: &str) -> MessagesAnswer {
    MessagesAnswer::events(&[
        ("message_start", message_start(10, 5_000, 0, 150_000)),
        ("content_block_start", json!({"index": 0, "content_block": {"type": "text", "text": ""}})),
        ("content_block_delta", json!({"index": 0, "delta": {"type": "text_delta", "text": text}})),
        ("content_block_stop", json!({"index": 0})),
        ("message_delta", message_delta("end_turn", 25_000)),
        ("message_stop", json!({})),
    ])
}

/// A daemon on the Anthropic provider against `server`, which lists [`model`], with
/// `src/lib.rs` in its working directory. The directory is no project, so an edit
/// asks.
async fn claude_daemon(server: &MessagesServer) -> TestDaemon {
    server.set_models(vec![model()]);
    let daemon = TestDaemon::builder().messages(server).persistent().start().await.unwrap();
    std::fs::create_dir_all(daemon.cwd().join("src")).unwrap();
    std::fs::write(daemon.cwd().join("src/lib.rs"), "fn main() {\n    old();\n}\n").unwrap();
    daemon
}

/// Sends prompt `n` and follows its turn to the end, answering every approval with
/// "yes". The events are the turn's own: the log of the conversation before it is
/// left out.
async fn run_turn(daemon: &TestDaemon, n: u128, text: &str) -> Vec<EventEnvelope> {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(n, text, TTY)).await.unwrap();
    let turn = sent.turn_id;
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = 100 * n;
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(
                event,
                Event::ApprovalRequested { turn_id, .. }
                    | Event::TurnCompleted { turn_id, .. }
                    | Event::TurnFailed { turn_id, .. }
                    if *turn_id == turn
            )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(turn)));
        let Event::ApprovalRequested { call_id, .. } = last else { break };
        answers += 1;
        let answer = Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(answers),
            conversation_id: sent.conversation_id,
            call_id,
            decision: ApprovalDecision::Allow,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    seen
}

async fn models(daemon: &TestDaemon) -> ModelsListResult {
    let client = daemon.client().await.unwrap();
    client.call(Method::ModelsList(ModelsList::default())).await.unwrap()
}

/// `value` without any `cache_control` member: what stays the same from one request to
/// the next while the markers move.
fn without_markers(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| key.as_str() != "cache_control")
                .map(|(key, value)| (key.clone(), without_markers(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_markers).collect()),
        other => other.clone(),
    }
}

/// Where `body` puts its markers, in prefix order: `system`, `tool`, or `message N`,
/// each with its time to live.
fn markers(body: &Value) -> Vec<(String, String)> {
    let ttl = |block: &Value| block["cache_control"]["ttl"].as_str().map(str::to_owned);
    let mut found = Vec::new();
    let blocks = |place: &str, blocks: &Value, found: &mut Vec<(String, String)>| {
        for block in blocks.as_array().into_iter().flatten() {
            if let Some(ttl) = ttl(block) {
                found.push((place.to_owned(), ttl));
            }
        }
    };
    blocks("tool", &body["tools"], &mut found);
    blocks("system", &body["system"], &mut found);
    for (index, message) in body["messages"].as_array().unwrap().iter().enumerate() {
        blocks(&format!("message {index}"), &message["content"], &mut found);
    }
    found
}

fn marks(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(place, ttl)| ((*place).to_owned(), (*ttl).to_owned())).collect()
}

/// True when the messages of `before` are the first messages of `after`, markers
/// left aside.
fn starts_with(after: &Value, before: &Value) -> bool {
    let after = without_markers(&after["messages"]);
    let before = without_markers(&before["messages"]);
    let (after, before) = (after.as_array().unwrap(), before.as_array().unwrap());
    after.len() >= before.len() && after[..before.len()] == before[..]
}

fn usage_of(events: &[EventEnvelope]) -> Usage {
    events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::TurnCompleted { usage, .. } => *usage,
            _ => None,
        })
        .unwrap_or_else(|| panic!("no completed turn with usage in {events:#?}"))
}

#[tokio::test]
async fn a_claude_conversation_edits_compacts_and_keeps_each_request_a_prefix_of_the_next() {
    let logs = logs();
    let server = MessagesServer::start().await;
    let edit = json!({"path": "src/lib.rs", "old_string": "old();", "new_string": "new();"});
    server.push(think_then_edit("toolu_efr_1", &edit));
    // NOTE: about 25000 tokens of answer, so the compaction's verbatim tail (20000)
    // holds only the next prompt and the summary covers the first turn.
    server.push(long_answer(&"The greeting now calls new. ".repeat(3_600)));
    server.push(MessagesAnswer::text("SUMMARY: src/lib.rs calls new()."));
    server.push(MessagesAnswer::text("Fine."));
    let daemon = claude_daemon(&server).await;

    let first = run_turn(&daemon, 1, "fix the greeting").await;
    let second = run_turn(&daemon, 2, "now the farewell").await;

    assert_eq!(server.remaining(), 0, "{:#?}", server.received());
    assert_eq!(
        std::fs::read_to_string(daemon.cwd().join("src/lib.rs")).unwrap(),
        "fn main() {\n    new();\n}\n",
        "the approved edit ran"
    );
    let approvals: Vec<&str> = first
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ApprovalRequested { summary, .. } => Some(summary.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    let tools: Vec<&str> = first
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallStarted { tool, .. } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tools, ["edit"]);
    let compaction = second
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::ConversationCompacted(compaction) => Some(compaction.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no compaction in {second:#?}"));
    assert_eq!(compaction.trigger, CompactionTrigger::Auto);
    assert_eq!(compaction.window, WINDOW);
    assert!(compaction.summary.as_deref().unwrap().starts_with("SUMMARY"), "{compaction:?}");

    let received = server.received();
    let bodies: Vec<&Value> = received.iter().map(|request| &request.body).collect();
    let [edit_call, answer_call, summary_call, after_call] = bodies[..] else {
        panic!("four calls, not {}", bodies.len())
    };
    for request in &received {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {ANTHROPIC_API_KEY}").as_str())
        );
        assert_eq!(request.api_key, None, "one credential header");
        assert_eq!(request.version.as_deref(), Some("2023-06-01"));
        assert_eq!(request.beta.as_deref(), Some(BINDING_BETA));
        let body = &request.body;
        let members: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(members.iter().all(|member| BODY_MEMBERS.contains(member)), "{members:?}");
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["output_config"], json!({"effort": "medium"}), "Claude Code's default");
        assert_eq!(
            body["thinking"],
            json!({
                "type": "adaptive",
                "display": "summarized",
                "block_binding": {"prefix_mismatch_behavior": "drop_block"},
            })
        );
        let tools: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(tools.contains(&"edit"), "{tools:?}");
        assert!(!tools.contains(&"apply_patch"), "a Claude model gets only the edit tool");
        assert_eq!(without_markers(&body["tools"]), without_markers(&edit_call["tools"]));
        assert_eq!(without_markers(&body["system"]), without_markers(&edit_call["system"]));
    }

    // The first call opens the turn: S and the tail are one-hour entries.
    assert_eq!(markers(edit_call), marks(&[("system", "1h"), ("message 0", "1h")]));
    // Inside the tool loop: S, the anchor at the prompt, and the tail for five minutes.
    assert_eq!(
        markers(answer_call),
        marks(&[("system", "1h"), ("message 0", "1h"), ("message 2", "5m")])
    );
    assert!(starts_with(answer_call, edit_call), "the tool loop appends");
    let replayed = &answer_call["messages"][1]["content"];
    assert_eq!(replayed[0]["type"], "thinking");
    assert_eq!(replayed[0]["signature"], "sig-efr-1", "the signed thinking goes back");
    assert_eq!(replayed[1]["id"], "toolu_efr_1");
    let result = &answer_call["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "toolu_efr_1");
    assert_ne!(result["is_error"], json!(true), "{result:#}");

    // The summary request reads the whole conversation from the cache and writes no
    // long entry for its own tail.
    assert!(starts_with(summary_call, answer_call), "the summary request is a cache-safe fork");
    // All four places: S, the anchor, P on the tail of the call before, and the tail,
    // which holds the next prompt and the summary prompt.
    assert_eq!(
        markers(summary_call),
        marks(&[("system", "1h"), ("message 0", "1h"), ("message 2", "5m"), ("message 4", "5m")])
    );
    // After the compaction, one user message holds the head and the prompt, and it
    // opens the turn.
    let head = after_call["messages"][0]["content"].to_string();
    assert!(head.contains("SUMMARY: src/lib.rs calls new()."), "{head}");
    assert!(head.contains("now the farewell"), "{head}");
    assert_eq!(markers(after_call), marks(&[("system", "1h"), ("message 0", "1h")]));

    let usage = usage_of(&first);
    assert_eq!(usage.input_tokens, 7_000 + 155_010, "the three parts of the input");
    assert_eq!(usage.cached_input_tokens, 150_000);
    assert_eq!(usage.cache_write_tokens, 8_000);
    assert_eq!(usage.cache_write_1h_tokens, 3_000);
    assert_eq!(usage.output_tokens, 50 + 25_000);
    assert_eq!(usage.context_tokens, 155_010 + 25_000, "the last call's input and output");

    let dirs = std::sync::Arc::clone(daemon.dirs());
    daemon.stop().await.unwrap();
    let credential = dirs.dirs().data().join(ANTHROPIC_CREDENTIAL).display().to_string();
    assert_eq!(files_holding(dirs.root(), ANTHROPIC_API_KEY.as_bytes()), [credential]);
    assert!(!logged().contains(ANTHROPIC_API_KEY), "a log line holds the key");
    drop(logs);
}

#[tokio::test]
async fn the_first_prompt_waits_for_the_model_list_which_comes_back_from_its_cache() {
    let server = MessagesServer::start().await;
    server.push(MessagesAnswer::text("Hello."));
    let mut daemon = claude_daemon(&server).await;

    let first = run_turn(&daemon, 1, "hello").await;

    assert_eq!(first.last().unwrap().event.kind(), "turn_completed", "{first:#?}");
    let pages = server.models_requests();
    assert!(!pages.is_empty(), "the list came before the call");
    assert_eq!(
        pages[0].authorization.as_deref(),
        Some(format!("Bearer {ANTHROPIC_API_KEY}").as_str())
    );
    assert_eq!(pages[0].limit.as_deref(), Some("1000"));
    assert_eq!(server.received()[0].body["max_tokens"], 128_000, "the model's limit from the list");
    let list = models(&daemon).await;
    let catalog = list.catalog.unwrap();
    assert_eq!(catalog.provider.as_deref(), Some("anthropic-api"));
    assert_eq!(catalog.origin, CatalogOrigin::Backend);
    assert_eq!(list.models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(), [MODEL]);
    assert_eq!(list.models[0].context_window, Some(WINDOW));
    assert!(list.models[0].default);
    let cache = daemon.dirs().dirs().state().join("anthropic_model_catalog.json");
    Wait::new("the cache file").until(|| cache.exists()).await.unwrap();
    assert!(!daemon.dirs().dirs().state().join("model_catalog.json").exists());

    server.refuse_models(MessagesAnswer::error(503, "api_error", "down"));
    daemon.restart().await.unwrap();

    let after = models(&daemon).await;
    assert_eq!(after.catalog.map(|catalog| catalog.origin), Some(CatalogOrigin::Cache));
    assert_eq!(after.models.len(), 1, "the cached list while the API is down");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn without_a_key_a_claude_turn_fails_as_unauthorized_and_calls_no_model() {
    let server = MessagesServer::start().await;
    server.refuse_models(MessagesAnswer::error(401, "authentication_error", "invalid x-api-key"));
    let daemon = TestDaemon::builder().messages(&server).start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let logout = Method::AdminLogout(AdminLogout { provider: "anthropic-api".to_owned() });
    let _: AdminLogoutResult = client.call(logout).await.unwrap();

    let list = models(&daemon).await;
    assert_eq!(list.catalog.map(|catalog| catalog.origin), Some(CatalogOrigin::Missing));
    assert_eq!(list.models, [], "no list and no guess");
    let events = run_turn(&daemon, 1, "hello").await;

    let Event::TurnFailed { error, .. } = &events.last().unwrap().event else {
        panic!("{events:#?}")
    };
    assert_eq!(error.code, ErrorCode::Unauthorized, "{error:?}");
    assert!(server.received().is_empty(), "no model call without a key");
    drop(client);
    daemon.stop().await.unwrap();
}
