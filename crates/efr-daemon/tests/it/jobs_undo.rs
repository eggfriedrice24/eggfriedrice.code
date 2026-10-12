//! Undo while background jobs run, and after one ended (efr's auto spec 3.18 and 10.5,
//! phase C), through a `TestDaemon` with the shared fake launcher (`/bin/sh`, no zsh),
//! the `job_env` seam and a scripted model: `turn.undo` waits while a job of the
//! conversation runs, or a job of another conversation writes in a root of the turn;
//! after the job's end, its files belong to the turn that started it.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, ApprovalDecision, ConversationId, ErrorBody, ErrorCode,
    Event, Method, Mode, TurnUndo, TurnUndoResult, UndoAction, UndoReason,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{ClientError, FakeLauncher, TTY, TestDaemon, command_id, events_until};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::sandbox::{REVIEWER_ALLOWS, ready, run_turn_with};

/// The second terminal of a test, with a conversation of its own.
const OTHER: &str = "/dev/pts/efr-jobs-undo";

/// A model that answers each request of every turn with the next call of its list, or
/// `done` when the turn's calls are used up: `None` in the list ends a turn.
pub(crate) struct Script {
    id: ProviderId,
    answers: Mutex<VecDeque<Option<(&'static str, Value)>>>,
}

impl std::fmt::Debug for Script {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Script").finish_non_exhaustive()
    }
}

impl Script {
    pub(crate) fn new(answers: Vec<Option<(&'static str, Value)>>) -> Arc<Self> {
        Arc::new(Script {
            id: ProviderId::new("test").unwrap(),
            answers: Mutex::new(answers.into()),
        })
    }
}

fn stream(events: Vec<ProviderEvent>) -> ProviderStream {
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}

#[async_trait]
impl Provider for Script {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        if request.one_shot {
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: REVIEWER_ALLOWS.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        }
        let next = self.answers.lock().unwrap().pop_front().flatten();
        let Some((name, input)) = next else {
            return Ok(stream(vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]));
        };
        let call_id = format!("call_{name}_{}", input.to_string().len());
        Ok(stream(vec![
            ProviderEvent::ToolCallStart {
                call_id: call_id.clone(),
                name: name.to_owned(),
                freeform: false,
            },
            ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
            ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
        ]))
    }
}

/// A daemon over `model` with the fake launcher, a ready sandbox and a fixed job
/// environment, whose working directory is a registered project.
async fn start(dirs: &Arc<TestDirs>, model: Arc<Script>) -> TestDaemon {
    let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
    let env = BTreeMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), dirs.home().display().to_string()),
    ]);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(dirs))
        .sandbox_launcher(fake.path())
        .probe_override(ready())
        .job_env(env)
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let client = daemon.client().await.unwrap();
    let add = Method::AdminProjectAdd(AdminProjectAdd {
        path: daemon.cwd().to_path_buf(),
        name: None,
        git_root: false,
    });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    daemon
}

/// A FIFO below the test's root, which a job reads and the test writes.
fn fifo(dirs: &TestDirs, name: &str) -> PathBuf {
    let path = dirs.root().join(name);
    nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    path
}

/// Writes one line to `fifo`; it waits for the job that reads it.
async fn release(fifo: &Path) {
    let fifo = fifo.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
}

/// A background `shell` call that waits for `fifo`, then writes `job.txt` in its start
/// dir.
fn job_writing(fifo: &Path) -> Option<(&'static str, Value)> {
    let line = format!("read answer < {}; echo hi > job.txt", fifo.display());
    Some(("shell", json!({ "command": line, "background": true })))
}

/// The undo of the newest turn of the conversation of `tty`, as `efr undo` asks for it.
async fn undo(
    daemon: &TestDaemon,
    tty: &str,
    n: u128,
    change: impl FnOnce(&mut TurnUndo),
) -> Result<TurnUndoResult, ErrorBody> {
    let mut params = TurnUndo {
        command_id: command_id(n),
        conversation_id: None,
        turn_id: None,
        dry_run: true,
        force: false,
        redo: false,
        list: false,
    };
    change(&mut params);
    let client = daemon.client_for_tty(tty).await.unwrap();
    match client.call::<TurnUndoResult>(Method::TurnUndo(params)).await {
        Ok(result) => Ok(result),
        Err(ClientError::Server { body }) => Err(body),
        Err(other) => panic!("turn.undo failed: {other:?}"),
    }
}

