use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    CallId, CommandId, ConversationId, ErrorBody, ErrorCode, Event, Origin, PtyId, Scope, TurnId,
    TurnSettings,
};
use efr_provider::{ContentBlock, Message, ProviderId, Role};
use efr_stdx::id::uuid_v7;
use efr_store::Batch;
use efr_test_support::{TestClock, TestRng, TestStore};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    CachedTurn, HistoryLimits, ModelKey, Snapshot, UNFINISHED_CALL, close_open_calls, decode,
    rebuild,
};

fn turn(seed: u64) -> TurnId {
    TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
}

fn call(seed: u64) -> CallId {
    CallId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(seed)))
}

fn conversation() -> ConversationId {
    ConversationId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(1)))
}

fn key(provider: &str, model: &str) -> ModelKey {
    ModelKey::new(ProviderId::new(provider).expect("provider id"), model)
}

fn completed(turn_id: TurnId, index: u32, text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id, index, text: text.to_owned() }
}

fn started(turn_id: TurnId, call_id: CallId, tool: &str) -> Event {
    Event::ToolCallStarted {
        turn_id,
        call_id,
        tool: tool.to_owned(),
        input: json!({}),
        manual_input: false,
        launch: None,
    }
}

fn finished(turn_id: TurnId, call_id: CallId, output: &str) -> Event {
    Event::ToolCallCompleted {
        turn_id,
        call_id,
        output: output.to_owned(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
    }
}

fn tool_call(call_id: CallId, name: &str) -> ContentBlock {
    ContentBlock::ToolCall { call_id: call_id.to_string(), name: name.to_owned(), input: json!({}) }
}

fn tool_result(call_id: CallId, output: &str) -> ContentBlock {
    ContentBlock::ToolResult {
        call_id: call_id.to_string(),
        output: output.to_owned(),
        is_error: false,
    }
}

#[test]
fn a_text_turn_is_the_prompt_and_the_answer() {
    let t = turn(2);
    let events = [completed(t, 0, "Hi.")];
    let refs: Vec<&Event> = events.iter().collect();
    assert_eq!(rebuild("hello", &refs), vec![Message::user("hello"), Message::assistant("Hi.")]);
}

#[test]
fn tool_calls_join_the_assistant_message_until_a_result_comes() {
    let t = turn(2);
    let (c1, c2) = (call(3), call(4));
    let events = [
        completed(t, 0, "Looking."),
        started(t, c1, "read_file"),
        finished(t, c1, "one"),
        started(t, c2, "read_file"),
        finished(t, c2, "two"),
        completed(t, 1, "Done."),
    ];
    let refs: Vec<&Event> = events.iter().collect();

    assert_eq!(
        rebuild("look", &refs),
        vec![
            Message::user("look"),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Text { text: "Looking.".to_owned() },
                    tool_call(c1, "read_file")
                ]
            ),
            Message::new(Role::User, vec![tool_result(c1, "one")]),
            Message::new(Role::Assistant, vec![tool_call(c2, "read_file")]),
            Message::new(Role::User, vec![tool_result(c2, "two")]),
            Message::assistant("Done."),
        ]
    );
}

fn unfinished(call_id: CallId) -> ContentBlock {
    ContentBlock::ToolResult {
        call_id: call_id.to_string(),
        output: UNFINISHED_CALL.to_owned(),
        is_error: true,
    }
}

#[test]
fn a_call_without_a_result_gets_an_error_result_after_its_message() {
    let t = turn(2);
    let (c1, c2) = (call(3), call(4));
    let events = [
        completed(t, 0, "Looking."),
        started(t, c1, "read_file"),
        finished(t, c1, "one"),
        started(t, c2, "shell"),
    ];
    let refs: Vec<&Event> = events.iter().collect();

    assert_eq!(
        rebuild("look", &refs),
        vec![
            Message::user("look"),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Text { text: "Looking.".to_owned() },
                    tool_call(c1, "read_file")
                ]
            ),
            Message::new(Role::User, vec![tool_result(c1, "one")]),
            Message::new(Role::Assistant, vec![tool_call(c2, "shell")]),
            Message::new(Role::User, vec![unfinished(c2)]),
        ]
    );
}

