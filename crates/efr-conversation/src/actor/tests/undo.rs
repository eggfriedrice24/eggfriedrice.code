//! An undo of a turn (`turn.undo`): it runs only while nothing else runs in the
//! conversation, a prompt waits for it, and only an applied undo is recorded, once.

use std::path::PathBuf;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use efr_protocol::{
    Event, Origin, TurnId, TurnUndo, TurnUndoResult, UndoAction, UndoEntry, UndoReason,
};
use efr_provider::{Message, ProviderEvent, StopReason};
use efr_store::receipts::ReceiptOutcome;
use pretty_assertions::assert_eq;

use crate::testing::{Harness, Setup, answer, done, expect_request, hold, request, text_answer};
use crate::{ConversationError, UndoBusyReason};

fn params(h: &mut Harness) -> TurnUndo {
    TurnUndo {
        command_id: h.command_id(),
        conversation_id: Some(h.conversation_id),
        turn_id: None,
        dry_run: false,
        force: false,
        redo: false,
        list: false,
    }
}

fn entry(action: UndoAction, path: &str, reason: Option<UndoReason>) -> UndoEntry {
    UndoEntry { action, path: path.to_owned(), reason }
}

/// What the fake toolbox answers for an undo of `turn_id`.
fn undone(h: &Harness, turn_id: TurnId) -> TurnUndoResult {
    TurnUndoResult {
        conversation_id: h.conversation_id,
        turn_id: Some(turn_id),
        title: Some("first".to_owned()),
        undo_id: None,
        applied: false,
        stopped: false,
        resumed: false,
        entries: vec![
            entry(UndoAction::Restore, "src/a.rs", None),
            entry(UndoAction::Trash, "notes.md", None),
            entry(UndoAction::Skip, "b.rs", Some(UndoReason::ChangedAfterTurn)),
            entry(UndoAction::NotCovered, "big.bin", Some(UndoReason::LeftOut)),
        ],
        entries_more: 0,
        refs: Vec::new(),
        trash: Some(PathBuf::from("/data/efr/undo-trash/u-1")),
        notes: Vec::new(),
        turns: Vec::new(),
    }
}

fn applied(events: &[Event]) -> Vec<Event> {
    events.iter().filter(|event| matches!(event, Event::UndoApplied { .. })).cloned().collect()
}

/// Runs one turn `first` that answers `One.`, and returns its id.
async fn first_turn(h: &mut Harness) -> TurnId {
    let first = h.prompt("first").await;
    h.wait_end(first.turn_id).await;
    first.turn_id
}

#[tokio::test]
async fn undo_refuses_while_turn_runs() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[ProviderEvent::TextDelta { text: "One.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut h = setup.start(records).await;
    let first = h.prompt("first").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;

    let params = params(&mut h);
    let refused = h.handle.undo(params, Vec::new(), Origin::Shell).await;

    assert!(
        matches!(
            refused,
            Err(ConversationError::UndoBusy { reason: UndoBusyReason::Turn, conversation_id })
                if conversation_id == h.conversation_id
        ),
        "{refused:?}"
    );
    assert!(h.toolbox.undos.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
    h.provider.handled_through(3);
    h.wait_end(first.turn_id).await;
    assert!(applied(&h.events().await).is_empty());
    h.finish();
}

#[tokio::test]
async fn undo_refuses_while_a_call_runs_past_its_timeout() {
    let setup = Setup::new();
    let mut h = setup.start(Vec::new()).await;
    h.toolbox.call_runs.store(true, Ordering::SeqCst);

    let params = params(&mut h);
    let refused = h.handle.undo(params, Vec::new(), Origin::Shell).await;

    assert!(
        matches!(
            refused,
            Err(ConversationError::UndoBusy { reason: UndoBusyReason::CallRuns, .. })
        ),
        "{refused:?}"
    );
    assert!(h.toolbox.undos.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
    h.finish();
}

#[tokio::test]
async fn undo_queues_a_prompt_until_it_ends() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
        expect_request(request(vec![
            setup.prompt(&state, "first"),
            Message::assistant("One."),
            setup.prompt(&state, "second"),
        ])),
        answer(&text_answer("Two.")),
    ];
    let mut h = setup.start(records).await;
    let first = first_turn(&mut h).await;
    *h.toolbox.undo_result.lock().unwrap_or_else(PoisonError::into_inner) = Some(undone(&h, first));
    h.toolbox.hold_undo.store(true, Ordering::SeqCst);

    let handle = h.handle.clone();
    let params = params(&mut h);
    let undo = tokio::spawn(async move { handle.undo(params, Vec::new(), Origin::Shell).await });
    h.toolbox.undo_reached.notified().await;
    let second = h.prompt("second").await;

    assert!(second.queued, "a prompt sent while an undo runs waits behind it");
    let state = h.handle.state().await.expect("state");
    assert_eq!(state.running, None);
    assert!(state.undoing);
    assert_eq!(state.queued, vec![second.turn_id]);

    h.toolbox.undo_released.notify_one();
    let result = undo.await.expect("the undo's task").expect("undone");
    assert!(result.applied);
    h.wait_end(second.turn_id).await;
    let events = h.events().await;
    let undone_at = events.iter().position(|e| matches!(e, Event::UndoApplied { .. }));
    let started_at = events.iter().position(
        |e| matches!(e, Event::TurnStarted { turn_id, .. } if *turn_id == second.turn_id),
    );
    assert!(undone_at.is_some() && undone_at < started_at, "{events:?}");
    h.finish();
}

#[tokio::test]
async fn undo_applied_recorded_once_per_apply() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;
    let first = first_turn(&mut h).await;
    *h.toolbox.undo_result.lock().unwrap_or_else(PoisonError::into_inner) = Some(undone(&h, first));

    let dry_run = TurnUndo { dry_run: true, ..params(&mut h) };
    let preview = h.handle.undo(dry_run, Vec::new(), Origin::Cli).await.expect("previewed");
    assert!(!preview.applied);
    assert!(applied(&h.events().await).is_empty(), "a dry run records nothing");
    let list = TurnUndo { list: true, ..params(&mut h) };
    h.handle.undo(list, Vec::new(), Origin::Cli).await.expect("listed");
    assert!(applied(&h.events().await).is_empty(), "a list records nothing");

    h.toolbox.hold_undo.store(true, Ordering::SeqCst);
    let apply = params(&mut h);
    let (first_handle, retry_handle) = (h.handle.clone(), h.handle.clone());
    let (once, again) = (apply.clone(), apply.clone());
    let undo = tokio::spawn(async move { first_handle.undo(once, Vec::new(), Origin::Cli).await });
    h.toolbox.undo_reached.notified().await;
    let retry =
        tokio::spawn(async move { retry_handle.undo(again, Vec::new(), Origin::Cli).await });
    // NOTE: one thread: the retry reaches the actor while the undo waits.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    h.toolbox.undo_released.notify_one();
    let result = undo.await.expect("the undo's task").expect("undone");
    let retried = retry.await.expect("the retry's task").expect("the same answer");

    assert_eq!(retried, result, "a retry while the undo runs gets its answer");
    assert_eq!(h.toolbox.undos.lock().unwrap_or_else(PoisonError::into_inner).len(), 3);
    let undo_id = result.undo_id.expect("an applied undo has an id");
    assert_eq!(
        applied(&h.events().await),
        [Event::UndoApplied {
            conversation_id: h.conversation_id,
            undone_turn_id: first,
            undo_id,
            redo: false,
            origin: Origin::Cli,
            restored: 1,
            trashed: 1,
            deleted: 0,
            conflicts: 1,
            not_covered: 1,
            refs: Vec::new(),
            trash: Some(PathBuf::from("/data/efr/undo-trash/u-1")),
            stopped: false,
        }]
    );
    let command_id = apply.command_id;
    let receipt = h
        .store
        .readers()
        .with(move |conn| efr_store::receipts::lookup(conn, command_id))
        .await
        .expect("read")
        .expect("the apply has a receipt");
    assert_eq!(receipt.method, "turn.undo");
    let ReceiptOutcome::Accepted { result: stored } = receipt.outcome else {
        panic!("the undo was accepted");
    };
    let stored = crate::completed_result("turn.undo", stored, receipt.seq.expect("seq"));
    assert_eq!(serde_json::from_value::<TurnUndoResult>(stored).expect("result"), result);
    h.finish();
}

