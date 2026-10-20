//! Full mode end to end over a `TestDaemon` (efr's full mode, plan 3.11): the gate key,
//! the config default, the caps of a phone and of a wake turn, the refusal of a root
//! efrd, the questions that full mode still asks (a guarded delete, a process that
//! outlives the turn, a settings change), who may answer them, the record of what full
//! mode let through, and `efr undo` of a full turn.
//!
//! Full mode runs commands as the user with no sandbox, so these tests keep to three
//! rules. A guard test uses the fake PTY holder, which never runs a line, and its delete
//! is a dead branch all the same (`false && rm -rf ~`). A test that drives a real zsh
//! (`shell_`, `EFR_TEST_ZSH=1`) writes only below its stand-in home. No test runs a real
//! `sudo`, `systemd-run`, `at` or `crontab`: a `sudo` line finds a fake one first on the
//! hidden shell's `PATH`, and a persistence line is a dead branch.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    AdminConfigReload, AdminConfigReloadResult, AdminProjectAdd, AdminProjectAddResult,
    AdminStatus, AdminStatusResult, ApprovalDecision, ApprovalRespond, ApprovalRespondResult,
    CallId, ConversationId, EffectiveSettings, ErrorBody, ErrorCode, Event, EventEnvelope,
    FullRefusal, JudgeKind, Launch, Method, Mode, ModeFallback, Origin, PromptSendResult,
    SnapshotPhase, TurnId, TurnSettings, Verdict,
};
use efr_test_daemon::{
    Client, ClientError, FakeLauncher, TTY, TestDaemon, TestDaemonBuilder, command_id, events_until,
};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::home::home_copies;
use crate::jobs_wake::{Answer, Recorder};
use crate::sandbox::{kinds, ready};
use crate::support::{logged, logs, zsh_enabled};
use crate::undo::{read, undo};

/// `config.toml` with the gate key, so a reload keeps full mode allowed.
const ALLOW_FULL: &str = "[permissions]\nallow_full = true\n";

/// `config.toml` that makes full mode the default of every turn that names no mode.
const DEFAULT_FULL: &str = "[permissions]\nmode = \"full\"\nallow_full = true\n";

/// The start of the summary of a question that full mode still asks.
const FULL_ASKS: &str = "full mode still asks for this: ";

/// The guarded delete of the tests: a dead branch, so it removes nothing even if a line
/// ran. The guard reads it all the same.
const GUARDED_DELETE: &str = "false && rm -rf ~";

/// Writes `text` as the `config.toml` of `dirs`.
pub(crate) fn write_config(dirs: &TestDirs, text: &str) -> PathBuf {
    let path = dirs.dirs().config().join("config.toml");
    std::fs::create_dir_all(dirs.dirs().config()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// A daemon on `dirs` that allows full mode, in its running settings and in its file.
pub(crate) fn full_builder(dirs: &Arc<TestDirs>) -> TestDaemonBuilder {
    write_config(dirs, ALLOW_FULL);
    TestDaemon::builder()
        .dirs(Arc::clone(dirs))
        .config(|config| config.permissions.allow_full = true)
}

/// A `shell` call of `command`.
pub(crate) fn shell(command: &str) -> Answer {
    Some(("shell", json!({ "command": command })))
}

/// A `write_file` call of `path` with `content`.
fn write_file(path: &Path, content: &str) -> Answer {
    Some(("write_file", json!({ "path": path, "content": content })))
}

/// Sends a `prompt.send` of `text` with the command id `n` from the shell of [`TTY`],
/// in `mode` when it is `Some`; the daemon's refusal as its error body.
pub(crate) async fn send(
    daemon: &TestDaemon,
    client: &Client,
    n: u128,
    text: &str,
    mode: Option<Mode>,
) -> Result<PromptSendResult, ErrorBody> {
    let Method::PromptSend(mut params) = daemon.prompt(n, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode, ..TurnSettings::default() };
    match client.call::<PromptSendResult>(Method::PromptSend(params)).await {
        Ok(sent) => Ok(sent),
        Err(ClientError::Server { body }) => Err(body),
        Err(other) => panic!("prompt.send failed: {other:?}"),
    }
}

/// True when `event` ends the turn `turn_id`.
fn ends(event: &Event, turn_id: TurnId) -> bool {
    match event {
        Event::TurnCompleted { turn_id: t, .. }
        | Event::TurnFailed { turn_id: t, .. }
        | Event::TurnInterrupted { turn_id: t, .. } => *t == turn_id,
        _ => false,
    }
}

/// The events of `conversation` from the start of its log up to the end of the turn
/// `turn_id`, or up to a question of that turn whose call is not in `past`.
pub(crate) async fn next_stop(
    daemon: &TestDaemon,
    client: &Client,
    conversation: ConversationId,
    turn_id: TurnId,
    past: &[CallId],
) -> Vec<EventEnvelope> {
    let mut follow = daemon.follow(client, conversation).await.unwrap();
    events_until(&mut follow, |event| match event {
        Event::ApprovalRequested { turn_id: t, call_id, .. } => {
            *t == turn_id && !past.contains(call_id)
        }
        _ => ends(event, turn_id),
    })
    .await
    .unwrap()
}

/// The turn's settings from its `turn_started`.
pub(crate) fn started(events: &[EventEnvelope], turn_id: TurnId) -> EffectiveSettings {
    events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::TurnStarted { turn_id: t, settings, .. } if *t == turn_id => settings.clone(),
            _ => None,
        })
        .unwrap()
}

