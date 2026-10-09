use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ConversationId, Event, Origin, Seq, TurnInterrupt, TurnSteer,
};
use efr_provider::{Message, ProviderEvent, StopReason};
use efr_store::receipts::ReceiptOutcome;
use efr_test_support::TestRng;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{Dispatch, Subscriber, span};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;

use crate::testing::{
    Setup, answer, done, expect_request, hold, request, text_answer, tool_answer,
};
use crate::{ConversationError, ConversationStart};

mod compact;
mod turn_input;

#[tokio::test]
async fn a_second_prompt_queues_behind_the_running_turn() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[ProviderEvent::TextDelta { text: "One.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
        expect_request(request(vec![
            setup.prompt(&state, "first"),
            Message::assistant("One."),
            setup.prompt(&state, "second"),
        ])),
        answer(&text_answer("Two.")),
    ];
    let mut h = setup.start(records).await;

    let first = h.prompt("first").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    let second = h.prompt("second").await;
    assert!(!first.queued);
    assert!(second.queued, "a prompt sent while a turn runs waits behind it");
    let state = h.handle.state().await.expect("state");
    assert_eq!(state.running, Some(first.turn_id));
    assert_eq!(state.queued, vec![second.turn_id]);

    h.provider.handled_through(3);
    h.wait_end(first.turn_id).await;
    h.wait_end(second.turn_id).await;

    let started: Vec<_> = h
        .events()
        .await
        .into_iter()
        .filter_map(|e| match e {
            Event::TurnStarted { turn_id, .. } => Some(turn_id),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec![first.turn_id, second.turn_id], "one turn at a time, in order");
    h.finish();
}

#[tokio::test]
async fn a_full_queue_refuses_a_prompt_without_recording_it() {
    let mut setup = Setup::new();
    setup.config.max_queued = 1;
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[]),
        hold(),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;

    let first = h.prompt("first").await;
    h.wait_for(|e| matches!(e, Event::TurnStarted { .. })).await;
    h.prompt("second").await;
    let cwd = h.cwd.clone();
    let third = h.prompt_params(&cwd, "third");
    let refused = h.handle.send_prompt(third, Origin::Shell).await;

    assert!(matches!(refused, Err(ConversationError::QueueFull { limit: 1, .. })), "{refused:?}");
    let queued =
        h.events().await.iter().filter(|e| matches!(e, Event::PromptQueued { .. })).count();
    assert_eq!(queued, 2);
    assert_eq!(first.seq, Seq::new(2));
    h.handle.shutdown().await.expect("the actor stops");
}

#[tokio::test]
async fn the_queue_limit_is_read_when_each_prompt_arrives() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[]),
        hold(),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;

    h.prompt("first").await;
    h.wait_for(|e| matches!(e, Event::TurnStarted { .. })).await;
    h.prompt("second").await;
    let mut smaller = (**h.settings.borrow()).clone();
    smaller.max_queued = 1;
    h.settings.send_replace(Arc::new(smaller));
    let cwd = h.cwd.clone();
    let third = h.prompt_params(&cwd, "third");
    let refused = h.handle.send_prompt(third, Origin::Shell).await;

    assert!(matches!(refused, Err(ConversationError::QueueFull { limit: 1, .. })), "{refused:?}");
    h.handle.shutdown().await.expect("the actor stops");
}

#[tokio::test]
async fn a_new_conversation_is_recorded_once_with_its_first_prompt() {
    let mut setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
    ];
    setup.config.max_queued = 4;
    let tty = Some("/dev/pts/3".to_owned());
    let start = ConversationStart::New { origin: Origin::Cli, tty: tty.clone() };
    let mut h = setup.start_with(records, start, None).await;

    let first = h.prompt("first").await;
    h.wait_end(first.turn_id).await;

    let events = h.events().await;
    assert_eq!(events[0], Event::ConversationCreated { origin: Origin::Cli, tty });
    let created = events.iter().filter(|e| matches!(e, Event::ConversationCreated { .. })).count();
    assert_eq!(created, 1);
    h.finish();
}

