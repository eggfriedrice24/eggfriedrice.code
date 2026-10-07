use std::collections::HashMap;
use std::path::PathBuf;

use efr_protocol::{
    CallId, CommandId, ConversationId, Event, EventEnvelope, Origin, PtyId, Scope, Seq, TurnId,
};
use efr_store::Batch;
use efr_store::conversations::TurnStatus;
use efr_store::outbox::NewOutboxItem;
use efr_test_support::{TestClock, TestStore};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::reconcile::{Reconciled, STOPPED_CALL, plan, reconcile};

fn id(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

fn queued(turn: TurnId, n: u128) -> Event {
    Event::PromptQueued {
        turn_id: turn,
        command_id: CommandId::from_uuid(id(100 + n)),
        text: format!("prompt {n}"),
        origin: Origin::Shell,
        context: None,
        settings: efr_protocol::TurnSettings::default(),
    }
}

fn started(turn: TurnId) -> Event {
    Event::TurnStarted {
        turn_id: turn,
        cwd: PathBuf::from("/home/u"),
        scope: Scope::Machine,
        settings: None,
    }
}

fn call_started(turn: TurnId, call: CallId) -> Event {
    Event::ToolCallStarted {
        turn_id: turn,
        call_id: call,
        tool: "write_file".to_owned(),
        input: json!({ "path": "/etc/hosts", "content": "" }),
        manual_input: false,
        launch: None,
    }
}

fn created() -> Event {
    Event::ConversationCreated { origin: Origin::Shell, tty: None }
}

struct Scene {
    store: TestStore,
    busy: ConversationId,
    t1: TurnId,
    t2: TurnId,
    call: CallId,
    pty: PtyId,
    hwm: Seq,
}

/// A conversation that was running a turn waiting for an approval, with a second
/// prompt queued and a shell open, and a finished conversation next to it.
async fn scene() -> Scene {
    let store = TestStore::open(TestClock::new().shared()).await.unwrap();
    let busy = ConversationId::from_uuid(id(1));
    let idle = ConversationId::from_uuid(id(2));
    let (t1, t2, t3) =
        (TurnId::from_uuid(id(11)), TurnId::from_uuid(id(12)), TurnId::from_uuid(id(13)));
    let call = CallId::from_uuid(id(21));
    let done = CallId::from_uuid(id(22));
    let pty = PtyId::from_uuid(id(31));
    let batch = Batch::new()
        .event(busy, created())
        .event(busy, queued(t1, 1))
        .event(busy, started(t1))
        .event(
            busy,
            Event::ShellStarted { pty_id: pty, cwd: PathBuf::from("/home/u"), pid: Some(7) },
        )
        .event(busy, call_started(t1, done))
        .event(
            busy,
            Event::ToolCallCompleted {
                turn_id: t1,
                call_id: done,
                output: "ok".to_owned(),
                truncated: false,
                is_error: false,
                exit_code: Some(0),
                sandbox: None,
                refusal: None,
                changes: None,
                diff: None,
            },
        )
        .event(busy, call_started(t1, call))
        .event(
            busy,
            Event::ApprovalRequested {
                turn_id: t1,
                call_id: call,
                summary: "write /etc/hosts".to_owned(),
                diff_preview: None,
                interactive: false,
                exit: None,
            },
        )
        .event(busy, queued(t2, 2))
        .event(idle, created())
        .event(idle, queued(t3, 3))
        .event(idle, started(t3))
        .event(idle, Event::TurnCompleted { turn_id: t3, usage: None, changes: None })
        .enqueue(NewOutboxItem::process_bound("notify", json!({})))
        .enqueue(NewOutboxItem::replay_safe("index", json!({})));
    let hwm = store.writer().append(batch).await.unwrap().last_seq();
    Scene { store, busy, t1, t2, call, pty, hwm }
}

fn after(events: Vec<EventEnvelope>, hwm: Seq) -> Vec<(Option<ConversationId>, Event)> {
    events
        .into_iter()
        .filter(|envelope| envelope.seq > hwm)
        .map(|envelope| (envelope.conversation_id, envelope.event))
        .collect()
}

#[tokio::test]
async fn a_restart_settles_every_kind_of_work_in_flight() {
    let Scene { store, busy, t1, t2, call, pty, hwm } = scene().await;

    let notices = tempfile::tempdir().unwrap();

    let done = reconcile(store.readers(), store.writer(), notices.path()).await.unwrap();

    assert_eq!(
        done,
        Reconciled {
            turns_cancelled: 1,
            approvals_expired: 1,
            calls_closed: 1,
            prompts_not_run: 1,
            shells_exited: 1,
            outbox_cancelled: 1,
            outbox_requeued: 0,
        }
    );
    assert_eq!(
        after(store.events().await.unwrap(), hwm),
        [
            (Some(busy), Event::ApprovalExpired { turn_id: t1, call_id: call }),
            (
                Some(busy),
                Event::ToolCallCompleted {
                    turn_id: t1,
                    call_id: call,
                    output: STOPPED_CALL.to_owned(),
                    truncated: false,
                    is_error: true,
                    exit_code: None,
                    sandbox: None,
                    refusal: None,
                    changes: None,
                    diff: None,
                }
            ),
            (Some(busy), Event::TurnCancelled { turn_id: t1 }),
            (Some(busy), Event::TurnCancelled { turn_id: t2 }),
            (Some(busy), Event::ShellExited { pty_id: pty, exit_code: None }),
        ]
    );
    let (turns, approvals, shells) = store
        .readers()
        .with(|conn| {
            Ok((
                efr_store::conversations::unfinished_turns(conn)?,
                efr_store::approvals::pending(conn, None)?,
                efr_store::shells::running(conn)?,
            ))
        })
        .await
        .unwrap();
    assert!(turns.is_empty(), "nothing waits to run: {turns:?}");
    assert!(approvals.is_empty());
    assert!(shells.is_empty());
    assert_eq!(
        std::fs::read_dir(notices.path()).unwrap().count(),
        0,
        "a conversation without a terminal gets no notice"
    );
    store.close().await;
}

#[tokio::test]
async fn a_second_restart_finds_nothing_left_to_settle() {
    let Scene { store, .. } = scene().await;
    let notices = tempfile::tempdir().unwrap();
    reconcile(store.readers(), store.writer(), notices.path()).await.unwrap();
    let hwm = store.writer().append(Batch::new()).await.unwrap().last_seq();

    let done = reconcile(store.readers(), store.writer(), notices.path()).await.unwrap();

    assert_eq!(done, Reconciled::default());
    assert_eq!(after(store.events().await.unwrap(), hwm), []);
    store.close().await;
}

#[tokio::test]
async fn the_plan_leaves_finished_turns_alone() {
    let Scene { store, busy, .. } = scene().await;
    let notices = tempfile::tempdir().unwrap();
    reconcile(store.readers(), store.writer(), notices.path()).await.unwrap();
    let (turns, approvals, shells) = store
        .readers()
        .with(move |conn| {
            Ok((
                efr_store::conversations::turns(conn, busy)?,
                efr_store::approvals::pending(conn, None)?,
                efr_store::shells::running(conn)?,
            ))
        })
        .await
        .unwrap();

    assert_eq!(turns.len(), 2, "the cancelled turn and the prompt that did not run");
    assert!(turns.iter().all(|turn| turn.status == TurnStatus::Cancelled), "{turns:?}");
    assert!(plan(&turns, &approvals, &HashMap::new(), &shells).is_empty());
    store.close().await;
}

/// A terminal's conversation with a running turn, a prompt queued behind it, and a
/// prompt that an earlier daemon held.
async fn terminal_scene() -> (TestStore, ConversationId, [TurnId; 3], Seq) {
    let store = TestStore::open(TestClock::new().shared()).await.unwrap();
    let tty = ConversationId::from_uuid(id(3));
    let (running, waiting, held) =
        (TurnId::from_uuid(id(41)), TurnId::from_uuid(id(42)), TurnId::from_uuid(id(43)));
    let long = format!("deploy\nthe {} now", "x".repeat(80));
    let created =
        Event::ConversationCreated { origin: Origin::Shell, tty: Some("/dev/pts/7".to_owned()) };
    let batch = Batch::new()
        .event(tty, created)
        .event(tty, queued(held, 3))
        .event(tty, Event::PromptHeld { turn_id: held })
        .event(tty, queued(running, 1))
        .event(tty, started(running))
        .event(
            tty,
            Event::PromptQueued {
                turn_id: waiting,
                command_id: CommandId::from_uuid(id(150)),
                text: long,
                origin: Origin::Shell,
                context: None,
                settings: efr_protocol::TurnSettings::default(),
            },
        );
    let hwm = store.writer().append(batch).await.unwrap().last_seq();
    (store, tty, [running, waiting, held], hwm)
}

#[tokio::test]
async fn a_waiting_prompt_is_recorded_as_not_run_and_its_terminal_told_to_send_it_again() {
    let (store, tty, [running, waiting, held], hwm) = terminal_scene().await;
    let notices = tempfile::tempdir().unwrap();

    let done = reconcile(store.readers(), store.writer(), notices.path()).await.unwrap();

    assert_eq!((done.turns_cancelled, done.prompts_not_run), (1, 2));
    assert_eq!(
        after(store.events().await.unwrap(), hwm),
        [
            (Some(tty), Event::TurnCancelled { turn_id: running }),
            (Some(tty), Event::TurnCancelled { turn_id: held }),
            (Some(tty), Event::TurnCancelled { turn_id: waiting }),
        ],
        "a prompt that an earlier daemon held is settled too"
    );
    let text = std::fs::read_to_string(notices.path().join("pts-7")).unwrap();
    // One line for the conversation, which names it and not the prompts: the terminal
    // name may belong to another terminal by now.
    assert_eq!(
        text,
        format!(
            "efr restarted; 2 queued prompts did not run; see them with efr history {tty} and send them again\n"
        )
    );
    assert!(!text.contains("deploy") && !text.contains("prompt 3"), "{text}");
    store.close().await;
}

#[tokio::test]
async fn a_prompt_that_did_not_run_is_reported_once_and_never_starts() {
    let (store, _, _, _) = terminal_scene().await;
    let notices = tempfile::tempdir().unwrap();
    reconcile(store.readers(), store.writer(), notices.path()).await.unwrap();
    let before = std::fs::read_to_string(notices.path().join("pts-7")).unwrap();

    let again = reconcile(store.readers(), store.writer(), notices.path()).await.unwrap();

    assert_eq!(again, Reconciled::default());
    let turns = store.readers().with(efr_store::conversations::unfinished_turns).await.unwrap();
    assert!(turns.is_empty(), "{turns:?}");
    assert_eq!(std::fs::read_to_string(notices.path().join("pts-7")).unwrap(), before);
    store.close().await;
}