/// The judge and the verdict of each `exit_judged` of `events`.
pub(crate) fn judged(events: &[EventEnvelope]) -> Vec<(JudgeKind, Verdict)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ExitJudged { judge, verdict, .. } => Some((*judge, *verdict)),
            _ => None,
        })
        .collect()
}

/// The output of each `tool_call_completed` of `events`.
fn outputs(events: &[EventEnvelope]) -> Vec<String> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, .. } => Some(output.clone()),
            _ => None,
        })
        .collect()
}

/// The open question at the end of `events`: its call, its summary and its time limit.
pub(crate) fn question(events: &[EventEnvelope]) -> (CallId, String, Option<u32>) {
    match &events.last().unwrap().event {
        Event::ApprovalRequested { call_id, summary, expires_in_secs, .. } => {
            (*call_id, summary.clone(), *expires_in_secs)
        }
        other => panic!("no open question but {other:?} after {:?}", kinds(events)),
    }
}

/// `approval.respond` of `call_id` with `decision`, command `n`.
pub(crate) fn respond(
    n: u128,
    conversation: ConversationId,
    call_id: CallId,
    decision: ApprovalDecision,
) -> Method {
    Method::ApprovalRespond(ApprovalRespond {
        command_id: command_id(n),
        conversation_id: conversation,
        call_id,
        decision,
    })
}

/// `admin.status`.
pub(crate) async fn status(client: &Client) -> AdminStatusResult {
    client.call(Method::AdminStatus(AdminStatus {})).await.unwrap()
}

/// `admin.config_reload`, as `efr config reload` sends it.
async fn reload(client: &Client) -> AdminConfigReloadResult {
    client.call(Method::AdminConfigReload(AdminConfigReload {})).await.unwrap()
}

/// The error data of a full mode refusal: the mode and the modes that a turn may ask
/// for in its stead.
fn refusal_data() -> Option<Value> {
    Some(json!({ "setting": "mode", "value": "full", "choices": ["manual", "cautious", "auto"] }))
}

