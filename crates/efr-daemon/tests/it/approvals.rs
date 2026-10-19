//! Approvals over a `TestDaemon`: a write outside `$SCRATCH` waits for the user, runs
//! when allowed and never runs when denied; an answer is a write with a receipt. An `a`
//! (always) answer from the user's own terminal saves its rule to `config.toml`, and the
//! next exit that the rule covers asks nothing (phase E).

use std::collections::BTreeMap;
use std::sync::Arc;

use efr_protocol::{
    AlwaysSaved, ApprovalDecision, ApprovalRespond, ApprovalRespondResult, CallId, ConversationId,
    ErrorCode, Event, EventEnvelope, JudgeKind, Method, Mode, NetworkMode, Origin,
    PromptSendResult, SandboxStatus, TurnSettings,
};
use efr_provider::Provider;
use efr_test_daemon::{
    Client, ClientError, FakeLauncher, Replay, TTY, TestDaemon, command_id, events_until,
};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::sandbox::{ScriptedModel, kinds, ready};

fn answer(replay: &Replay, command: u128, decision: ApprovalDecision) -> Method {
    let call_id = replay.bindings().get("<call:1>").unwrap().parse().unwrap();
    Method::ApprovalRespond(ApprovalRespond {
        command_id: command_id(command),
        conversation_id: replay.conversation().unwrap(),
        call_id,
        decision,
    })
}

#[tokio::test]
async fn approval_ask_then_allow() {
    let replay = Replay::run("approval_ask_then_allow").await.unwrap();

    let note = replay.daemon().cwd().join("note.txt");
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "remember the milk\n");
    let events = replay.events().await.unwrap();
    let asked = events.iter().position(|e| e.event.kind() == "approval_requested").unwrap();
    let resolved = events.iter().position(|e| e.event.kind() == "approval_resolved").unwrap();
    let completed = events.iter().position(|e| e.event.kind() == "tool_call_completed").unwrap();
    assert!(asked < resolved && resolved < completed, "the call ran only after the answer");
    // The user sees what the write would put there before answering.
    let Event::ApprovalRequested { diff_preview, .. } = &events[asked].event else {
        panic!("{:?}", events[asked]);
    };
    assert_eq!(
        diff_preview.as_deref(),
        Some(
            format!(
                "--- /dev/null\n+++ b{}\n@@ -0,0 +1,1 @@\n+remember the milk\n",
                note.display()
            )
            .as_str()
        )
    );
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn approval_deny() {
    let replay = Replay::run("approval_deny").await.unwrap();

    assert!(!replay.daemon().cwd().join("note.txt").exists(), "a denied write never runs");
    let events = replay.events().await.unwrap();
    let denied = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { is_error, output, .. } => Some((*is_error, output.clone())),
        _ => None,
    });
    let (is_error, output) = denied.unwrap();
    assert!(is_error);
    assert!(output.contains("denied"), "{output}");
    replay.stop().await.unwrap();
}

#[tokio::test]
async fn a_retried_answer_gets_its_receipt_and_a_late_one_is_not_found() {
    let replay = Replay::run("approval_ask_then_allow").await.unwrap();
    let first = replay.result(2).unwrap().clone().unwrap();

    // The same command again: answered from the receipt, nothing runs twice.
    let retried: Value =
        replay.client().call(answer(&replay, 2, ApprovalDecision::Allow)).await.unwrap();
    assert_eq!(retried, first);
    let parsed: ApprovalRespondResult = serde_json::from_value(retried).unwrap();
    assert!(parsed.seq.get() > 0);

    // A new answer to the call that is no longer pending.
    let late = replay.client().call::<Value>(answer(&replay, 3, ApprovalDecision::Deny)).await;
    let Err(ClientError::Server { body }) = late else { panic!("{late:?}") };
    assert_eq!(body.code, ErrorCode::NotFound);
    let resolved = replay.events().await.unwrap();
    let resolved = resolved.iter().filter(|e| e.event.kind() == "approval_resolved");
    assert_eq!(resolved.count(), 1);
    replay.stop().await.unwrap();
}

/// The reviewer's answer that hands every exit to the user.
const REVIEWER_ASKS: &str = r#"{"verdict":"ask_user","risk":"medium","user_authorization":"low","category":"unsure","rationale":"the user decides"}"#;

/// A `config.toml` with comments, which an `a` answer must keep.
const COMMENTED: &str = "\
# my efr settings
[network]
# efr's proxy, for the hosts of the jobs
mode = \"proxy\"

[sandbox]
# no extra write roots
write_roots = []
";

/// A background job that runs `true` and needs `extra.test`, a host exit of efr's proxy.
fn host_job() -> Value {
    json!({
        "command": "true",
        "background": true,
        "max_minutes": 1,
        "needs": { "hosts": ["extra.test"] },
    })
}

