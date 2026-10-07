//! What the agent changed in files, end to end: the changes and the diff of a file
//! tool's call, the changes of a `shell` call and of the turn from efr's own snapshot
//! store, and `conversation.diff` with and without `stat`. The `shell_` tests drive a
//! real zsh (`EFR_TEST_ZSH=1`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, ApprovalDecision, ApprovalRespond,
    ApprovalRespondResult, ChangeKind, ConversationDiff, ConversationDiffResult, ConversationId,
    ErrorCode, Event, EventEnvelope, FileChanges, Method, PromptSendResult, TurnId,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_daemon::{ClientError, TTY, TestDaemon, command_id, events_until};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::support::zsh_enabled;

/// A model that calls each `(tool, input)` in turn, then says `done`.
#[derive(Debug)]
struct ScriptedModel {
    id: ProviderId,
    calls: Vec<(&'static str, Value)>,
    asked: AtomicUsize,
}

impl ScriptedModel {
    fn new(calls: Vec<(&'static str, Value)>) -> Arc<Self> {
        Arc::new(ScriptedModel {
            id: ProviderId::new("test").unwrap(),
            calls,
            asked: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Provider for ScriptedModel {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        let events = match self.calls.get(n) {
            Some((tool, input)) => {
                let call_id = format!("call_{n}");
                vec![
                    ProviderEvent::ToolCallStart {
                        call_id: call_id.clone(),
                        name: (*tool).to_owned(),
                    },
                    ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
                    ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None },
                ]
            }
            None => vec![
                ProviderEvent::TextDelta { text: "done".to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ],
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
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

/// A turn's events, with every approval allowed.
async fn run_turn(daemon: &TestDaemon, text: &str) -> (PromptSendResult, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(1, text, TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = 10;
    loop {
        let events = events_until(&mut follow, |event| {
            matches!(
                event,
                Event::ApprovalRequested { .. }
                    | Event::TurnCompleted { .. }
                    | Event::TurnFailed { .. }
                    | Event::TurnInterrupted { .. }
            )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events);
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
    (sent, seen)
}

/// The `changes` and `diff` of the turn's only completed call.
fn call_changes(events: &[EventEnvelope]) -> (Option<FileChanges>, Option<String>) {
    let mut found = events.iter().filter_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { changes, diff, .. } => Some((changes.clone(), diff.clone())),
        _ => None,
    });
    let first = found.next().unwrap();
    assert!(found.next().is_none(), "one call");
    first
}

fn turn_changes(events: &[EventEnvelope]) -> Option<FileChanges> {
    events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::TurnCompleted { changes, .. } => Some(changes.clone()),
            _ => None,
        })
        .unwrap()
}

async fn diff(
    daemon: &TestDaemon,
    conversation_id: Option<ConversationId>,
    turn_id: Option<TurnId>,
    stat: bool,
) -> Result<ConversationDiffResult, ClientError> {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    client.call(Method::ConversationDiff(ConversationDiff { conversation_id, turn_id, stat })).await
}

fn paths(changes: &FileChanges) -> Vec<(&str, ChangeKind)> {
    changes.files.iter().map(|file| (file.path.as_str(), file.kind)).collect()
}

#[tokio::test]
async fn a_file_write_in_a_project_shows_its_diff_and_the_turn_keeps_it() {
    let input = json!({ "path": "notes.md", "content": "one\ntwo\n" });
    let model = ScriptedModel::new(vec![("write_file", input)]);
    let daemon = TestDaemon::builder().custom_provider(model).start().await.unwrap();
    register_cwd(&daemon).await;
    std::fs::write(daemon.cwd().join("keep.txt"), "kept\n").unwrap();

    let (sent, events) = run_turn(&daemon, "write a note").await;

    let (changes, diff_text) = call_changes(&events);
    let changes = changes.unwrap();
    assert_eq!(paths(&changes), [("notes.md", ChangeKind::Added)]);
    assert_eq!((changes.added, changes.removed), (2, 0));
    assert_eq!(diff_text.unwrap(), "--- /dev/null\n+++ b/notes.md\n@@ -0,0 +1,2 @@\n+one\n+two\n");
    let turn = turn_changes(&events).unwrap();
    assert_eq!(paths(&turn), [("notes.md", ChangeKind::Added)]);

    let full = diff(&daemon, None, None, false).await.unwrap();
    assert_eq!(full.turn_id, sent.turn_id);
    assert_eq!(full.changes, turn);
    let patch = full.diff.unwrap();
    assert!(patch.contains("+++ b/notes.md\n"), "{patch}");
    assert!(patch.contains("+one\n+two\n"), "{patch}");

    let stat = diff(&daemon, Some(sent.conversation_id), Some(sent.turn_id), true).await.unwrap();
    assert_eq!(stat.changes, turn);
    assert_eq!(stat.diff, None, "stat leaves the diff out");

    // A connection with no terminal finds the conversation through the turn.
    let client = daemon.client().await.unwrap();
    let by_turn =
        ConversationDiff { conversation_id: None, turn_id: Some(sent.turn_id), stat: true };
    let by_turn: ConversationDiffResult =
        client.call(Method::ConversationDiff(by_turn)).await.unwrap();
    assert_eq!(by_turn.changes, turn);
    let elsewhere = ConversationDiff {
        conversation_id: Some(ConversationId::from_uuid(uuid::Uuid::from_u128(7))),
        turn_id: Some(sent.turn_id),
        stat: true,
    };
    let elsewhere = client.call::<ConversationDiffResult>(Method::ConversationDiff(elsewhere));
    let elsewhere = elsewhere.await.unwrap_err();
    assert!(
        matches!(&elsewhere, ClientError::Server { body } if body.code == ErrorCode::NotFound),
        "{elsewhere:?}"
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn conversation_diff_of_a_turn_without_snapshots_is_empty_and_unknown_ones_are_not_found() {
    let model = ScriptedModel::new(Vec::new());
    let daemon = TestDaemon::builder().custom_provider(model).start().await.unwrap();

    let none = diff(&daemon, None, None, false).await.unwrap_err();
    assert!(
        matches!(&none, ClientError::Server { body } if body.code == ErrorCode::NotFound),
        "{none:?}"
    );

    let (sent, _) = run_turn(&daemon, "hello").await;
    let empty = diff(&daemon, Some(sent.conversation_id), None, false).await.unwrap();
    assert_eq!(empty.turn_id, sent.turn_id, "the last finished turn");
    assert!(empty.changes.is_empty(), "{empty:?}");
    let other_turn = TurnId::from_uuid(uuid::Uuid::from_u128(9));
    let unknown_turn = diff(&daemon, None, Some(other_turn), true).await.unwrap_err();
    assert!(
        matches!(&unknown_turn, ClientError::Server { body } if body.code == ErrorCode::NotFound),
        "{unknown_turn:?}"
    );
    let unknown =
        diff(&daemon, Some(ConversationId::from_uuid(uuid::Uuid::from_u128(7))), None, true)
            .await
            .unwrap_err();
    assert!(
        matches!(&unknown, ClientError::Server { body } if body.code == ErrorCode::NotFound),
        "{unknown:?}"
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_a_call_lists_what_it_changed_in_a_plain_project() {
    let test = "shell_a_call_lists_what_it_changed_in_a_plain_project";
    if !zsh_enabled(test) {
        return;
    }
    let line = "printf 'new\\n' > a.txt && rm gone.txt && mv old.txt new.txt && mkdir -p sub && printf 'x\\n' > sub/b.txt";
    let model = ScriptedModel::new(vec![("shell", json!({ "command": line }))]);
    let daemon = TestDaemon::builder().local_pty().custom_provider(model).start().await.unwrap();
    register_cwd(&daemon).await;
    let cwd = daemon.cwd().to_path_buf();
    write(&cwd, "a.txt", "old\n");
    write(&cwd, "gone.txt", "1\n2\n");
    write(&cwd, "old.txt", "moved\n");

    let (sent, events) = run_turn(&daemon, "change files").await;

    let (changes, diff_text) = call_changes(&events);
    assert_eq!(diff_text, None, "a shell call has no inline diff");
    let changes = changes.unwrap();
    assert_eq!(
        paths(&changes),
        [
            ("a.txt", ChangeKind::Modified),
            ("gone.txt", ChangeKind::Deleted),
            ("new.txt", ChangeKind::Renamed),
            ("sub/b.txt", ChangeKind::Added),
        ]
    );
    assert_eq!(turn_changes(&events).unwrap(), changes);
    let full = diff(&daemon, Some(sent.conversation_id), None, false).await.unwrap();
    assert!(full.diff.unwrap().contains("-old\n+new\n"));
    assert!(!cwd.join(".git").exists(), "the project gets no .git");
    daemon.stop().await.unwrap();
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}