#[test]
fn open_calls_join_the_results_after_their_message_or_start_one() {
    let (c1, c2, c3) = (call(3), call(4), call(5));
    let mut messages = vec![
        Message::user("go"),
        Message::new(Role::Assistant, vec![tool_call(c1, "shell"), tool_call(c2, "shell")]),
        Message::new(Role::User, vec![tool_result(c1, "one")]),
        Message::new(Role::Assistant, vec![tool_call(c3, "read_file")]),
        Message::user("faster"),
    ];

    close_open_calls(&mut messages);

    assert_eq!(
        messages,
        vec![
            Message::user("go"),
            Message::new(Role::Assistant, vec![tool_call(c1, "shell"), tool_call(c2, "shell")]),
            Message::new(Role::User, vec![tool_result(c1, "one"), unfinished(c2)]),
            Message::new(Role::Assistant, vec![tool_call(c3, "read_file")]),
            Message::new(Role::User, vec![unfinished(c3)]),
            Message::user("faster"),
        ]
    );
    let again = messages.clone();
    close_open_calls(&mut messages);
    assert_eq!(messages, again, "a closed transcript stays as it is");
}

#[test]
fn steering_is_a_user_message_where_it_happened() {
    let t = turn(2);
    let events = [
        completed(t, 0, "Working."),
        Event::TurnSteered { turn_id: t, text: "faster".to_owned() },
        completed(t, 1, "Done."),
    ];
    let refs: Vec<&Event> = events.iter().collect();
    assert_eq!(
        rebuild("go", &refs),
        vec![
            Message::user("go"),
            Message::assistant("Working."),
            Message::user("faster"),
            Message::assistant("Done."),
        ]
    );
}

#[test]
fn text_that_streamed_but_never_completed_ends_the_turn() {
    let t = turn(2);
    let events = [
        Event::AssistantMessageUpdated { turn_id: t, index: 0, offset: 0, delta: "Hal".to_owned() },
        Event::AssistantMessageUpdated {
            turn_id: t,
            index: 0,
            offset: 3,
            delta: "f an answ".to_owned(),
        },
    ];
    let refs: Vec<&Event> = events.iter().collect();
    assert_eq!(rebuild("go", &refs), vec![Message::user("go"), Message::assistant("Half an answ")]);
}

/// Records a whole turn: queued, started, `body`, then `end`.
fn whole_turn(t: TurnId, prompt: &str, body: Vec<Event>, end: Event) -> Vec<Event> {
    let command_id = CommandId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(1000)));
    let mut events = vec![
        Event::PromptQueued {
            turn_id: t,
            command_id,
            text: prompt.to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers: Vec::new(),
        },
        Event::TurnStarted {
            turn_id: t,
            cwd: PathBuf::from("/home/u"),
            scope: Scope::Machine,
            settings: None,
        },
    ];
    events.extend(body);
    events.push(end);
    events
}

async fn store_with(batches: Vec<Vec<Event>>) -> TestStore {
    let clock = TestClock::new();
    let store = TestStore::open(clock.shared()).await.expect("store");
    let created = Event::ConversationCreated { origin: Origin::Shell, tty: None };
    let mut all = vec![vec![created]];
    all.extend(batches);
    for events in all {
        let batch = events.into_iter().fold(Batch::new(), |b, e| b.event(conversation(), e));
        store.writer().append(batch).await.expect("append");
    }
    store
}

async fn snapshot(store: &TestStore, limits: HistoryLimits) -> Snapshot {
    Snapshot::read(store.readers(), conversation(), limits).await.expect("snapshot")
}

#[tokio::test]
async fn only_finished_turns_other_than_the_current_one_count() {
    let (a, b, current) = (turn(2), turn(3), turn(4));
    let store = store_with(vec![
        whole_turn(
            a,
            "first",
            vec![completed(a, 0, "One.")],
            Event::TurnCompleted { turn_id: a, usage: None, changes: None },
        ),
        whole_turn(
            b,
            "second",
            vec![],
            Event::TurnFailed { turn_id: b, error: ErrorBody::new(ErrorCode::Unauthorized, "no") },
        ),
        vec![
            Event::PromptQueued {
                turn_id: current,
                command_id: CommandId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(1001))),
                text: "third".to_owned(),
                origin: Origin::Shell,
                context: None,
                settings: TurnSettings::default(),
                steers: Vec::new(),
            },
            Event::TurnStarted {
                turn_id: current,
                cwd: PathBuf::from("/"),
                scope: Scope::Machine,
                settings: None,
            },
        ],
    ])
    .await;

    let history = snapshot(&store, HistoryLimits::default()).await.history(
        current,
        &HashMap::new(),
        &key("replay", "m"),
        HistoryLimits::default(),
    );

    assert_eq!(
        history,
        vec![Message::user("first"), Message::assistant("One."), Message::user("second")]
    );
}