/// Waits for the `job_ended` of `id` in `conversation`.
async fn job_ended(daemon: &TestDaemon, conversation: ConversationId, id: &str) {
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    events_until(
        &mut follow,
        |event| matches!(event, Event::JobEnded { job_id, .. } if job_id.as_str() == id),
    )
    .await
    .unwrap();
}

/// The undo of the conversation of `tty` once no job keeps it waiting: the job's end
/// joins its files to its turn before the job leaves efrd's table.
async fn undo_once_free(daemon: &TestDaemon, tty: &str, n: u128, force: bool) -> TurnUndoResult {
    let mut tries = 0;
    Wait::new("an undo that no job keeps waiting")
        .until_some_async(async || {
            tries += 1;
            match undo(daemon, tty, n + tries, |params| params.force = force).await {
                Ok(result) => Some(result),
                Err(body) if body.code == ErrorCode::Busy => None,
                Err(body) => panic!("{body:?}"),
            }
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn undo_refuses_while_a_job_runs() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let gate = fifo(&dirs, "go");
    let daemon = start(&dirs, Script::new(vec![job_writing(&gate), None])).await;
    let (sent, _) = run_turn_with(
        &daemon,
        TTY,
        1,
        "write job.txt as a job",
        Mode::Auto,
        ApprovalDecision::Deny,
    )
    .await;

    let busy = undo(&daemon, TTY, 10, |_| {}).await.unwrap_err();
    assert_eq!(busy.code, ErrorCode::Busy);
    assert_eq!(busy.message, "job j1 runs; stop it first (efr jobs stop j1)");
    assert_eq!(busy.data.unwrap()["job_id"], "j1");

    release(&gate).await;
    job_ended(&daemon, sent.conversation_id, "j1").await;
    assert_eq!(std::fs::read_to_string(daemon.cwd().join("job.txt")).unwrap(), "hi\n");
    // NOTE: the job's file joined the turn that started it, which now has no record
    // after it: its undo needs force.
    let preview = undo_once_free(&daemon, TTY, 20, false).await;
    let row = preview.entries.iter().find(|entry| entry.path.ends_with("job.txt")).unwrap();
    assert_eq!(row.reason, Some(UndoReason::NoRecordAfterTurn), "{row:?}");
    let forced = undo_once_free(&daemon, TTY, 40, true).await;
    let row = forced.entries.iter().find(|entry| entry.path.ends_with("job.txt")).unwrap();
    assert_eq!(row.action, UndoAction::Trash, "{row:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn undo_waits_for_job_of_other_conversation() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let gate = fifo(&dirs, "go");
    let model = Script::new(vec![
        Some(("write_file", json!({ "path": "a.txt", "content": "from the first\n" }))),
        None,
        job_writing(&gate),
        None,
    ]);
    let daemon = start(&dirs, model).await;
    run_turn_with(&daemon, TTY, 1, "write a.txt", Mode::Auto, ApprovalDecision::Allow).await;
    let (other, _) = run_turn_with(
        &daemon,
        OTHER,
        2,
        "write job.txt as a job",
        Mode::Auto,
        ApprovalDecision::Deny,
    )
    .await;

    let busy = undo(&daemon, TTY, 10, |_| {}).await.unwrap_err();
    assert_eq!(busy.code, ErrorCode::Busy, "{busy:?}");
    assert_eq!(
        busy.message,
        "a job of another conversation writes here; stop it first (efr jobs list --all)"
    );
    assert_eq!(busy.data.unwrap()["same_conversation"], false);

    release(&gate).await;
    job_ended(&daemon, other.conversation_id, "j1").await;
    let preview = undo_once_free(&daemon, TTY, 20, false).await;
    let rows: Vec<(&str, UndoAction)> =
        preview.entries.iter().map(|entry| (entry.path.as_str(), entry.action)).collect();
    assert_eq!(rows, [("a.txt", UndoAction::Trash)], "the other conversation's job.txt stays");
    daemon.stop().await.unwrap();
}
