//! `efr undo` from the prompt to the files, over a `TestDaemon` (efr's auto spec 10.5
//! to 10.8, 16.4): turns of a scripted model write through efrd's file tools, and
//! `turn.undo` takes them back as `efr undo` asks for it. Undo is a stack per
//! conversation, a redo takes back the newest undo, a running turn refuses it, the
//! `pre` ref of a turn survives a daemon stop, a named floor edit and a write outside
//! the roots come back from the home store, and an edit of the user between two calls
//! of the turn stays. The turns of the project run in `cautious` with no launcher; the
//! reviewer's turns in `auto` run with the fake launcher of `sandbox`. Only the test of
//! the daemon stop drives a real zsh (`EFR_TEST_ZSH=1`).

use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, ApprovalDecision, ApprovalRespond,
    ApprovalRespondResult, ErrorBody, ErrorCode, Event, EventEnvelope, Method, Mode,
    PromptSendResult, TurnId, TurnSettings, TurnUndo, TurnUndoResult, UndoAction, UndoEntry,
    UndoReason, UndoRefused,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{Client, ClientError, TTY, TestDaemon, command_id, events_until};
use efr_test_support::TestDirs;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, reviewed_daemon, run_turn};
use crate::support::zsh_enabled;

/// What the model answers to one request of a turn.
enum Answer {
    /// A call of the tool with its input.
    Call(&'static str, Value),
    /// The text `done`, which ends the turn.
    Done,
    /// The text `done` once the test lets it go.
    HoldThenDone(Arc<Notify>),
    /// No answer at all: the request waits until the daemon stops.
    Hang,
    /// Runs the hook, then answers.
    After(Box<dyn FnOnce() + Send>, Box<Answer>),
}

/// A model that answers each request with the next [`Answer`], over every turn.
struct Script {
    id: ProviderId,
    answers: Mutex<VecDeque<Answer>>,
    calls: AtomicUsize,
}

impl std::fmt::Debug for Script {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Script").finish_non_exhaustive()
    }
}

impl Script {
    fn new(answers: Vec<Answer>) -> Arc<Self> {
        Arc::new(Script {
            id: ProviderId::new("test").unwrap(),
            answers: Mutex::new(answers.into()),
            calls: AtomicUsize::new(0),
        })
    }
}

/// The events of one answer of the model.
fn stream_of(events: Vec<ProviderEvent>) -> ProviderStream {
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}

fn done() -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta { text: "done".to_owned() },
        ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
    ]
}

#[async_trait]
impl Provider for Script {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let mut answer = self.answers.lock().unwrap().pop_front().unwrap_or(Answer::Done);
        loop {
            match answer {
                Answer::After(hook, then) => {
                    hook();
                    answer = *then;
                }
                Answer::Call(name, input) => {
                    let call_id = format!("call_{}", self.calls.fetch_add(1, Ordering::SeqCst));
                    return Ok(stream_of(vec![
                        ProviderEvent::ToolCallStart {
                            call_id: call_id.clone(),
                            name: name.to_owned(),
                            freeform: false,
                        },
                        ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
                        ProviderEvent::Done {
                            stop_reason: StopReason::ToolUse,
                            provider_raw: None,
                        },
                    ]));
                }
                Answer::Done => return Ok(stream_of(done())),
                Answer::HoldThenDone(gate) => {
                    gate.notified().await;
                    return Ok(stream_of(done()));
                }
                Answer::Hang => std::future::pending().await,
            }
        }
    }
}

/// A `write_file` call of `path` with `content`.
fn write(path: impl AsRef<Path>, content: &str) -> Answer {
    Answer::Call("write_file", json!({ "path": path.as_ref(), "content": content }))
}