/// An interrupt sent an unread steer again as a prompt of its own: the rebuilt history
/// holds it once, as that prompt, and not as a steer of the turn that never read it.
#[tokio::test]
async fn a_steer_sent_again_as_a_prompt_is_in_the_history_once() {
    let (a, b, current) = (turn(2), turn(3), turn(4));
    // NOTE: seq 1 is conversation_created, 2 and 3 queue and start `a`, 4 is the steer.
    let mut resent = whole_turn(
        b,
        "only the parser",
        vec![completed(b, 0, "Parser fixed.")],
        Event::TurnCompleted { turn_id: b, usage: None, changes: None },
    );
    if let Event::PromptQueued { steers, .. } = &mut resent[0] {
        *steers = vec![efr_protocol::Seq::new(4)];
    }
    let store = store_with(vec![
        whole_turn(
            a,
            "fix the build",
            vec![
                Event::TurnSteered { turn_id: a, text: "only the parser".to_owned() },
                Event::TurnInterruptRequested { turn_id: a, origin: Origin::Shell },
            ],
            Event::TurnInterrupted { turn_id: a },
        ),
        resent,
    ])
    .await;
    let page = snapshot(&store, HistoryLimits::default()).await;
    assert!(
        page.page
            .iter()
            .any(|e| e.seq == efr_protocol::Seq::new(4)
                && matches!(e.event, Event::TurnSteered { .. })),
        "the steer has the seq that the prompt names"
    );

    let history =
        page.history(current, &HashMap::new(), &key("replay", "m"), HistoryLimits::default());

    assert_eq!(
        history,
        vec![
            Message::user("fix the build"),
            Message::user("only the parser"),
            Message::assistant("Parser fixed."),
        ]
    );
}

#[tokio::test]
async fn a_turn_cancelled_during_a_call_still_answers_the_call() {
    let (a, c) = (turn(2), call(3));
    let store = store_with(vec![whole_turn(
        a,
        "first",
        vec![
            started(a, c, "write_file"),
            Event::ApprovalRequested {
                turn_id: a,
                call_id: c,
                summary: "write ~/.zshrc".to_owned(),
                diff_preview: None,
                interactive: false,
                exit: None,
            },
            Event::ApprovalExpired { turn_id: a, call_id: c },
        ],
        Event::TurnCancelled { turn_id: a },
    )])
    .await;

    let history = snapshot(&store, HistoryLimits::default()).await.history(
        turn(9),
        &HashMap::new(),
        &key("replay", "m"),
        HistoryLimits::default(),
    );

    assert_eq!(
        history,
        vec![
            Message::user("first"),
            Message::new(Role::Assistant, vec![tool_call(c, "write_file")]),
            Message::new(Role::User, vec![unfinished(c)]),
        ]
    );
}

#[tokio::test]
async fn a_turn_whose_start_fell_out_of_the_page_is_left_out() {
    let (a, b) = (turn(2), turn(3));
    let store = store_with(vec![
        whole_turn(
            a,
            "first",
            vec![completed(a, 0, "One.")],
            Event::TurnCompleted { turn_id: a, usage: None, changes: None },
        ),
        whole_turn(
            b,
            "second",
            vec![completed(b, 0, "Two.")],
            Event::TurnCompleted { turn_id: b, usage: None, changes: None },
        ),
    ])
    .await;
    let limits = HistoryLimits::new(50, 4, usize::MAX);

    let history = snapshot(&store, limits).await.history(
        turn(9),
        &HashMap::new(),
        &key("replay", "m"),
        limits,
    );

    assert_eq!(history, vec![Message::user("second"), Message::assistant("Two.")]);
}

