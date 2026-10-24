//! `conversation.delete` over a `TestDaemon`: everything of the conversation goes and
//! nothing of another one, a search finds none of its text, a terminal that holds it
//! keeps it unless the user says `force`, a running turn keeps it unless the user says
//! `stop`, and a delete that a stop of efrd broke off after its tombstone ends at the
//! next start. The stop of a running job first is in `jobs.rs`
//! (`delete_stops_a_running_job_first`), next to the fake launcher.
//!
//! The conversations come from the prompts of `routing.rs` (`prompt N`), whose turns run
//! against the empty replay model and fail at once. The files of a conversation outside
//! the database are planted by the test, at the paths where efrd keeps them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use efr_protocol::{
    ConversationDelete, ConversationDeleteResult, ConversationHistory, ConversationId,
    ConversationsList, ConversationsListResult, ConversationsSearch, ConversationsSearchResult,
    ErrorCode, Event, Method, Origin, PromptSendResult, PtyId, Seq, UndoId,
};
use efr_provider::{Provider, ProviderError, ProviderId, ProviderStream, Request};
use efr_store::receipts::NewReceipt;
use efr_store::{Batch, Store, StoreConfig};
use efr_test_daemon::{Client, ClientError, TestDaemon, command_id, events_until};
use efr_test_support::{TestClock, TestDirs};
use pretty_assertions::assert_eq;

use crate::routing::{Place, tree};
use crate::search::indexed;

/// `conversation.delete` of `conversation` with the command id `n`.
fn params(n: u128, conversation: ConversationId, force: bool, stop: bool) -> Method {
    Method::ConversationDelete(ConversationDelete {
        command_id: command_id(5000 + n),
        conversation_id: conversation,
        force,
        stop,
    })
}

async fn delete(
    client: &Client,
    n: u128,
    conversation: ConversationId,
    force: bool,
) -> Result<ConversationDeleteResult, ClientError> {
    client.call(params(n, conversation, force, false)).await
}

fn deleted(result: &ConversationDeleteResult) -> (ConversationId, u32, Vec<PathBuf>) {
    match result {
        ConversationDeleteResult::Deleted {
            conversation_id, jobs_stopped, not_removed, ..
        } => (*conversation_id, *jobs_stopped, not_removed.clone()),
        other => panic!("not deleted: {other:?}"),
    }
}

/// Files of `conversation` at each place where efrd keeps something of it outside the
/// database and the snapshot stores, with a dir that a command made read-only; the
/// paths that a delete must remove.
fn plant(dirs: &TestDirs, conversation: ConversationId) -> Vec<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let roots = dirs.dirs();
    let id = conversation.to_string();
    let tail: String = id.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
    let scratch = roots.data().join("scratch").join(format!("2026-10-24-prompt-{tail}"));
    std::fs::create_dir_all(scratch.join("jobs")).unwrap();
    std::fs::write(scratch.join(".efr-conversation"), format!("{id}\n")).unwrap();
    std::fs::write(scratch.join("jobs/j1.out"), b"the output of a job\n").unwrap();
    let sandbox = roots.state().join("sandbox").join(&id);
    let work = sandbox.join("cache/cargo/work/work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(sandbox.join("last-call"), b"").unwrap();
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o000)).unwrap();
    let shell = roots.runtime().join("sbx").join(&id);
    std::fs::create_dir_all(&shell).unwrap();
    std::fs::write(shell.join("env"), b"SECRET=1\0").unwrap();
    let backups = roots.data().join("file-backups").join(&id);
    std::fs::create_dir_all(backups.join("t/1")).unwrap();
    std::fs::write(backups.join("t/1/hosts"), b"127.0.0.1 x\n").unwrap();
    vec![scratch, sandbox, shell, backups]
}

/// The recording of `pty` (one segment through the store) and the undo trash of
/// `undo`, which a seeded log names; the paths that a delete must remove.
async fn plant_outside_events(
    store: &Store,
    dirs: &TestDirs,
    pty: PtyId,
    undo: UndoId,
) -> Vec<PathBuf> {
    let recordings = dirs.dirs().data().join("recordings");
    let recording = efr_store::recording::Recordings::new(
        &recordings,
        store.writer().clone(),
        store.readers().clone(),
        TestClock::new().shared(),
    );
    let mut writer = recording.start(pty).await.unwrap();
    writer.append(b"$ rsync -a photos laptop:\r\n").await.unwrap();
    writer.close().await.unwrap();
    let trash = dirs.dirs().data().join("undo-trash").join(undo.to_string());
    std::fs::create_dir_all(trash.join("root/x")).unwrap();
    std::fs::write(trash.join("root/x/notes.md"), b"old notes\n").unwrap();
    vec![recordings.join(pty.to_string()), trash]
}