#[tokio::test]
async fn mode_full_refused_without_allow_full() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("notes.txt");
    let model = Recorder::new(vec![write_file(&notes, "x\n")]);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .custom_provider(Arc::clone(&model) as _)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let full_mode = status(&client).await.full_mode.unwrap();
    assert_eq!((full_mode.allowed, full_mode.refused), (false, None));

    let body = send(&daemon, &client, 1, "write my notes", Some(Mode::Full)).await.unwrap_err();

    assert_eq!(body.code, ErrorCode::Invalid, "{body:?}");
    assert!(body.message.contains("set permissions.allow_full = true"), "{}", body.message);
    assert_eq!(body.data, refusal_data());
    assert!(model.firsts().is_empty(), "no turn reached the model");
    assert!(!notes.exists());
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn mode_full_runs_with_allow_full() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("notes.txt");
    std::fs::write(&notes, "old\n").unwrap();
    let model = Recorder::new(vec![write_file(&notes, "new\n")]);
    let daemon =
        full_builder(&dirs).custom_provider(Arc::clone(&model) as _).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let full_mode = status(&client).await.full_mode.unwrap();
    assert_eq!((full_mode.allowed, full_mode.refused), (true, None));

    let sent = send(&daemon, &client, 1, "put new in my notes", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;

    let settings = started(&events, sent.turn_id);
    assert_eq!((settings.mode, settings.fallback), (Mode::Full, None));
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert_eq!(read(&notes), "new\n");
    // The write outside the project ran with no question, as an exit that full mode let
    // through, and nobody reviewed it.
    assert_eq!(judged(&events), [(JudgeKind::Full, Verdict::Allow)]);
    assert_eq!(model.reviews(), 0);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn config_mode_full_is_a_load_error() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let daemon = TestDaemon::builder().dirs(Arc::clone(&dirs)).start().await.unwrap();
    let client = daemon.client().await.unwrap();
    write_config(&dirs, "[permissions]\nmode = \"full\"\n");

    let result = reload(&client).await;

    assert!(!result.applied);
    let error = result.error.unwrap();
    assert_eq!(error.key.as_deref(), Some("permissions.mode"), "{error:?}");
    assert!(error.message.contains("permissions.allow_full = true"), "{}", error.message);
    // The old settings stay: full mode is still off.
    assert!(!status(&client).await.full_mode.unwrap().allowed);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn config_default_full_runs_full_turns() {
    let _logs = logs();
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("notes.txt");
    let other = dirs.home().join("other.txt");
    let model = Recorder::new(vec![write_file(&notes, "full\n"), None, write_file(&other, "x\n")]);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .custom_provider(Arc::clone(&model) as _)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    write_config(&dirs, DEFAULT_FULL);
    let result = reload(&client).await;
    assert!(result.applied, "{result:?}");
    assert!(
        logged().contains("permissions.mode is full: every turn that names no mode runs"),
        "efrd warns when the config makes full mode the default"
    );

    // A prompt that names no mode runs in full mode: no question.
    let sent = send(&daemon, &client, 1, "write my notes", None).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    let settings = started(&events, sent.turn_id);
    assert_eq!((settings.mode, settings.overridden.mode), (Mode::Full, false));
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert_eq!(read(&notes), "full\n");
    assert_eq!(judged(&events), [(JudgeKind::Full, Verdict::Allow)]);

    // A prompt that names cautious moves down: the write outside the project asks.
    let sent = send(&daemon, &client, 2, "write the other", Some(Mode::Cautious)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    assert_eq!(started(&events, sent.turn_id).mode, Mode::Cautious);
    let (call_id, _, expires) = question(&events);
    assert_eq!(expires, None);
    let _: ApprovalRespondResult = client
        .call(respond(10, sent.conversation_id, call_id, ApprovalDecision::Deny))
        .await
        .unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert!(!other.exists(), "a denied write never runs");
    daemon.stop().await.unwrap();
}

/// A daemon builder over `model` with the fake launcher of `efr-test-daemon`, a ready
/// sandbox and a fixed job environment, so a turn can start a background job.
fn job_builder(
    builder: TestDaemonBuilder,
    dirs: &TestDirs,
    model: Arc<Recorder>,
) -> TestDaemonBuilder {
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), dirs.home().display().to_string()),
    ]);
    builder
        .sandbox_launcher(fake.path())
        .probe_override(ready())
        .job_env(env)
        .custom_provider(model)
}

/// A FIFO below the test's root, which a job reads and the test writes.
fn fifo(dirs: &TestDirs, name: &str) -> PathBuf {
    let path = dirs.root().join(name);
    nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    path
}

/// Writes one line to `fifo`, which a job reads; it waits for the reader.
async fn release(fifo: &Path) {
    let fifo = fifo.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
}

/// A background job that waits for `fifo`, then ends.
fn blocked_job(fifo: &Path) -> Answer {
    let command = format!("read answer < {}; echo ended", fifo.display());
    Some(("shell", json!({ "command": command, "background": true })))
}

/// The turn of `conversation` that starts after the turn `after`, and the events of the
/// log up to its end.
async fn wake_turn(
    daemon: &TestDaemon,
    client: &Client,
    conversation: ConversationId,
    after: TurnId,
) -> (TurnId, Vec<EventEnvelope>) {
    let mut follow = daemon.follow(client, conversation).await.unwrap();
    let mut wake = None;
    let events = events_until(&mut follow, |event| match event {
        Event::TurnStarted { turn_id, .. } if *turn_id != after && wake.is_none() => {
            wake = Some(*turn_id);
            false
        }
        _ => wake.is_some_and(|wake| ends(event, wake)),
    })
    .await
    .unwrap();
    (wake.unwrap(), events)
}

/// A prompt from a phone to `conversation`, in `mode` when it is `Some`.
async fn phone_prompt(
    daemon: &TestDaemon,
    phone: &Client,
    n: u128,
    conversation: ConversationId,
    mode: Option<Mode>,
) -> PromptSendResult {
    let Method::PromptSend(mut params) = daemon.prompt(n, "from the phone", TTY) else {
        unreachable!()
    };
    params.conversation_id = Some(conversation);
    params.context = None;
    params.settings = TurnSettings { mode, ..TurnSettings::default() };
    phone.call(Method::PromptSend(params)).await.unwrap()
}