#[tokio::test]
async fn a_cached_turn_keeps_its_provider_items_only_for_the_same_provider_and_model() {
    let (a, c) = (turn(2), call(3));
    let store = store_with(vec![whole_turn(
        a,
        "first",
        vec![started(a, c, "shell"), finished(a, c, "ok"), completed(a, 0, "One.")],
        Event::TurnCompleted { turn_id: a, usage: None, changes: None },
    )])
    .await;
    let raw = json!([
        { "type": "reasoning", "id": "rs_1", "encrypted_content": "opaque" },
        { "type": "function_call", "id": "fc_1", "call_id": "p1", "name": "shell", "arguments": "{}" },
    ]);
    let calling = Message::new(Role::Assistant, vec![tool_call(c, "shell")]);
    let results = Message::new(Role::User, vec![tool_result(c, "ok")]);
    let exact = vec![
        Message::user("first"),
        calling.clone().with_provider_raw(raw),
        results.clone(),
        Message::assistant("One."),
    ];
    let cache = HashMap::from([(
        a,
        Arc::new(CachedTurn { key: key("replay", "gpt-5.5"), messages: exact.clone() }),
    )]);
    let snapshot = snapshot(&store, HistoryLimits::default()).await;
    let history = |key: ModelKey| snapshot.history(turn(9), &cache, &key, HistoryLimits::default());

    let same = history(key("replay", "gpt-5.5"));
    let other_model = history(key("replay", "gpt-6-sol"));
    let other_provider = history(key("other", "gpt-5.5"));

    assert_eq!(same, exact);
    let canonical = vec![Message::user("first"), calling, results, Message::assistant("One.")];
    assert_eq!(other_model, canonical, "text, calls and results stay; the raw items go");
    assert_eq!(other_provider, canonical);
    assert!(other_model.iter().all(|message| message.provider_raw.is_none()));
}

#[tokio::test]
async fn the_oldest_turns_go_first_when_history_is_too_long() {
    let (a, b, c) = (turn(2), turn(3), turn(4));
    let long = "x".repeat(1000);
    let store = store_with(vec![
        whole_turn(
            a,
            "first",
            vec![completed(a, 0, &long)],
            Event::TurnCompleted { turn_id: a, usage: None, changes: None },
        ),
        whole_turn(
            b,
            "second",
            vec![completed(b, 0, "Two.")],
            Event::TurnCompleted { turn_id: b, usage: None, changes: None },
        ),
        whole_turn(
            c,
            "third",
            vec![completed(c, 0, "Three.")],
            Event::TurnCompleted { turn_id: c, usage: None, changes: None },
        ),
    ])
    .await;
    let snapshot = snapshot(&store, HistoryLimits::default()).await;
    let none = HashMap::new();
    let replay = key("replay", "m");

    let by_turns =
        snapshot.history(turn(9), &none, &replay, HistoryLimits::new(2, 4096, usize::MAX));
    let by_bytes = snapshot.history(turn(9), &none, &replay, HistoryLimits::new(50, 4096, 500));

    let last_two = vec![
        Message::user("second"),
        Message::assistant("Two."),
        Message::user("third"),
        Message::assistant("Three."),
    ];
    assert_eq!(by_turns, last_two);
    assert_eq!(by_bytes, last_two);
}

#[tokio::test]
async fn the_hidden_shell_s_directory_is_the_newest_report() {
    let pty = PtyId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(50)));
    let store = store_with(vec![
        vec![Event::ShellStarted { pty_id: pty, cwd: PathBuf::from("/home/u"), pid: Some(1) }],
        vec![Event::CwdChanged { pty_id: pty, cwd: PathBuf::from("/etc"), host: None }],
    ])
    .await;
    assert_eq!(
        snapshot(&store, HistoryLimits::default()).await.agent_cwd(),
        Some(PathBuf::from("/etc"))
    );
    let empty = store_with(Vec::new()).await;
    assert_eq!(snapshot(&empty, HistoryLimits::default()).await.agent_cwd(), None);
}