/// The conversations that `conversations.list` shows.
async fn listed(daemon: &TestDaemon) -> Vec<ConversationId> {
    let client = daemon.client().await.unwrap();
    let method = Method::ConversationsList(ConversationsList::default());
    let list: ConversationsListResult = client.call(method).await.unwrap();
    list.conversations.into_iter().map(|summary| summary.id).collect()
}

/// The conversations whose rows a search for `query` shows.
async fn found(daemon: &TestDaemon, query: &str) -> Vec<ConversationId> {
    let client = daemon.client().await.unwrap();
    let method = Method::ConversationsSearch(ConversationsSearch {
        query: query.to_owned(),
        limit: Some(50),
        ..Default::default()
    });
    let result: ConversationsSearchResult = client.call(method).await.unwrap();
    result.rows.into_iter().map(|row| row.conversation.summary.id).collect()
}

/// The newest sequence number of the log, through the conversations that it shows.
async fn newest(daemon: &TestDaemon, conversations: &[ConversationId]) -> Seq {
    let client = daemon.client().await.unwrap();
    let mut newest = Seq::ZERO;
    for &conversation in conversations {
        let events = daemon.events(&client, conversation).await.unwrap();
        newest = newest.max(events.last().map_or(Seq::ZERO, |envelope| envelope.seq));
    }
    newest
}

/// What the store keeps of `conversation` after the daemon stopped: its events, the
/// receipt of the prompt `n`, its saved messages, its open delete, the segments of
/// `pty`, and whether its tombstone is done.
async fn kept(
    dirs: &TestDirs,
    conversation: ConversationId,
    prompt: u128,
    pty: Option<PtyId>,
) -> (Vec<String>, bool, usize, usize, bool) {
    let clock = TestClock::new();
    let store =
        Store::open(StoreConfig::in_data_dir(dirs.dirs().data()), clock.shared()).await.unwrap();
    let kept = store
        .readers()
        .with(move |conn| {
            let events =
                efr_store::events::read_conversation_after(conn, conversation, Seq::ZERO, 1000)?;
            let kinds = events.iter().map(|envelope| envelope.event.kind().to_owned()).collect();
            let receipt = efr_store::receipts::lookup(conn, command_id(prompt))?.is_some();
            let messages = efr_store::turn_messages::of_conversation(conn, conversation)?.len();
            let segments = match pty {
                Some(pty) => efr_store::recording::segments(conn, pty)?.len(),
                None => 0,
            };
            let done = efr_store::deletes::pending(conn)?.is_empty()
                && efr_store::deletes::get(conn, conversation)?.is_some();
            Ok((kinds, receipt, messages, segments, done))
        })
        .await
        .unwrap();
    store.close().await;
    kept
}

#[tokio::test]
async fn delete_removes_every_trace_and_search_finds_none_of_its_text() {
    let place = Place::start_with(|builder| builder.persistent()).await;
    let dirs = Arc::clone(place.daemon.dirs());
    let mut first = Place::prompt(1, &place.app, "pts-1", 100);
    first.text = "rsync the photos to the laptop".to_owned();
    let gone = place.send(first).await;
    let mut second = Place::prompt(2, &place.lib, "pts-2", 200);
    second.text = "docker prune the images".to_owned();
    let kept_one = place.send(second).await;
    let gone_files = plant(&dirs, gone.conversation_id);
    let kept_files = plant(&dirs, kept_one.conversation_id);
    let seq = newest(&place.daemon, &[gone.conversation_id, kept_one.conversation_id]).await;
    indexed(&place.daemon, seq).await;
    assert_eq!(found(&place.daemon, "rsync").await, [gone.conversation_id]);

    let client = place.daemon.client_for_tty("pts-1").await.unwrap();
    let answer = delete(&client, 1, gone.conversation_id, false).await.unwrap();

    assert_eq!(deleted(&answer), (gone.conversation_id, 0, Vec::new()));
    assert_eq!(listed(&place.daemon).await, [kept_one.conversation_id]);
    assert_eq!(found(&place.daemon, "rsync").await, Vec::<ConversationId>::new());
    assert_eq!(found(&place.daemon, "laptop").await, Vec::<ConversationId>::new());
    assert_eq!(found(&place.daemon, "docker").await, [kept_one.conversation_id]);
    let history = Method::ConversationHistory(ConversationHistory {
        conversation_id: gone.conversation_id,
        cursor: None,
        limit: None,
    });
    let error = client.call::<serde_json::Value>(history).await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { body } if body.code == ErrorCode::NotFound),
        "{error:?}"
    );
    for path in &gone_files {
        assert!(path.symlink_metadata().is_err(), "{} is still there", path.display());
    }
    for path in &kept_files {
        assert!(path.exists(), "{} of the other conversation went", path.display());
    }
    // A retry of the command answers from its receipt; a new command finds nothing.
    let again = client.call(params(1, gone.conversation_id, false, false)).await.unwrap();
    assert_eq!(deleted(&again).0, gone.conversation_id);
    let error = delete(&client, 2, gone.conversation_id, false).await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { body } if body.code == ErrorCode::NotFound),
        "{error:?}"
    );
    // The terminal's next prompt in that scope starts a new conversation.
    let next = place.at(3, &place.app, "pts-1", 100).await;
    assert_ne!(next.conversation_id, gone.conversation_id);
    drop(client);
    place.stop().await;

    let (kinds, receipt, messages, _, done) = kept(&dirs, gone.conversation_id, 1, None).await;
    assert_eq!(kinds, ["conversation_deleted"], "only the tombstone stays");
    assert!(!receipt, "the receipt of its prompt went");
    assert_eq!(messages, 0);
    assert!(done);
    let (kinds, receipt, ..) = kept(&dirs, kept_one.conversation_id, 2, None).await;
    assert!(kinds.len() > 3 && receipt, "the other conversation keeps its log: {kinds:?}");
}