#[tokio::test]
async fn a_retried_command_returns_its_receipt_instead_of_running_again() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;

    let cwd = h.cwd.clone();
    let params = h.prompt_params(&cwd, "first");
    let first = h.handle.send_prompt(params.clone(), Origin::Shell).await.expect("accepted");
    h.wait_end(first.turn_id).await;
    let retried = h.handle.send_prompt(params, Origin::Shell).await;

    let Err(ConversationError::DuplicateCommand { receipt }) = retried else {
        panic!("a retry gets the stored receipt, got {retried:?}");
    };
    assert_eq!(receipt.method, "prompt.send");
    assert_eq!(receipt.seq, Some(first.seq), "the receipt records the prompt_queued event");
    let ReceiptOutcome::Accepted { result } = receipt.outcome else {
        panic!("the prompt was accepted");
    };
    assert_eq!(
        result,
        json!({
            "conversation_id": h.conversation_id,
            "turn_id": first.turn_id,
            "queued": false,
            "settings": { "mode": "cautious", "model": "test-model" },
        }),
        "the result is stored without its seq"
    );
    let queued =
        h.events().await.iter().filter(|e| matches!(e, Event::PromptQueued { .. })).count();
    assert_eq!(queued, 1);
    h.finish();
}

#[tokio::test]
async fn the_last_command_reaches_the_preamble_but_never_an_event() {
    let setup = Setup::new();
    let mut state = setup.live_state(&setup.cwd, "why did it fail");
    state.last_command = Some("export TOKEN=hunter2 && make".to_owned());
    state.last_status = Some(2);
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "why did it fail")])),
        answer(&text_answer("The token is wrong.")),
    ];
    let mut h = setup.start(records).await;

    let cwd = h.cwd.clone();
    let mut params = h.prompt_params(&cwd, "why did it fail");
    params.last_command = Some("export TOKEN=hunter2 && make".to_owned());
    if let Some(context) = params.context.as_mut() {
        context.last_status = Some(2);
    }
    let sent = h.handle.send_prompt(params, Origin::Shell).await.expect("accepted");
    h.wait_end(sent.turn_id).await;

    let log = serde_json::to_string(&h.events().await).expect("events serialize");
    assert!(!log.contains("hunter2"), "the last command never enters the log");
    h.finish();
}

#[tokio::test]
async fn steering_or_interrupting_without_a_running_turn_is_refused() {
    let setup = Setup::new();
    let mut h = setup.start(Vec::new()).await;

    let steer = TurnSteer {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: None,
        text: "more".to_owned(),
        if_late: None,
    };
    let steered = h.handle.steer(steer, Origin::Shell).await;
    assert!(matches!(steered, Err(ConversationError::NoRunningTurn { .. })), "{steered:?}");
    let interrupt = TurnInterrupt {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: None,
        resend_steers: Vec::new(),
        resend_as: None,
        withdraw_steers: Vec::new(),
        withdraw: Vec::new(),
    };
    let interrupted = h.handle.interrupt(interrupt, Origin::Shell).await;
    assert!(matches!(interrupted, Err(ConversationError::NoRunningTurn { .. })));
    assert!(h.events().await.is_empty(), "nothing is recorded");
}

#[tokio::test]
async fn a_request_for_another_turn_or_conversation_is_refused() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[]),
        hold(),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;
    let first = h.prompt("first").await;
    h.wait_for(|e| matches!(e, Event::TurnStarted { .. })).await;

    let other_turn =
        efr_protocol::TurnId::from_uuid(efr_stdx::id::uuid_v7(&h.clock, &TestRng::new(99)));
    let interrupt = TurnInterrupt {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: Some(other_turn),
        resend_steers: Vec::new(),
        resend_as: None,
        withdraw_steers: Vec::new(),
        withdraw: Vec::new(),
    };
    let mismatch = h.handle.interrupt(interrupt, Origin::Shell).await;
    assert!(
        matches!(mismatch, Err(ConversationError::TurnMismatch { running, requested })
            if running == first.turn_id && requested == other_turn),
        "{mismatch:?}"
    );

    let other = ConversationId::from_uuid(efr_stdx::id::uuid_v7(&h.clock, &TestRng::new(98)));
    let steer = TurnSteer {
        command_id: h.command_id(),
        conversation_id: other,
        turn_id: None,
        text: "more".to_owned(),
        if_late: None,
    };
    let wrong = h.handle.steer(steer, Origin::Shell).await;
    assert!(matches!(wrong, Err(ConversationError::WrongConversation { .. })), "{wrong:?}");
    h.handle.shutdown().await.expect("the actor stops");
}