#[tokio::test]
async fn a_shell_that_exited_has_no_directory_until_the_next_one_starts() {
    let pty = PtyId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(50)));
    let next = PtyId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(51)));
    let started =
        |pty_id, cwd: &str| Event::ShellStarted { pty_id, cwd: PathBuf::from(cwd), pid: Some(1) };
    let mut batches = vec![
        vec![started(pty, "/home/u")],
        vec![Event::CwdChanged { pty_id: pty, cwd: PathBuf::from("/etc"), host: None }],
        vec![Event::ShellExited { pty_id: pty, exit_code: None }],
    ];
    let exited = store_with(batches.clone()).await;
    assert_eq!(snapshot(&exited, HistoryLimits::default()).await.agent_cwd(), None);

    batches.push(vec![started(next, "/srv")]);
    let restarted = store_with(batches).await;
    assert_eq!(
        snapshot(&restarted, HistoryLimits::default()).await.agent_cwd(),
        Some(PathBuf::from("/srv"))
    );
}

/// Saves `messages` as turn `t`'s, answered by `provider` and `model`.
async fn save(store: &TestStore, t: TurnId, provider: &str, messages: Vec<serde_json::Value>) {
    let saved = efr_store::turn_messages::NewTurnMessages::new(
        conversation(),
        t,
        provider,
        "m",
        messages,
        HistoryLimits::default().max_turns,
    );
    store.writer().append(Batch::new().turn_messages(saved)).await.expect("save");
}

#[tokio::test]
async fn a_saved_turn_takes_the_place_of_the_cache_after_a_restart() {
    let a = turn(2);
    let store = store_with(vec![whole_turn(
        a,
        "first",
        vec![completed(a, 0, "One.")],
        Event::TurnCompleted { turn_id: a, usage: None, changes: None },
    )])
    .await;
    let raw = json!([{ "type": "reasoning", "encrypted_content": "opaque" }]);
    let exact = vec![Message::user("first"), Message::assistant("One.").with_provider_raw(raw)];
    let json = exact.iter().map(|message| serde_json::to_value(message).unwrap()).collect();
    save(&store, a, "replay", json).await;
    let snapshot = snapshot(&store, HistoryLimits::default()).await;

    let same =
        snapshot.history(turn(9), &HashMap::new(), &key("replay", "m"), HistoryLimits::default());
    let other =
        snapshot.history(turn(9), &HashMap::new(), &key("other", "m"), HistoryLimits::default());

    assert_eq!(same, exact, "the same provider and model get their items back");
    assert_eq!(other, vec![Message::user("first"), Message::assistant("One.")]);
}

#[tokio::test]
async fn a_saved_turn_that_cannot_be_read_back_is_rebuilt() {
    let a = turn(2);
    let store = store_with(vec![whole_turn(
        a,
        "first",
        vec![completed(a, 0, "One.")],
        Event::TurnCompleted { turn_id: a, usage: None, changes: None },
    )])
    .await;
    save(&store, a, "replay", vec![json!({ "role": "nobody" })]).await;
    let snapshot = snapshot(&store, HistoryLimits::default()).await;

    let history =
        snapshot.history(turn(9), &HashMap::new(), &key("replay", "m"), HistoryLimits::default());

    assert!(snapshot.saved.is_empty());
    assert_eq!(history, vec![Message::user("first"), Message::assistant("One.")]);
}

/// What a test's log lines say, as plain text.
#[derive(Debug, Clone, Default)]
struct LogText(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogText {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogText {
    type Writer = LogText;

    fn make_writer(&'a self) -> LogText {
        self.clone()
    }
}

#[tokio::test]
async fn saved_messages_that_cannot_be_read_are_logged_without_their_text() {
    let secret = "ghp_the_token_that_a_tool_printed";
    let a = turn(2);
    let store = store_with(vec![whole_turn(
        a,
        "first",
        vec![completed(a, 0, "One.")],
        Event::TurnCompleted { turn_id: a, usage: None, changes: None },
    )])
    .await;
    // A string where the content's list belongs, as after a change of the schema.
    save(&store, a, "openai", vec![json!({ "role": "user", "content": secret })]).await;
    let saved = store
        .readers()
        .with(|conn| efr_store::turn_messages::of_conversation(conn, conversation()))
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let log = LogText::default();
    let subscriber = tracing_subscriber::fmt().with_writer(log.clone()).with_ansi(false).finish();
    let decoded = tracing::subscriber::with_default(subscriber, || decode(saved));
    assert!(decoded.is_none());
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("the saved messages of a turn could not be read"), "{text}");
    assert!(!text.contains(secret), "{text}");
}