#[tokio::test]
async fn a_conversation_that_another_terminal_holds_is_deleted_only_with_force() {
    let place = Place::start().await;
    let theirs = place.at(1, &place.app, "pts-1", 100).await;
    let mine = place.at(2, &place.lib, "pts-2", 200).await;
    let client = place.daemon.client_for_tty("pts-2").await.unwrap();

    let answer = delete(&client, 1, theirs.conversation_id, false).await.unwrap();

    assert_eq!(
        answer,
        ConversationDeleteResult::Held {
            tty: "pts-1".to_owned(),
            title: Some("prompt 1".to_owned())
        }
    );
    assert_eq!(listed(&place.daemon).await.len(), 2, "nothing went");
    // The terminal that holds a conversation deletes it with no force.
    let answer = delete(&client, 2, mine.conversation_id, false).await.unwrap();
    assert_eq!(deleted(&answer).0, mine.conversation_id);
    let answer = delete(&client, 3, theirs.conversation_id, true).await.unwrap();
    assert_eq!(deleted(&answer).0, theirs.conversation_id);
    assert_eq!(listed(&place.daemon).await, Vec::<ConversationId>::new());
    assert_eq!(place.held_by("pts-1").await, Vec::<ConversationId>::new());
    drop(client);
    place.stop().await;
}

#[tokio::test]
async fn a_conversation_whose_shell_ended_is_held_by_no_terminal() {
    let place = Place::start().await;
    let theirs = place.at(1, &place.app, "pts-1", 100).await;
    place.procs.kill(100);
    let client = place.daemon.client_for_tty("pts-2").await.unwrap();

    let answer = delete(&client, 1, theirs.conversation_id, false).await.unwrap();

    assert_eq!(deleted(&answer).0, theirs.conversation_id);
    drop(client);
    place.stop().await;
}

/// A model whose stream never ends, so a turn runs until something stops it.
#[derive(Debug)]
struct Endless {
    id: ProviderId,
}

#[async_trait]
impl Provider for Endless {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures::stream::pending()))
    }
}

#[tokio::test]
async fn a_running_turn_is_busy_unless_the_delete_stops_it() {
    let (dirs, procs) = tree();
    let model: Arc<dyn Provider> = Arc::new(Endless { id: ProviderId::new("test").unwrap() });
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .proc_view(procs.clone())
        .custom_provider(model)
        .start()
        .await
        .unwrap();
    let place = Place::on(daemon, procs, &dirs);
    let client = place.daemon.client_for_tty("pts-1").await.unwrap();
    let sent: PromptSendResult =
        client.call(Method::PromptSend(Place::prompt(1, &place.app, "pts-1", 100))).await.unwrap();
    let mut follow = place.daemon.follow(&client, sent.conversation_id).await.unwrap();
    events_until(&mut follow, |event| matches!(event, Event::TurnStarted { .. })).await.unwrap();

    let error = delete(&client, 1, sent.conversation_id, false).await.unwrap_err();

    let ClientError::Server { body } = &error else { panic!("{error:?}") };
    assert_eq!(body.code, ErrorCode::Busy);
    assert_eq!(body.data, Some(serde_json::json!({ "busy": "turn" })));
    assert_eq!(listed(&place.daemon).await, [sent.conversation_id]);

    let answer = client.call(params(2, sent.conversation_id, false, true)).await.unwrap();

    assert_eq!(deleted(&answer).0, sent.conversation_id);
    // The follow of the conversation ends with its tombstone.
    let rest =
        events_until(&mut follow, |event| matches!(event, Event::ConversationDeleted { .. }))
            .await
            .unwrap();
    assert!(matches!(
        rest.last().unwrap().event,
        Event::ConversationDeleted { origin: Origin::Shell }
    ));
    assert_eq!(listed(&place.daemon).await, Vec::<ConversationId>::new());
    drop((client, follow));
    place.stop().await;
}