#[tokio::test]
async fn an_answer_for_a_call_that_does_not_wait_is_refused() {
    let setup = Setup::new();
    let mut h = setup.start(Vec::new()).await;

    let call_id =
        efr_protocol::CallId::from_uuid(efr_stdx::id::uuid_v7(&h.clock, &TestRng::new(97)));
    let params = ApprovalRespond {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        call_id,
        decision: ApprovalDecision::Allow,
    };
    let answered = h.handle.respond_approval(params, Origin::Shell).await;

    assert!(
        matches!(answered, Err(ConversationError::ApprovalNotPending { call_id: id }) if id == call_id),
        "{answered:?}"
    );
    assert!(h.events().await.is_empty());
}

#[tokio::test]
async fn shutdown_stops_the_actor_and_later_requests_fail() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "wait");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "wait")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
    ];
    let mut h = setup.start(records).await;
    h.prompt("wait").await;
    h.toolbox.hang_started.notified().await;

    h.handle.shutdown().await.expect("the actor stops");

    assert!(h.handle.is_closed());
    assert!(matches!(h.handle.state().await, Err(ConversationError::Stopped)));
    let ended = h
        .events()
        .await
        .iter()
        .any(|e| matches!(e, Event::TurnCompleted { .. } | Event::TurnInterrupted { .. }));
    assert!(!ended, "the dropped turn is left for the reconciliation at the next start");
}

#[tokio::test]
async fn queued_turns_finish_after_every_handle_is_dropped() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;
    let first = h.prompt("first").await;
    let (handle, store, provider, _dirs) = h.into_parts();
    let mut events = store.writer().subscribe();
    drop(handle);

    let completed = |event: &Event| matches!(event, Event::TurnCompleted { turn_id, .. } if *turn_id == first.turn_id);
    let logged = store.events().await.expect("events");
    if !logged.iter().any(|envelope| completed(&envelope.event)) {
        loop {
            let committed = events.recv().await.expect("the store broadcasts");
            if committed.events().iter().any(|envelope| completed(&envelope.event)) {
                break;
            }
        }
    }
    provider.finish().expect("the replay went as the transcript says");
}

#[tokio::test]
async fn a_steer_after_the_last_model_call_is_refused_and_never_recorded() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hi");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hi")])),
        answer(&text_answer("Hello.")),
    ];
    let mut h = setup.start(records).await;
    h.toolbox.hold_end.store(true, Ordering::SeqCst);
    let sent = h.prompt("hi").await;
    // The model answered without a tool call: the turn has made its last model call
    // and waits in `turn_changes` before its end.
    h.toolbox.end_reached.notified().await;

    let steer = TurnSteer {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: Some(sent.turn_id),
        text: "and in French".to_owned(),
        if_late: None,
    };
    let steered = h.handle.steer(steer, Origin::Shell).await;
    h.toolbox.end_released.notify_one();

    assert!(matches!(steered, Err(ConversationError::NoRunningTurn { .. })), "{steered:?}");
    assert!(matches!(h.wait_end(sent.turn_id).await, Event::TurnCompleted { .. }));
    let steered = h.events().await.iter().any(|e| matches!(e, Event::TurnSteered { .. }));
    assert!(!steered, "no model call reads it, so it is not recorded");
    h.finish();
}