/// Registers the daemon's working directory as a project.
async fn register_cwd(daemon: &TestDaemon) {
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: daemon.cwd().to_path_buf(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
}

/// A daemon over `script` whose working directory is a registered project.
async fn project_daemon(script: Arc<Script>) -> TestDaemon {
    let daemon = TestDaemon::builder().custom_provider(script).start().await.unwrap();
    register_cwd(&daemon).await;
    daemon
}

/// Sends `text` (prompt `n`) from [`TTY`] in `mode`; with `wait`, also waits for the
/// end of its turn, with every approval allowed, and returns its events.
async fn turn(
    daemon: &TestDaemon,
    n: u128,
    text: &str,
    mode: Mode,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send(daemon, &client, n, text, mode).await;
    let events = until_end(daemon, &client, &sent).await;
    (sent, events)
}

/// Sends `text` (prompt `n`) from [`TTY`] in `mode`.
async fn send(
    daemon: &TestDaemon,
    client: &Client,
    n: u128,
    text: &str,
    mode: Mode,
) -> PromptSendResult {
    let Method::PromptSend(mut params) = daemon.prompt(n, text, TTY) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(mode), ..TurnSettings::default() };
    client.call(Method::PromptSend(params)).await.unwrap()
}

/// The events of the turn of `sent` up to its end, with every approval allowed.
async fn until_end(
    daemon: &TestDaemon,
    client: &Client,
    sent: &PromptSendResult,
) -> Vec<EventEnvelope> {
    let turn_id = sent.turn_id;
    until(daemon, client, sent, |event| ended(event, turn_id)).await
}