#[tokio::test]
async fn a_delete_that_stopped_after_its_tombstone_ends_at_the_next_start() {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let clock = TestClock::new();
    let gone: ConversationId = "0192f0c1-7a00-7000-8000-00000000a001".parse().unwrap();
    let kept_one: ConversationId = "0192f0c1-7a00-7000-8000-00000000a002".parse().unwrap();
    let pty: PtyId = "0192f0c1-7a00-7000-8000-00000000b001".parse().unwrap();
    let undo: UndoId = "0192f0c1-7a00-7000-8000-00000000c001".parse().unwrap();
    let store =
        Store::open(StoreConfig::in_data_dir(dirs.dirs().data()), clock.shared()).await.unwrap();
    for (conversation, n, text) in [(gone, 1, "rsync the photos"), (kept_one, 2, "docker prune")] {
        let place = Place::prompt(n, Path::new("/srv"), "pts-1", 100);
        let queued = Event::PromptQueued {
            turn_id: format!("0192f0c1-7a00-7000-8000-00000000d00{n}").parse().unwrap(),
            command_id: command_id(n),
            text: text.to_owned(),
            origin: Origin::Shell,
            context: place.context,
            settings: Default::default(),
            steers: Vec::new(),
        };
        let mut batch = Batch::new()
            .event(
                conversation,
                Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None },
            )
            .event(conversation, queued)
            .receipt(NewReceipt::accepted(command_id(n), "prompt.send", serde_json::json!({})));
        if conversation == gone {
            batch = batch
                .event(
                    conversation,
                    Event::ShellStarted { pty_id: pty, cwd: "/srv".into(), pid: Some(4242) },
                )
                .event(conversation, undo_applied(gone, undo));
        }
        store.writer().append(batch).await.unwrap();
    }
    let mut gone_files = plant(&dirs, gone);
    gone_files.extend(plant_outside_events(&store, &dirs, pty, undo).await);
    let kept_files = plant(&dirs, kept_one);
    // The stop came right after the tombstone committed.
    let tombstone = Batch::new().event(gone, Event::ConversationDeleted { origin: Origin::Cli });
    store.writer().append(tombstone).await.unwrap();
    let pending = store.readers().with(efr_store::deletes::pending).await.unwrap();
    assert_eq!(pending.len(), 1);
    store.close().await;

    let daemon = TestDaemon::builder().dirs(Arc::clone(&dirs)).persistent().start().await.unwrap();

    // The start finished the delete before its socket opened.
    for path in &gone_files {
        assert!(path.symlink_metadata().is_err(), "{} is still there", path.display());
    }
    for path in &kept_files {
        assert!(path.exists(), "{} of the other conversation went", path.display());
    }
    assert_eq!(listed(&daemon).await, [kept_one]);
    let seq = newest(&daemon, &[kept_one]).await;
    indexed(&daemon, seq).await;
    assert_eq!(found(&daemon, "rsync").await, Vec::<ConversationId>::new());
    assert_eq!(found(&daemon, "docker").await, [kept_one]);
    daemon.stop().await.unwrap();
    let (kinds, receipt, messages, segments, done) = kept(&dirs, gone, 1, Some(pty)).await;
    assert_eq!(kinds, ["conversation_deleted"]);
    assert_eq!((receipt, messages, segments, done), (false, 0, 0, true));
    let (_, receipt, ..) = kept(&dirs, kept_one, 2, None).await;
    assert!(receipt, "the other conversation keeps its receipt");
}

fn undo_applied(conversation: ConversationId, undo: UndoId) -> Event {
    Event::UndoApplied {
        conversation_id: conversation,
        undone_turn_id: "0192f0c1-7a00-7000-8000-00000000d001".parse().unwrap(),
        undo_id: undo,
        redo: false,
        origin: Origin::Cli,
        restored: 1,
        trashed: 1,
        deleted: 0,
        conflicts: 0,
        not_covered: 0,
        refs: Vec::new(),
        trash: None,
        stopped: false,
    }
}