/// Holds the thread that closes the span of a turn until the test releases it. The
/// turn's task has done all its work then, but it has not finished, so the actor
/// cannot know yet that it ended.
struct HoldTurnClose {
    closing: mpsc::UnboundedSender<()>,
    release: Arc<Release>,
}

#[derive(Default)]
struct Release {
    released: Mutex<bool>,
    changed: Condvar,
}

impl Release {
    fn open(&self) {
        *self.released.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut released = self.released.lock().unwrap_or_else(PoisonError::into_inner);
        while !*released {
            released = self.changed.wait(released).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Opens the release when the test ends, also when it fails, so no worker stays held.
struct OpenOnDrop(Arc<Release>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

impl<S> Layer<S> for HoldTurnClose
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let is_turn = ctx.span(&id).is_some_and(|span| {
            span.name() == "turn" && span.metadata().target().starts_with("efr_conversation")
        });
        if is_turn {
            let _ = self.closing.send(());
            self.release.wait();
        }
    }
}

/// A client that sees the end of a turn and at once steers, interrupts or sends a
/// prompt must find no running turn. The turn's task is held after all its work, so
/// an end that is sent before the actor clears the running turn is seen here every
/// time, with no help from timing.
#[test]
fn a_client_that_sees_the_end_of_a_turn_finds_no_running_turn() {
    let (closing_tx, mut closing) = mpsc::unbounded_channel();
    let release = Arc::new(Release::default());
    let layer = HoldTurnClose { closing: closing_tx, release: Arc::clone(&release) };
    let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .on_thread_start(move || {
            // NOTE: the guard is forgotten so the subscriber stays on this worker for
            // as long as the worker runs.
            std::mem::forget(tracing::dispatcher::set_default(&dispatch));
        })
        .build()
        .expect("runtime");
    // NOTE: made after the runtime so it drops first: a failed check opens the release
    // before the runtime waits for its workers.
    let _open = OpenOnDrop(Arc::clone(&release));

    runtime.block_on(async move {
        let setup = Setup::new();
        let state = setup.live_state(&setup.cwd, "hi");
        let records = vec![
            expect_request(request(vec![setup.prompt(&state, "hi")])),
            answer(&text_answer("Hello.")),
            expect_request(request(vec![
                setup.prompt(&state, "hi"),
                Message::assistant("Hello."),
                setup.prompt(&state, "again"),
            ])),
            answer(&text_answer("Hello again.")),
        ];
        let mut h = setup.start(records).await;
        let first = h.prompt("hi").await;
        closing.recv().await.expect("the turn's span closes");

        // NOTE: the end must not be out while the turn's task is held. If it is, the
        // checks below run while the actor still counts the turn as running.
        if !h.has_ended(first.turn_id) {
            release.open();
            h.wait_end(first.turn_id).await;
        }

        let steer = TurnSteer {
            command_id: h.command_id(),
            conversation_id: h.conversation_id,
            turn_id: None,
            text: "more".to_owned(),
            if_late: None,
        };
        let steered = h.handle.steer(steer, Origin::Shell).await;
        assert!(matches!(steered, Err(ConversationError::NoRunningTurn { .. })), "{steered:?}");
        let interrupt = TurnInterrupt {
            command_id: h.command_id(),
            conversation_id: h.conversation_id,
            turn_id: None,
            resend_steers: Vec::new(),
            resend_as: None,
            withdraw_steers: Vec::new(),
            withdraw: Vec::new(),
        };
        let interrupted = h.handle.interrupt(interrupt, Origin::Shell).await;
        assert!(
            matches!(interrupted, Err(ConversationError::NoRunningTurn { .. })),
            "{interrupted:?}"
        );
        assert_eq!(h.handle.state().await.expect("state").running, None);
        let second = h.prompt("again").await;
        assert!(!second.queued, "the prompt starts at once");

        release.open();
        h.wait_end(second.turn_id).await;
        let kinds = h.kinds().await;
        let late = ["turn_steered", "turn_interrupt_requested"];
        assert!(!kinds.iter().any(|kind| late.contains(&kind.as_str())), "{kinds:?}");
        h.finish();
    });
}