#[tokio::test]
async fn stopped_undo_is_recorded() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;
    let first = first_turn(&mut h).await;
    let stopped = TurnUndoResult { stopped: true, ..undone(&h, first) };
    *h.toolbox.undo_result.lock().unwrap_or_else(PoisonError::into_inner) = Some(stopped);

    let apply = params(&mut h);
    let result = h.handle.undo(apply, Vec::new(), Origin::Cli).await.expect("ran");

    assert!(result.stopped && !result.applied, "{result:?}");
    let recorded = applied(&h.events().await);
    assert!(
        matches!(recorded.as_slice(), [Event::UndoApplied { stopped: true, restored: 1, .. }]),
        "a write error after some changes still leaves a trace: {recorded:?}"
    );
    h.finish();
}

#[tokio::test]
async fn a_phone_cannot_undo() {
    let setup = Setup::new();
    let mut h = setup.start(Vec::new()).await;
    *h.toolbox.undo_result.lock().unwrap_or_else(PoisonError::into_inner) = Some(undone(
        &h,
        TurnId::from_uuid(efr_stdx::id::uuid_v7(
            &*h.clock.shared(),
            &efr_test_support::TestRng::new(1),
        )),
    ));

    let params = params(&mut h);
    let refused = h.handle.undo(params, Vec::new(), Origin::Phone).await;

    assert!(
        matches!(
            refused,
            Err(ConversationError::UndoRefused(efr_protocol::UndoRefused::PhoneOrigin))
        ),
        "{refused:?}"
    );
    assert!(h.toolbox.undos.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
    h.finish();
}

#[tokio::test]
async fn undo_refuses_while_a_job_runs() {
    let setup = Setup::new();
    let mut h = setup.start(Vec::new()).await;
    let reason = UndoBusyReason::Job { number: 3, same_conversation: true };
    *h.toolbox.blocks_undo.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
    let turn_id: TurnId = "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap();
    let candidate = crate::UndoCandidate {
        turn_id,
        title: Some("first".to_owned()),
        ended: None,
        stopped: false,
        wake: false,
    };

    let undo = params(&mut h);
    let refused = h.handle.undo(undo, vec![candidate.clone()], Origin::Shell).await;
    assert!(
        matches!(refused, Err(ConversationError::UndoBusy { reason: r, .. }) if r == reason),
        "{refused:?}"
    );
    assert!(h.toolbox.undos.lock().unwrap_or_else(PoisonError::into_inner).is_empty());

    // A list only reads, so a job never keeps it from running.
    let mut list = params(&mut h);
    list.list = true;
    let listed = h.handle.undo(list, vec![candidate], Origin::Shell).await;
    assert!(!matches!(listed, Err(ConversationError::UndoBusy { .. })), "{listed:?}");
    h.finish();
}