/// The events of the turn of `sent` up to the first for which `stop` is true, with
/// every approval allowed.
async fn until(
    daemon: &TestDaemon,
    client: &Client,
    sent: &PromptSendResult,
    mut stop: impl FnMut(&Event) -> bool,
) -> Vec<EventEnvelope> {
    let turn_id = sent.turn_id;
    let mut follow = daemon.follow(client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    loop {
        let events = events_until(&mut follow, |event| {
            let ours = event.turn_id() == Some(turn_id);
            ours && (matches!(event, Event::ApprovalRequested { .. }) || stop(event))
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(
            events.into_iter().filter(|envelope| envelope.event.turn_id() == Some(turn_id)),
        );
        let Event::ApprovalRequested { call_id, .. } = last else { break };
        let answer = Method::ApprovalRespond(ApprovalRespond {
            command_id: answer_id(),
            conversation_id: sent.conversation_id,
            call_id,
            decision: ApprovalDecision::Allow,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    seen
}

/// A command id that no other answer of the test process has.
fn answer_id() -> efr_protocol::CommandId {
    static NEXT: AtomicUsize = AtomicUsize::new(5000);
    command_id(NEXT.fetch_add(1, Ordering::SeqCst) as u128)
}

/// True when `event` ends the turn `turn_id`.
fn ended(event: &Event, turn_id: TurnId) -> bool {
    match event {
        Event::TurnCompleted { turn_id: t, .. }
        | Event::TurnFailed { turn_id: t, .. }
        | Event::TurnInterrupted { turn_id: t, .. } => *t == turn_id,
        _ => false,
    }
}

/// `turn.undo` from [`TTY`] as `efr undo` sends it, command `n`, changed by `change`.
pub(crate) async fn undo(
    daemon: &TestDaemon,
    n: u128,
    change: impl FnOnce(&mut TurnUndo),
) -> Result<TurnUndoResult, ErrorBody> {
    let mut params = TurnUndo {
        command_id: command_id(n),
        conversation_id: None,
        turn_id: None,
        dry_run: false,
        force: false,
        redo: false,
        list: false,
    };
    change(&mut params);
    let client = daemon.client_for_tty(TTY).await.unwrap();
    match client.call::<TurnUndoResult>(Method::TurnUndo(params)).await {
        Ok(result) => Ok(result),
        Err(ClientError::Server { body }) => Err(body),
        Err(other) => panic!("turn.undo failed: {other:?}"),
    }
}

/// The refusal in the data of a `conflict` answer.
pub(crate) fn refusal(body: &ErrorBody) -> UndoRefused {
    assert_eq!(body.code, ErrorCode::Conflict, "{body:?}");
    let data = body.data.clone().unwrap();
    serde_json::from_value(data["refused"].clone()).unwrap()
}

pub(crate) fn entry(action: UndoAction, path: &str) -> UndoEntry {
    UndoEntry { action, path: path.to_owned(), reason: None }
}

fn skip(path: &str, reason: UndoReason) -> UndoEntry {
    UndoEntry { action: UndoAction::Skip, path: path.to_owned(), reason: Some(reason) }
}

pub(crate) fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Each `undo_applied` of the conversation: the undone turn and whether it was a redo.
async fn applied(daemon: &TestDaemon, sent: &PromptSendResult) -> Vec<(TurnId, bool)> {
    let client = daemon.client().await.unwrap();
    let events = daemon.events(&client, sent.conversation_id).await.unwrap();
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::UndoApplied { undone_turn_id, redo, .. } => Some((*undone_turn_id, *redo)),
            _ => None,
        })
        .collect()
}

/// Each file below `dir`, relative to it, with its text, sorted.
pub(crate) fn files_below(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(at) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                found.push((path.strip_prefix(dir).unwrap().to_path_buf(), text));
            }
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn undo_is_a_stack() {
    let script = Script::new(vec![
        write("a.txt", "two\n"),
        Answer::Done,
        write("a.txt", "three\n"),
        write("b.txt", "new\n"),
        Answer::Done,
    ]);
    let daemon = project_daemon(script).await;
    let a = daemon.cwd().join("a.txt");
    std::fs::write(&a, "one\n").unwrap();
    let (first, _) = turn(&daemon, 1, "set a to two", Mode::Cautious).await;
    let (second, _) = turn(&daemon, 2, "set a to three and add b", Mode::Cautious).await;
    let b = daemon.cwd().join("b.txt");
    assert_eq!((read(&a), read(&b)), ("three\n".to_owned(), "new\n".to_owned()));

    // The older turn waits for the newer one.
    let older = undo(&daemon, 10, |p| p.turn_id = Some(first.turn_id)).await.unwrap_err();
    assert_eq!(refusal(&older), UndoRefused::LaterTurnsNotUndone { turn_id: second.turn_id });
    assert_eq!(read(&a), "three\n", "a refusal changes nothing");

    let newest = undo(&daemon, 11, |_| {}).await.unwrap();
    assert!(newest.applied);
    assert_eq!(newest.turn_id, Some(second.turn_id));
    assert_eq!(
        newest.entries,
        [entry(UndoAction::Restore, "a.txt"), entry(UndoAction::Trash, "b.txt")]
    );
    assert_eq!(read(&a), "two\n");
    assert!(!b.exists(), "the file of the turn went to the undo trash");
    let trash = newest.trash.unwrap();
    assert!(trash.starts_with(daemon.dirs().dirs().data().join("undo-trash")), "{trash:?}");
    let kept: Vec<String> = files_below(&trash).into_iter().map(|(_, text)| text).collect();
    assert_eq!(kept, ["three\n", "new\n"], "what undo overwrote or removed");

    let next = undo(&daemon, 12, |_| {}).await.unwrap();
    assert_eq!(next.turn_id, Some(first.turn_id), "the next undo takes the turn below");
    assert_eq!(read(&a), "one\n");

    let empty = undo(&daemon, 13, |_| {}).await.unwrap_err();
    assert_eq!(refusal(&empty), UndoRefused::NothingToUndo);
    assert_eq!(applied(&daemon, &first).await, [(second.turn_id, false), (first.turn_id, false)]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn redo_after_undo() {
    let script = Script::new(vec![write("a.txt", "two\n"), write("c.txt", "made\n"), Answer::Done]);
    let daemon = project_daemon(script).await;
    let a = daemon.cwd().join("a.txt");
    let c = daemon.cwd().join("c.txt");
    std::fs::write(&a, "one\n").unwrap();
    let (sent, _) = turn(&daemon, 1, "set a to two and make c", Mode::Cautious).await;
    let none = undo(&daemon, 10, |p| p.redo = true).await.unwrap_err();
    assert_eq!(refusal(&none), UndoRefused::NoRedo, "nothing to redo before an undo");

    let undone = undo(&daemon, 11, |_| {}).await.unwrap();
    assert!(undone.applied);
    assert_eq!((read(&a), c.exists()), ("one\n".to_owned(), false));
    let listed = undo(&daemon, 12, |p| p.list = true).await.unwrap();
    let [listed] = listed.turns.try_into().unwrap();
    assert_eq!((listed.turn_id, listed.undone), (sent.turn_id, true));

    let preview = undo(&daemon, 13, |p| {
        p.redo = true;
        p.dry_run = true;
    })
    .await
    .unwrap();
    assert!(!preview.applied);
    assert_eq!(preview.turn_id, Some(sent.turn_id));
    assert_eq!(read(&a), "one\n", "a dry run writes nothing");
    let redone = undo(&daemon, 14, |p| p.redo = true).await.unwrap();
    assert!(redone.applied);
    assert_eq!((read(&a), read(&c)), ("two\n".to_owned(), "made\n".to_owned()));
    let listed = undo(&daemon, 15, |p| p.list = true).await.unwrap();
    assert!(!listed.turns[0].undone, "the redo took the undo back");
    let again = undo(&daemon, 16, |p| p.redo = true).await.unwrap_err();
    assert_eq!(refusal(&again), UndoRefused::NoRedo);
    assert_eq!(applied(&daemon, &sent).await, [(sent.turn_id, false), (sent.turn_id, true)]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn undo_refuses_while_turn_runs() {
    let gate = Arc::new(Notify::new());
    let script = Script::new(vec![
        write("a.txt", "two\n"),
        Answer::Done,
        Answer::HoldThenDone(Arc::clone(&gate)),
    ]);
    let daemon = project_daemon(script).await;
    let a = daemon.cwd().join("a.txt");
    std::fs::write(&a, "one\n").unwrap();
    turn(&daemon, 1, "set a to two", Mode::Cautious).await;
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let running = send(&daemon, &client, 2, "say done", Mode::Cautious).await;
    let mut follow = daemon.follow(&client, running.conversation_id).await.unwrap();
    let turn_id = running.turn_id;
    events_until(
        &mut follow,
        |e| matches!(e, Event::TurnStarted { turn_id: t, .. } if *t == turn_id),
    )
    .await
    .unwrap();

    let busy = undo(&daemon, 10, |_| {}).await.unwrap_err();

    assert_eq!(busy.code, ErrorCode::Busy, "{busy:?}");
    assert_eq!(busy.data, Some(json!({ "busy": "turn" })));
    assert_eq!(read(&a), "two\n", "a refusal changes nothing");
    gate.notify_one();
    until_end(&daemon, &client, &running).await;
    let undone = undo(&daemon, 11, |_| {}).await.unwrap();
    assert!(undone.applied);
    assert_eq!(read(&a), "one\n", "the undo works once the turn ended");
    daemon.stop().await.unwrap();
}

// NOTE: the turn's shell call moves its `last` ref, which the undo of a turn with no
// `post` reads; so this test drives a real zsh.
#[tokio::test]
async fn shell_pre_ref_survives_a_daemon_stop() {
    if !zsh_enabled("shell_pre_ref_survives_a_daemon_stop") {
        return;
    }
    let script = Script::new(vec![
        write("a.txt", "two\n"),
        Answer::Call("shell", json!({ "command": "printf 'made\\n' > b.txt" })),
        Answer::Hang,
    ]);
    let mut daemon = TestDaemon::builder()
        .persistent()
        .local_pty()
        .custom_provider(script)
        .start()
        .await
        .unwrap();
    register_cwd(&daemon).await;
    let a = daemon.cwd().join("a.txt");
    let b = daemon.cwd().join("b.txt");
    std::fs::write(&a, "one\n").unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent = send(&daemon, &client, 1, "set a to two and make b", Mode::Cautious).await;
    let mut calls = 0;
    until(&daemon, &client, &sent, |event| {
        calls += usize::from(matches!(event, Event::ToolCallCompleted { .. }));
        calls == 2
    })
    .await;
    drop(client);
    assert_eq!((read(&a), read(&b)), ("two\n".to_owned(), "made\n".to_owned()));

    // NOTE: the model's next answer never comes, so the turn runs when efrd stops.
    daemon.restart().await.unwrap();

    let listed = undo(&daemon, 10, |p| p.list = true).await.unwrap();
    let [listed] = listed.turns.try_into().unwrap();
    assert_eq!((listed.turn_id, listed.stopped, listed.undone), (sent.turn_id, true, false));
    let plain = undo(&daemon, 11, |_| {}).await.unwrap();
    assert!(!plain.applied);
    assert_eq!(
        plain.entries,
        [
            skip("a.txt", UndoReason::NoRecordAfterTurn),
            skip("b.txt", UndoReason::NoRecordAfterTurn)
        ],
        "a turn with no snapshot from its end needs force"
    );
    assert_eq!((read(&a), read(&b)), ("two\n".to_owned(), "made\n".to_owned()));
    let forced = undo(&daemon, 12, |p| p.force = true).await.unwrap();
    assert!(forced.applied, "{forced:?}");
    assert_eq!(forced.turn_id, Some(sent.turn_id));
    assert_eq!(
        forced.entries,
        [entry(UndoAction::Restore, "a.txt"), entry(UndoAction::Trash, "b.txt")],
        "force acts on the paths of the turn's calls up to the stop"
    );
    assert_eq!(read(&a), "one\n");
    assert!(!b.exists());
    daemon.stop().await.unwrap();
}

/// The permission bits of `path`.
fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[tokio::test]
async fn undo_of_named_floor_edit_restores_bytes() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let zshrc = dirs.home().join(".zshrc");
    // NOTE: bytes that a text path could change: a CR, a tab, no newline at the end.
    let old: &[u8] = b"alias ll='ls -l'\r\n\texport A=1";
    std::fs::write(&zshrc, old).unwrap();
    std::fs::set_permissions(&zshrc, std::fs::Permissions::from_mode(0o640)).unwrap();
    let write = json!({ "path": zshrc, "content": "alias k=kubectl\n" });
    let model = ScriptedModel::reviewed(vec![("write_file", write)], REVIEWER_ALLOWS);
    let daemon = reviewed_daemon(&dirs, model).await;
    let (_, events) =
        run_turn(&daemon, "add alias k=kubectl to my zshrc", Mode::Auto, ApprovalDecision::Deny)
            .await;
    assert!(events.iter().any(|e| e.event.kind() == "home_file_changed"));
    assert_eq!(read(&zshrc), "alias k=kubectl\n");
    let after_turn = mode(&zshrc);

    let preview = undo(&daemon, 10, |p| p.dry_run = true).await.unwrap();
    assert_eq!(preview.entries, [entry(UndoAction::Restore, "~/.zshrc")]);
    assert_eq!(read(&zshrc), "alias k=kubectl\n", "a dry run writes nothing");
    let undone = undo(&daemon, 11, |_| {}).await.unwrap();

    assert!(undone.applied);
    assert_eq!(undone.entries, [entry(UndoAction::Restore, "~/.zshrc")]);
    assert_eq!(std::fs::read(&zshrc).unwrap(), old, "the bytes from before the turn");
    assert_eq!(mode(&zshrc), after_turn, "a file that exists keeps its mode");
    // NOTE: the trash holds a home path below `home/`, without its leading `/`.
    let trash = undone.trash.unwrap();
    let below = zshrc.strip_prefix("/").unwrap();
    assert_eq!(
        files_below(&trash),
        [(Path::new("home").join(below), "alias k=kubectl\n".to_owned())],
        "what undo overwrote"
    );
    assert!(!dirs.dirs().data().join("file-backups").exists(), "no phase A backup");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn undo_of_write_file_outside_roots() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let notes = dirs.home().join("Documents/notes.md");
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(&notes, "old notes\n").unwrap();
    let outside = dirs.create_dir("home/outside").unwrap();
    let made = outside.join("b.txt");
    let calls = vec![
        ("write_file", json!({ "path": notes, "content": "new notes\n" })),
        ("write_file", json!({ "path": made, "content": "hello\n" })),
    ];
    let model = ScriptedModel::reviewed(calls, REVIEWER_ALLOWS);
    let daemon = reviewed_daemon(&dirs, model).await;
    run_turn(
        &daemon,
        "put new notes in ~/Documents/notes.md and write hello into ~/outside/b.txt",
        Mode::Auto,
        ApprovalDecision::Deny,
    )
    .await;
    assert_eq!((read(&notes), read(&made)), ("new notes\n".to_owned(), "hello\n".to_owned()));

    let undone = undo(&daemon, 10, |_| {}).await.unwrap();

    assert!(undone.applied);
    assert_eq!(
        undone.entries,
        [
            entry(UndoAction::Restore, "~/Documents/notes.md"),
            entry(UndoAction::Trash, "~/outside/b.txt")
        ]
    );
    assert_eq!(read(&notes), "old notes\n");
    assert!(!made.exists(), "a file that was missing before the turn goes to the undo trash");
    assert!(outside.is_dir(), "its directory stays");
    let home = |path: &Path| Path::new("home").join(path.strip_prefix("/").unwrap());
    assert_eq!(
        files_below(&undone.trash.unwrap()),
        [(home(&notes), "new notes\n".to_owned()), (home(&made), "hello\n".to_owned())]
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn undo_leaves_a_user_edit_made_during_the_turn() {
    let cwd = Arc::new(Mutex::new(PathBuf::new()));
    let at = Arc::clone(&cwd);
    // NOTE: between the turn's two calls, the user makes a file and edits another.
    let user_edits = move || {
        let cwd = at.lock().unwrap().clone();
        std::fs::write(cwd.join("mine.txt"), "mine\n").unwrap();
        std::fs::write(cwd.join("keep.txt"), "edited by the user\n").unwrap();
    };
    let script = Script::new(vec![
        write("a.txt", "two\n"),
        Answer::After(Box::new(user_edits), Box::new(write("b.txt", "made\n"))),
        Answer::Done,
    ]);
    let daemon = project_daemon(script).await;
    *cwd.lock().unwrap() = daemon.cwd().to_path_buf();
    let a = daemon.cwd().join("a.txt");
    let keep = daemon.cwd().join("keep.txt");
    let mine = daemon.cwd().join("mine.txt");
    std::fs::write(&a, "one\n").unwrap();
    std::fs::write(&keep, "kept\n").unwrap();
    turn(&daemon, 1, "set a to two and make b", Mode::Cautious).await;
    assert_eq!(read(&mine), "mine\n");

    let rows = [
        entry(UndoAction::Restore, "a.txt"),
        entry(UndoAction::Trash, "b.txt"),
        skip("keep.txt", UndoReason::OutsideTurnCalls),
        skip("mine.txt", UndoReason::OutsideTurnCalls),
    ];
    let preview = undo(&daemon, 10, |p| p.dry_run = true).await.unwrap();
    assert_eq!(preview.entries, rows, "undo acts only on the paths of the turn's calls");
    let undone = undo(&daemon, 11, |_| {}).await.unwrap();

    assert!(undone.applied);
    assert_eq!(undone.entries, rows);
    assert_eq!(read(&a), "one\n");
    assert!(!daemon.cwd().join("b.txt").exists());
    assert_eq!(read(&mine), "mine\n", "the user's new file stays");
    assert_eq!(read(&keep), "edited by the user\n", "the user's edit stays");
    daemon.stop().await.unwrap();
}