#[tokio::test]
async fn phone_prompt_in_full_runs_as_cautious() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("notes.txt");
    let model = Recorder::new(vec![None, write_file(&notes, "from the phone\n")]);
    let daemon =
        full_builder(&dirs).custom_provider(Arc::clone(&model) as _).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let first = send(&daemon, &client, 1, "hello", Some(Mode::Full)).await.unwrap();
    next_stop(&daemon, &client, first.conversation_id, first.turn_id, &[]).await;
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();

    let sent = phone_prompt(&daemon, &phone, 2, first.conversation_id, Some(Mode::Full)).await;
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;

    // The phone's turn runs as cautious with no error, so the write asks.
    assert_eq!(started(&events, sent.turn_id).mode, Mode::Cautious);
    let (call_id, summary, expires) = question(&events);
    assert!(!summary.starts_with(FULL_ASKS), "{summary}");
    assert_eq!(expires, None);
    let _: ApprovalRespondResult = client
        .call(respond(10, sent.conversation_id, call_id, ApprovalDecision::Deny))
        .await
        .unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"));
    assert!(!notes.exists());
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn full_mode_on_an_unavailable_sandbox() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("notes.txt");
    let model = Recorder::new(vec![write_file(&notes, "full\n"), None]);
    let unavailable = efr_protocol::SandboxStatus {
        available: false,
        reason: Some("Landlock is off in this kernel".to_owned()),
        ..ready()
    };
    let daemon = full_builder(&dirs)
        .probe_override(unavailable)
        .custom_provider(Arc::clone(&model) as _)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    assert!(!status(&client).await.sandbox.unwrap().available);

    let sent = send(&daemon, &client, 1, "write my notes", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;

    // Full mode needs no sandbox: no fallback, and the exit is still predicted and
    // recorded with the facts of the call.
    let settings = started(&events, sent.turn_id);
    assert_eq!((settings.mode, settings.fallback), (Mode::Full, None));
    assert_eq!(judged(&events), [(JudgeKind::Full, Verdict::Allow)]);
    assert!(kinds(&events).contains(&"exit_requested"), "{:?}", kinds(&events));
    assert_eq!(read(&notes), "full\n");
    // An auto turn on the same daemon falls back to cautious, with the probe's reason.
    let sent = send(&daemon, &client, 2, "hello", Some(Mode::Auto)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    let settings = started(&events, sent.turn_id);
    assert_eq!(settings.mode, Mode::Cautious);
    assert_eq!(settings.fallback.map(|fallback| fallback.asked), Some(Mode::Auto));
    daemon.stop().await.unwrap();
}

/// A daemon that allows full mode and whose model runs the guarded delete, then says
/// `done`; the prompt's turn up to its question.
async fn guard_question() -> (TestDaemon, Client, PromptSendResult, Vec<EventEnvelope>) {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let model = Recorder::new(vec![shell(GUARDED_DELETE)]);
    let daemon = full_builder(&dirs).custom_provider(model).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send(&daemon, &client, 1, "clean my home", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    (daemon, client, sent, events)
}

#[tokio::test]
async fn full_guard_question_expires() {
    let (daemon, client, sent, events) = guard_question().await;
    let (call_id, summary, expires) = question(&events);
    assert!(summary.starts_with(FULL_ASKS), "{summary}");
    assert!(summary.contains("delete of ~ (your home directory)"), "{summary}");
    assert_eq!(expires, Some(120));

    daemon.clock().advance(Duration::from_secs(120));
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;

    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert!(kinds(&events).contains(&"approval_expired"), "{:?}", kinds(&events));
    let [output] = outputs(&events).try_into().unwrap();
    assert!(output.starts_with("efr did not run this delete of ~"), "{output}");
    assert!(output.contains("the user did not answer in 120 seconds"), "{output}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn full_guard_question_from_phone_refused() {
    let (daemon, client, sent, events) = guard_question().await;
    let (call_id, _, _) = question(&events);
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();

    let allow = respond(10, sent.conversation_id, call_id, ApprovalDecision::Allow);
    let answer = phone.call::<ApprovalRespondResult>(allow).await;

    let Err(ClientError::Server { body }) = answer else { panic!("{answer:?}") };
    assert_eq!(body.code, ErrorCode::Forbidden, "{body:?}");
    let logged = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert!(!kinds(&logged).contains(&"approval_resolved"), "the question stays open");
    // The user's own terminal answers it.
    let _: ApprovalRespondResult = client
        .call(respond(11, sent.conversation_id, call_id, ApprovalDecision::Deny))
        .await
        .unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    let [output] = outputs(&events).try_into().unwrap();
    assert!(output.contains("it did not run"), "{output}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn full_wake_turn_runs_as_auto() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let go = fifo(&dirs, "go");
    let model = Recorder::new(vec![blocked_job(&go), None, None]);
    let daemon = job_builder(full_builder(&dirs), &dirs, model).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send(&daemon, &client, 1, "run it and tell me", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    // A full job starts with no question.
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert!(kinds(&events).contains(&"job_started"), "{:?}", kinds(&events));

    release(&go).await;
    let (wake, events) = wake_turn(&daemon, &client, sent.conversation_id, sent.turn_id).await;

    let settings = started(&events, wake);
    assert_eq!(settings.mode, Mode::Auto);
    assert_eq!(
        settings.fallback,
        Some(ModeFallback { asked: Mode::Full, reason: ModeFallback::WAKE_REASON.to_owned() })
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn config_default_full_still_caps_wake_and_phone() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    write_config(&dirs, DEFAULT_FULL);
    let go = fifo(&dirs, "go");
    let model = Recorder::new(vec![blocked_job(&go), None, None, None]);
    let builder = TestDaemon::builder().dirs(Arc::clone(&dirs)).config(|config| {
        config.permissions.allow_full = true;
        config.permissions.mode = Mode::Full;
    });
    let daemon = job_builder(builder, &dirs, model).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    // The default turn is full, and its job starts with no question.
    let sent = send(&daemon, &client, 1, "run it and tell me", None).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    assert_eq!(started(&events, sent.turn_id).mode, Mode::Full);
    assert!(kinds(&events).contains(&"job_started"), "{:?}", kinds(&events));

    // The turn that the job's notice starts runs with at most auto.
    release(&go).await;
    let (wake, events) = wake_turn(&daemon, &client, sent.conversation_id, sent.turn_id).await;
    let settings = started(&events, wake);
    assert_eq!(settings.mode, Mode::Auto);
    assert_eq!(settings.fallback.map(|fallback| fallback.asked), Some(Mode::Full));

    // A phone's turn that names no mode runs with at most cautious.
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
    let from_phone = phone_prompt(&daemon, &phone, 2, sent.conversation_id, None).await;
    let events = next_stop(&daemon, &client, sent.conversation_id, from_phone.turn_id, &[]).await;
    assert_eq!(started(&events, from_phone.turn_id).mode, Mode::Cautious);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn efr_undo_reverts_full_turn_write_outside_project() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("notes.txt");
    let zshrc = dirs.home().join(".zshrc");
    std::fs::write(&notes, "old notes\n").unwrap();
    std::fs::write(&zshrc, "# old rc\n").unwrap();
    let model =
        Recorder::new(vec![write_file(&notes, "new notes\n"), write_file(&zshrc, "# new rc\n")]);
    let daemon = full_builder(&dirs).custom_provider(model).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let text = "change my notes and my zshrc";
    let sent = send(&daemon, &client, 1, text, Some(Mode::Full)).await.unwrap();

    // The notes run with no question; a write of a startup file is a process that
    // outlives the turn, so full mode asks for it, and the user says yes.
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    let (call_id, summary, expires) = question(&events);
    assert!(summary.starts_with(FULL_ASKS), "{summary}");
    assert_eq!(expires, None);
    assert_eq!(read(&notes), "new notes\n");
    let _: ApprovalRespondResult = client
        .call(respond(10, sent.conversation_id, call_id, ApprovalDecision::Allow))
        .await
        .unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert_eq!(read(&zshrc), "# new rc\n");
    assert_eq!(
        home_copies(&dirs).await,
        [(zshrc.clone(), "# old rc\n".to_owned()), (notes.clone(), "old notes\n".to_owned())]
    );

    let undone = undo(&daemon, 20, |_| {}).await.unwrap();

    assert!(undone.applied, "{undone:?}");
    assert_eq!((read(&notes), read(&zshrc)), ("old notes\n".to_owned(), "# old rc\n".to_owned()));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn full_turn_with_root_uid_refused() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let model = Recorder::new(vec![shell("true")]);
    let daemon = full_builder(&dirs)
        .as_root(true)
        .custom_provider(Arc::clone(&model) as _)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let full_mode = status(&client).await.full_mode.unwrap();
    assert_eq!((full_mode.allowed, full_mode.refused), (true, Some(FullRefusal::Root)));

    let body = send(&daemon, &client, 1, "go", Some(Mode::Full)).await.unwrap_err();

    assert_eq!(body.code, ErrorCode::Invalid, "{body:?}");
    assert_eq!(body.message, "full mode cannot run: efrd runs as root");
    assert_eq!(body.data, refusal_data());
    assert!(model.firsts().is_empty(), "no turn reached the model");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn full_persistence_line_asks() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let model = Recorder::new(vec![shell("false && systemd-run --user true")]);
    let daemon =
        full_builder(&dirs).custom_provider(Arc::clone(&model) as _).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send(&daemon, &client, 1, "start it for later", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;

    let (call_id, summary, expires) = question(&events);
    assert!(summary.starts_with(FULL_ASKS), "{summary}");
    assert_eq!(expires, None, "only a guarded delete has a time limit");
    assert_eq!(model.reviews(), 0, "nobody reviews a question of full mode");
    // Not local only: the phone may answer it, as in auto.
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();
    let _: ApprovalRespondResult = phone
        .call(respond(10, sent.conversation_id, call_id, ApprovalDecision::Deny))
        .await
        .unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    let [output] = outputs(&events).try_into().unwrap();
    assert!(output.contains("it did not run"), "{output}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn full_settings_tool_question_still_asks() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let change = json!({ "operation": "set", "key": "model.name", "value": "gpt-6-luna" });
    let model = Recorder::new(vec![Some(("settings", change))]);
    let daemon = full_builder(&dirs).custom_provider(model).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send(&daemon, &client, 1, "use another model", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;

    let (call_id, _, expires) = question(&events);
    assert_eq!(expires, None);
    let _: ApprovalRespondResult = client
        .call(respond(10, sent.conversation_id, call_id, ApprovalDecision::Deny))
        .await
        .unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    let config = std::fs::read_to_string(dirs.dirs().config().join("config.toml")).unwrap();
    assert_eq!(config, ALLOW_FULL, "a denied change never reaches the file");
    daemon.stop().await.unwrap();
}

/// A fake `sudo` that only notes its arguments in `~/sudo-ran`, for the hidden shell's
/// `PATH`. A full call runs `sudo -k` before and after its line, so the file also shows
/// that efr cleared sudo's cache.
const FAKE_SUDO: &str = "#!/bin/sh\necho \"$*\" >> \"$HOME/sudo-ran\"\n";

/// A daemon builder that allows full mode and runs real zsh shells (`EFR_TEST_ZSH=1`)
/// with the stand-in home, and a dir of the test first on `PATH` that holds a fake
/// `sudo`.
fn zsh_full(dirs: &Arc<TestDirs>, model: Arc<Recorder>) -> TestDaemonBuilder {
    let bin = dirs.create_dir("home/bin").unwrap();
    let sudo = bin.join("sudo");
    std::fs::write(&sudo, FAKE_SUDO).unwrap();
    std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), format!("{}:/usr/bin:/bin", bin.display())),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
    ]);
    full_builder(dirs).local_pty().shell_env(env).custom_provider(model)
}

/// Registers the daemon's working directory as a project, so its calls take snapshots.
async fn register_cwd(daemon: &TestDaemon, client: &Client) {
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: daemon.cwd().to_path_buf(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
}

/// The launch of each `tool_call_started` of `events`.
fn launches(events: &[EventEnvelope]) -> Vec<Option<Launch>> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallStarted { launch, .. } => Some(launch.clone()),
            _ => None,
        })
        .collect()
}

/// True when python3 is on the `PATH` of the tests; otherwise `test` says on stderr
/// that it skips.
#[expect(clippy::print_stderr, reason = "a skipped test says why")]
fn python3(test: &str) -> bool {
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|dir| dir.join("python3").is_file());
    if !found {
        eprintln!("skipping {test}: python3 is not installed");
    }
    found
}

#[tokio::test]
async fn full_shell_call_takes_snapshot_first() {
    if !zsh_enabled("full_shell_call_takes_snapshot_first") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let model = Recorder::new(vec![shell("echo new > a.txt")]);
    let daemon = zsh_full(&dirs, Arc::clone(&model)).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    register_cwd(&daemon, &client).await;
    let file = daemon.cwd().join("a.txt");
    std::fs::write(&file, "old\n").unwrap();

    let sent = send(&daemon, &client, 1, "put new in a.txt", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;

    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert_eq!(launches(&events), [None], "a direct call names no launch: no sandbox");
    assert_eq!(read(&file), "new\n");
    // The turn took the project's snapshot before the call wrote the file: the turn
    // reports it, and undo gives back the text of before.
    let pre: Vec<u32> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::SnapshotTaken { phase: SnapshotPhase::Pre, root, files, .. }
                if root != "$SCRATCH" =>
            {
                Some(*files)
            }
            _ => None,
        })
        .collect();
    assert_eq!(pre, [1], "{:?}", kinds(&events));
    let undone = undo(&daemon, 20, |_| {}).await.unwrap();
    assert!(undone.applied, "{undone:?}");
    assert_eq!(read(&file), "old\n");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn full_shell_write_outside_project_is_copied_and_undone() {
    if !zsh_enabled("full_shell_write_outside_project_is_copied_and_undone") {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("notes.txt");
    std::fs::write(&notes, "old\n").unwrap();
    let model = Recorder::new(vec![shell("echo x >> ~/notes.txt")]);
    let daemon = zsh_full(&dirs, Arc::clone(&model)).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();

    let sent = send(&daemon, &client, 1, "add x to my notes", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;

    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    assert_eq!(launches(&events), [None]);
    assert_eq!(judged(&events), [(JudgeKind::Full, Verdict::Allow)]);
    assert_eq!(model.reviews(), 0);
    assert_eq!(read(&notes), "old\nx\n");
    assert_eq!(home_copies(&dirs).await, [(notes.clone(), "old\n".to_owned())]);
    // The hidden shell cleared sudo's cache before and after the call.
    let ran = read(&dirs.home().join("sudo-ran"));
    assert!(ran.lines().count() >= 2 && ran.lines().all(|line| line == "-k"), "{ran}");
    let undone = undo(&daemon, 20, |_| {}).await.unwrap();
    assert!(undone.applied, "{undone:?}");
    assert_eq!(read(&notes), "old\n");
    daemon.stop().await.unwrap();
}

/// A peer that a command of the model leaves behind: it waits for the file `go`, then
/// sends the frames in `frames/` to the daemon's socket and writes every byte of the
/// answers to `out`.
const WAITING_PEER: &str = r#"
import os, socket, sys, time
sock, frames, go, out = sys.argv[1:5]
end = time.time() + 30
while not os.path.exists(go) and time.time() < end:
    time.sleep(0.01)
s = socket.socket(socket.AF_UNIX)
s.connect(sock)
for name in sorted(os.listdir(frames)):
    s.sendall(open(os.path.join(frames, name), "rb").read())
s.settimeout(0.5)
data = b""
wanted = len(os.listdir(frames))
end = time.time() + 10
def answered(data):
    count, at = 0, 0
    while at + 4 <= len(data):
        size = int.from_bytes(data[at:at + 4], "big")
        payload = data[at + 4:at + 4 + size]
        if len(payload) < size:
            break
        count += b'"end":true' in payload or b'"error":' in payload
        at += 4 + size
    return count
while time.time() < end and answered(data) < wanted:
    try:
        chunk = s.recv(65536)
    except socket.timeout:
        continue
    if not chunk:
        break
    data += chunk
open(out + ".tmp", "wb").write(data)
os.rename(out + ".tmp", out)
"#;

// NOTE: two workers, so the daemon in this process answers the peer while the test
// waits for the peer's file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_side_peer_cannot_answer_a_full_question() {
    use efr_protocol::framing::{Decoder, encode};
    use efr_protocol::{ClientFrame, Hello, PROTOCOL_VERSION, RequestId, ServerFrame};

    let test = "model_side_peer_cannot_answer_a_full_question";
    if !zsh_enabled(test) || !python3(test) {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let work = dirs.create_dir("home/peer").unwrap();
    let frames = dirs.create_dir("home/peer/frames").unwrap();
    std::fs::write(work.join("peer.py"), WAITING_PEER).unwrap();
    let (go, out) = (work.join("go"), work.join("answers"));
    // NOTE: the subshell ends at once, so python3 is an orphan that the hidden zsh, a
    // child subreaper, adopts: a process of the model's command that outlives its call.
    let leave = format!(
        "(python3 {} {} {} {} {} >/dev/null 2>&1 &)",
        work.join("peer.py").display(),
        dirs.dirs().socket_path().display(),
        frames.display(),
        go.display(),
        out.display()
    );
    let model = Recorder::new(vec![shell(&leave), shell("sudo true")]);
    let daemon = zsh_full(&dirs, model).start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent =
        send(&daemon, &client, 1, "leave a peer, then sudo", Some(Mode::Full)).await.unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    // The peer's line ran with no question; the sudo line asks at the user's machine.
    let (call_id, summary, _) = question(&events);
    assert!(summary.starts_with(FULL_ASKS), "{summary}");
    assert_eq!(outputs(&events).len(), 1, "{:?}", kinds(&events));

    // The peer answers the question as a shell would.
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        origin: Origin::Shell,
        client: Some("peer".to_owned()),
        capabilities: efr_protocol::Capabilities::default(),
        tty: None,
        pid: None,
        device_id: None,
    };
    let answer = respond(9, sent.conversation_id, call_id, ApprovalDecision::Allow);
    for (n, method) in [Method::Hello(hello), answer].into_iter().enumerate() {
        let frame = ClientFrame::Request { id: RequestId::new(n as u64 + 1), method };
        std::fs::write(frames.join(format!("{n}")), encode(&frame).unwrap()).unwrap();
    }
    std::fs::write(&go, "").unwrap();
    let answers = Wait::new("the peer's file of the daemon's answers")
        .limit(Duration::from_secs(30))
        .until_some(|| std::fs::read(&out).ok())
        .await
        .unwrap();

    let mut refused = None;
    for payload in Decoder::new().push(&answers).unwrap() {
        if let ServerFrame::Error(frame) = ServerFrame::from_json(&payload).unwrap()
            && frame.id.map(RequestId::get) == Some(2)
        {
            refused = Some((frame.error.code, frame.error.message));
        }
    }
    let (code, message) = refused.unwrap();
    assert_eq!(code, ErrorCode::Forbidden);
    assert!(
        message.contains("not open to a process that the model's commands started"),
        "{message}"
    );
    let logged = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert!(!kinds(&logged).contains(&"approval_resolved"), "the question stays open");
    let _: ApprovalRespondResult = client
        .call(respond(10, sent.conversation_id, call_id, ApprovalDecision::Deny))
        .await
        .unwrap();
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;
    assert_eq!(kinds(&events).last(), Some(&"turn_completed"), "{:?}", kinds(&events));
    // Only the forget key ran the fake sudo, before and after the full call; the
    // denied line never ran.
    let ran = read(&dirs.home().join("sudo-ran"));
    assert!(ran.lines().count() >= 2 && ran.lines().all(|line| line == "-k"), "{ran}");
    drop(client);
    daemon.stop().await.unwrap();
}

/// A script that appends a rule that loosens permissions to the file at its argument,
/// as a program of a full turn can.
const APPEND_RULE: &str = "import sys\nopen(sys.argv[1], 'a').write('[[permissions.rules]]\\naction = \"execute\"\\nresource = { command = { program = \"cargo\" } }\\neffect = \"allow\"\\n')\n";

/// What `admin.status` says is held, after the clock moved past the watcher's quiet
/// time until `held` says so.
async fn held_after_watch(
    daemon: &TestDaemon,
    client: &Client,
    held: bool,
) -> Option<efr_protocol::HeldConfig> {
    Wait::new("the watcher's reload of config.toml")
        .until_some_async(async || {
            daemon.clock().advance(Duration::from_millis(250));
            let now = status(client).await.config.unwrap().held;
            (now.is_some() == held).then_some(now)
        })
        .await
        .unwrap()
}

// NOTE: two workers, so the watcher and the reload run while the test polls.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_turn_holds_a_config_write_by_a_script() {
    let test = "full_turn_holds_a_config_write_by_a_script";
    if !zsh_enabled(test) || !python3(test) {
        return;
    }
    let dirs = Arc::new(TestDirs::new().unwrap());
    let script = dirs.home().join("append.py");
    std::fs::write(&script, APPEND_RULE).unwrap();
    let config = dirs.dirs().config().join("config.toml");
    // NOTE: the line names only the script; the script names the file, so efr cannot
    // see the write, as with any program that a full turn runs.
    let write = format!("python3 {} {}", script.display(), config.display());
    let model = Recorder::new(vec![shell(&write), shell("sudo true")]);
    let daemon = zsh_full(&dirs, model).config_watch().start().await.unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send(&daemon, &client, 1, "run the script", Some(Mode::Full)).await.unwrap();

    // While the turn waits on its sudo question, the watcher's reload is held.
    let events = next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[]).await;
    let (call_id, _, _) = question(&events);
    assert!(read(&config).contains("[[permissions.rules]]"), "the script wrote the file");
    let held = held_after_watch(&daemon, &client, true).await.unwrap();
    assert_eq!(held.loosens, ["permissions.rules"]);
    // The user's own reload is refused while the full turn runs.
    let refused = reload(&client).await;
    assert!(!refused.applied);
    assert!(refused.error.unwrap().message.contains("while a full turn or its job runs"));
    let _: ApprovalRespondResult = client
        .call(respond(10, sent.conversation_id, call_id, ApprovalDecision::Deny))
        .await
        .unwrap();
    next_stop(&daemon, &client, sent.conversation_id, sent.turn_id, &[call_id]).await;

    // After the turn the change stays held, and the next watcher reload holds it again.
    let after = status(&client).await.config.unwrap().held;
    assert_eq!(after.map(|held| held.loosens), Some(vec!["permissions.rules".to_owned()]));
    // Taking the change out of the file clears the hold at the next reload.
    write_config(&dirs, ALLOW_FULL);
    assert_eq!(held_after_watch(&daemon, &client, false).await, None);
    drop(client);
    daemon.stop().await.unwrap();
}