/// A daemon with efr's proxy and the fake launcher, whose model starts `jobs` host jobs
/// in one turn and whose reviewer hands each exit to the user.
async fn proxy_daemon(dirs: &Arc<TestDirs>, jobs: usize) -> (TestDaemon, Arc<ScriptedModel>) {
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), dirs.home().display().to_string()),
    ]);
    let calls = (0..jobs).map(|_| ("shell", host_job())).collect();
    let model = ScriptedModel::reviewed(calls, REVIEWER_ASKS);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(dirs))
        .sandbox_launcher(fake.path())
        .probe_override(SandboxStatus { proxy: true, ..ready() })
        .network_mode(NetworkMode::Proxy)
        .job_env(env)
        .custom_provider(Arc::clone(&model) as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();
    (daemon, model)
}

/// Sends `text` in `auto` from the shell of [`TTY`].
async fn send_auto(daemon: &TestDaemon, client: &Client, text: &str) -> PromptSendResult {
    let Method::PromptSend(mut params) = daemon.prompt(1, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    client.call(Method::PromptSend(params)).await.unwrap()
}

/// `allow_always` for `call_id` of `conversation_id` with the command id `n`.
fn always(n: u128, conversation_id: ConversationId, call_id: CallId) -> Method {
    Method::ApprovalRespond(ApprovalRespond {
        command_id: command_id(n),
        conversation_id,
        call_id,
        decision: ApprovalDecision::AllowAlways,
    })
}

/// The events of `conversation_id` up to its first question or the end of a turn.
async fn next_stop(
    daemon: &TestDaemon,
    client: &Client,
    conversation_id: ConversationId,
) -> Vec<EventEnvelope> {
    let mut follow = daemon.follow(client, conversation_id).await.unwrap();
    events_until(&mut follow, |event| {
        matches!(
            event,
            Event::ApprovalRequested { .. }
                | Event::TurnCompleted { .. }
                | Event::TurnFailed { .. }
                | Event::TurnInterrupted { .. }
        )
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn always_answer_writes_the_rule_and_the_next_exit_asks_nothing() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (daemon, model) = proxy_daemon(&dirs, 2).await;
    let config = dirs.dirs().config().join("config.toml");
    std::fs::write(&config, COMMENTED).unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send_auto(&daemon, &client, "fetch from extra.test in two jobs").await;

    let events = next_stop(&daemon, &client, sent.conversation_id).await;
    let Event::ApprovalRequested { call_id, exit: Some(exit), .. } = &events.last().unwrap().event
    else {
        panic!("{:?}", kinds(&events));
    };
    assert_eq!(exit.always.as_deref(), Some("host extra.test:443"), "the question offers it");
    let _: ApprovalRespondResult =
        client.call(always(2, sent.conversation_id, *call_id)).await.unwrap();

    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let first = *call_id;
    let events = events_until(&mut follow, |event| match event {
        Event::TurnCompleted { .. } => true,
        Event::ApprovalRequested { call_id, .. } => *call_id != first,
        _ => false,
    })
    .await
    .unwrap();
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{:?}", kinds(&events));
    let saved: Vec<AlwaysSaved> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::AlwaysRuleSaved { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        saved,
        vec![AlwaysSaved::Saved { index: 0, rule: "host extra.test:443".to_owned() }]
    );
    // The second job's exit ran by the rule: no question, no review.
    let judges: Vec<JudgeKind> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ExitJudged { judge, .. } => Some(*judge),
            _ => None,
        })
        .collect();
    assert_eq!(judges, vec![JudgeKind::Classifier, JudgeKind::User, JudgeKind::Always]);
    assert_eq!(kinds(&events).iter().filter(|kind| **kind == "approval_requested").count(), 1);
    assert_eq!(model.reviews(), 1, "only the first exit went to the reviewer");
    // The file keeps every comment and line, with the rule after them.
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.starts_with(COMMENTED.trim_end()), "{text}");
    assert!(text.contains("[[sandbox.allow]]\nkind = \"host\"\nhost = \"extra.test\""), "{text}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn always_allow_only_from_own_terminal() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let (daemon, _model) = proxy_daemon(&dirs, 1).await;
    let config = dirs.dirs().config().join("config.toml");
    std::fs::write(&config, COMMENTED).unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send_auto(&daemon, &client, "fetch from extra.test in a job").await;
    let events = next_stop(&daemon, &client, sent.conversation_id).await;
    let Event::ApprovalRequested { call_id, .. } = &events.last().unwrap().event else {
        panic!("{:?}", kinds(&events));
    };

    // A phone gets `forbidden`, the question stays open, and the file stays as it was.
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
    let refused = phone.call::<Value>(always(2, sent.conversation_id, *call_id)).await;
    let Err(ClientError::Server { body }) = refused else { panic!("{refused:?}") };
    assert_eq!(body.code, ErrorCode::Forbidden);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), COMMENTED, "byte for byte");
    let logged = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert!(!kinds(&logged).contains(&"approval_resolved"), "{:?}", kinds(&logged));

    // The user's own terminal writes the rule, with a new command id.
    let _: ApprovalRespondResult =
        client.call(always(3, sent.conversation_id, *call_id)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    events_until(&mut follow, |event| matches!(event, Event::TurnCompleted { .. })).await.unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("extra.test"), "{text}");
    daemon.stop().await.unwrap();
}
