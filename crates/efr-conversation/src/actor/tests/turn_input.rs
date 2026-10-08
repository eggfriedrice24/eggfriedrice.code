//! The input row of a turn: late steers, the steers that a model call read, withdrawn
//! prompts and the interrupt that hands steers and prompts back. The races are made
//! with gates, not with time: a tool that never ends holds a turn in a call, and
//! `hold_end` holds a turn after it decided how it ends and before the actor knows.

use std::sync::atomic::Ordering;

use efr_protocol::{
    Event, LateSteer, Mode, Origin, ResentSteers, Seq, ShellContext, TurnId, TurnSettings,
    WithdrawTarget, WithdrawnPrompt, WithdrawnSteer,
};
use efr_provider::{Message, ProviderEvent, StopReason};
use efr_store::receipts::ReceiptOutcome;
use efr_test_support::TestRng;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::testing::{
    Harness, Setup, answer, done, expect_request, find, hold, request, result_message, text_answer,
    tool_answer, tool_message,
};
use crate::turn::STOPPED;
use crate::{ConversationError, completed_result};

const MINE: &str = "/dev/pts/1";
const OTHER: &str = "/dev/pts/2";

/// The answer of a retried command: the stored result completed from the receipt.
fn replayed(error: ConversationError, method: &str) -> Value {
    let ConversationError::DuplicateCommand { receipt } = error else {
        panic!("a retry gets the stored receipt, got {error:?}");
    };
    let ReceiptOutcome::Accepted { result } = receipt.outcome else {
        panic!("the command was accepted");
    };
    completed_result(method, result, receipt.seq.expect("the receipt has a seq"))
}

/// The turns in the order they started.
async fn started(h: &Harness) -> Vec<TurnId> {
    h.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::TurnStarted { turn_id, .. } => Some(turn_id),
            _ => None,
        })
        .collect()
}

fn unknown_turn(h: &Harness, seed: u64) -> TurnId {
    TurnId::from_uuid(efr_stdx::id::uuid_v7(&h.clock, &TestRng::new(seed)))
}

/// The review finding: a steer after `turn_interrupt_requested` was recorded and never
/// read. It is late now: refused without `if_late`, a queued prompt with it.
#[tokio::test]
async fn a_steer_after_an_interrupt_is_never_recorded_and_can_become_a_prompt() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
        expect_request(request(vec![
            Message::user("first"),
            tool_message("call_1", "hang", &json!({})),
            result_message("call_1", STOPPED, true),
            setup.prompt(&state, "in French"),
        ])),
        answer(&text_answer("En français.")),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("first").await;
    h.toolbox.hang_started.notified().await;
    let interrupt = h.interrupt_params(Some(sent.turn_id), Vec::new(), Vec::new());
    h.handle.interrupt(interrupt, Origin::Shell).await.expect("interrupt accepted");

    let plain = h.steer_params(Some(sent.turn_id), "in French", false);
    let refused = h.handle.steer(plain, Origin::Shell).await;
    let queue = h.steer_params(Some(sent.turn_id), "in French", true);
    let command_id = queue.command_id;
    let late = h.handle.steer(queue, Origin::Shell).await.expect("the late steer is queued");

    assert!(matches!(refused, Err(ConversationError::NoRunningTurn { .. })), "{refused:?}");
    assert!(late.queued);
    assert_ne!(late.turn_id, sent.turn_id, "the steer is a turn of its own");
    assert!(matches!(h.wait_end(sent.turn_id).await, Event::TurnInterrupted { .. }));
    assert!(matches!(h.wait_end(late.turn_id).await, Event::TurnCompleted { .. }));
    let envelopes = h.envelopes().await;
    assert!(!envelopes.iter().any(|e| matches!(e.event, Event::TurnSteered { .. })));
    let queued = envelopes.iter().find(|e| e.seq == late.seq).expect("the prompt's event");
    assert_eq!(
        queued.event,
        Event::PromptQueued {
            turn_id: late.turn_id,
            command_id,
            text: "in French".to_owned(),
            origin: Origin::Shell,
            context: Some(ShellContext::new(&h.cwd)),
            settings: Default::default(),
            steers: Vec::new(),
        }
    );
    h.finish();
}

/// A steer that arrives as steering closes: the model answered without a tool call and
/// the turn waits before its end. It is late, so it is never recorded as a steer; with
/// `if_late` it queues in the same step and runs after the turn.
#[tokio::test]
async fn a_steer_at_the_moment_steering_closes_queues_in_the_same_step() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hi");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hi")])),
        answer(&text_answer("Hello.")),
        expect_request(request(vec![
            Message::user("hi"),
            Message::assistant("Hello."),
            setup.prompt(&state, "and in French"),
        ])),
        answer(&text_answer("Bonjour.")),
    ];
    let mut h = setup.start(records).await;
    h.toolbox.hold_end.store(true, Ordering::SeqCst);
    let sent = h.prompt("hi").await;
    h.toolbox.end_reached.notified().await;

    let plain = h.steer_params(Some(sent.turn_id), "and in French", false);
    let refused = h.handle.steer(plain, Origin::Shell).await;
    let queue = h.steer_params(Some(sent.turn_id), "and in French", true);
    let late = h.handle.steer(queue, Origin::Shell).await.expect("the late steer is queued");
    let state = h.handle.state().await.expect("state");
    h.toolbox.hold_end.store(false, Ordering::SeqCst);
    h.toolbox.end_released.notify_one();

    assert!(matches!(refused, Err(ConversationError::NoRunningTurn { .. })), "{refused:?}");
    assert!(late.queued);
    assert_eq!(state.running, Some(sent.turn_id));
    assert_eq!(state.queued, vec![late.turn_id], "it waits behind the turn that ends");
    assert!(matches!(h.wait_end(sent.turn_id).await, Event::TurnCompleted { .. }));
    assert!(matches!(h.wait_end(late.turn_id).await, Event::TurnCompleted { .. }));
    let kinds = h.kinds().await;
    assert!(!kinds.contains(&"turn_steered".to_owned()), "{kinds:?}");
    assert_eq!(started(&h).await, vec![sent.turn_id, late.turn_id]);
    h.finish();
}

/// A steer that names a turn that ended, with no turn running, starts as a prompt at
/// once. A steer while the turn reads steering ignores `if_late`.
#[tokio::test]
async fn a_late_steer_without_a_running_turn_starts_at_once() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hi");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hi")])),
        answer(&[ProviderEvent::TextDelta { text: "Hello.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
        expect_request(request(vec![
            setup.prompt(&state, "hi"),
            Message::assistant("Hello."),
            Message::user("shorter"),
        ])),
        answer(&text_answer("Hi.")),
        expect_request(request(vec![
            Message::user("hi"),
            Message::assistant("Hello."),
            Message::user("shorter"),
            Message::assistant("Hi."),
            setup.prompt(&state, "more"),
        ])),
        answer(&text_answer("More.")),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("hi").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    let open = h.steer_params(Some(sent.turn_id), "shorter", true);
    let steered = h.handle.steer(open, Origin::Shell).await.expect("steer accepted");
    h.provider.handled_through(3);
    h.wait_end(sent.turn_id).await;

    let ended = h.steer_params(Some(sent.turn_id), "more", true);
    let late = h.handle.steer(ended, Origin::Shell).await.expect("the late steer is queued");

    assert!(!steered.queued, "a steer that a model call reads is a steer");
    assert_eq!(steered.turn_id, sent.turn_id);
    assert!(late.queued);
    assert!(matches!(h.wait_end(late.turn_id).await, Event::TurnCompleted { .. }));
    assert_eq!(started(&h).await, vec![sent.turn_id, late.turn_id]);
    h.finish();
}

#[tokio::test]
async fn a_queued_prompt_is_withdrawn_by_its_turn_or_as_the_newest_of_its_terminal() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("first").await;
    h.toolbox.hang_started.notified().await;
    let second = h.prompt_from(MINE, "second").await;
    let third = h.prompt_from(MINE, "third").await;
    let other = h.prompt_from(OTHER, "other").await;

    let by_turn = h.withdraw_params(WithdrawTarget::Turn { turn_id: second.turn_id });
    let first = h.handle.withdraw(by_turn.clone(), Origin::Shell).await.expect("withdrawn");
    let newest = h.withdraw_params(WithdrawTarget::NewestFromTty { tty: MINE.to_owned() });
    let newest = h.handle.withdraw(newest, Origin::Shell).await.expect("withdrawn");

    assert_eq!(first.withdrawn.turn_id, second.turn_id);
    assert_eq!(first.withdrawn.text, "second");
    assert_eq!(newest.withdrawn.turn_id, third.turn_id, "the newest prompt of the terminal");
    assert_eq!(h.handle.state().await.expect("state").queued, vec![other.turn_id]);
    let envelopes = h.envelopes().await;
    let recorded = envelopes.iter().find(|e| e.seq == first.withdrawn.seq).expect("event");
    assert_eq!(
        recorded.event,
        Event::PromptWithdrawn { turn_id: second.turn_id, origin: Origin::Shell }
    );

    let none_left = h.withdraw_params(WithdrawTarget::NewestFromTty { tty: MINE.to_owned() });
    let none_left = h.handle.withdraw(none_left, Origin::Shell).await;
    assert!(matches!(none_left, Err(ConversationError::NoQueuedPrompt { .. })), "{none_left:?}");
    let again = h.withdraw_params(WithdrawTarget::Turn { turn_id: second.turn_id });
    let again = h.handle.withdraw(again, Origin::Shell).await;
    assert!(matches!(again, Err(ConversationError::PromptNotWaiting { .. })), "{again:?}");
    let running = h.withdraw_params(WithdrawTarget::Turn { turn_id: sent.turn_id });
    let running = h.handle.withdraw(running, Origin::Shell).await;
    assert!(matches!(running, Err(ConversationError::PromptNotWaiting { .. })), "{running:?}");
    let unknown = unknown_turn(&h, 99);
    let unknown = h.withdraw_params(WithdrawTarget::Turn { turn_id: unknown });
    let unknown = h.handle.withdraw(unknown, Origin::Shell).await;
    assert!(matches!(unknown, Err(ConversationError::UnknownTurn { .. })), "{unknown:?}");

    // NOTE: the daemon answers a retry from the receipt before it asks the actor, which
    // would find the prompt gone.
    let command_id = by_turn.command_id;
    let receipt = h
        .store
        .readers()
        .with(move |conn| efr_store::receipts::lookup(conn, command_id))
        .await
        .expect("read")
        .expect("the withdraw has a receipt");
    assert_eq!(
        replayed(
            ConversationError::DuplicateCommand { receipt: Box::new(receipt) },
            "prompt.withdraw"
        ),
        serde_json::to_value(&first).expect("encodes"),
        "a retry answers exactly as the first time"
    );
    let withdrawn =
        h.events().await.iter().filter(|e| matches!(e, Event::PromptWithdrawn { .. })).count();
    assert_eq!(withdrawn, 2, "the refusals record nothing");
    h.handle.shutdown().await.expect("the actor stops");
}

/// The turn has decided how it ends and the actor does not know yet, so its queued
/// prompt would start as soon as it does. A withdraw now wins: the prompt never starts.
#[tokio::test]
async fn a_withdraw_at_the_moment_the_prompt_would_start_keeps_it_from_starting() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
    ];
    let mut h = setup.start(records).await;
    h.toolbox.hold_end.store(true, Ordering::SeqCst);
    let sent = h.prompt("first").await;
    let second = h.prompt("second").await;
    h.toolbox.end_reached.notified().await;

    let params = h.withdraw_params(WithdrawTarget::Turn { turn_id: second.turn_id });
    let withdrawn = h.handle.withdraw(params, Origin::Shell).await.expect("withdrawn");
    h.toolbox.hold_end.store(false, Ordering::SeqCst);
    h.toolbox.end_released.notify_one();
    h.wait_end(sent.turn_id).await;

    assert_eq!(withdrawn.withdrawn.text, "second");
    let state = h.handle.state().await.expect("state");
    assert_eq!((state.running, state.queued), (None, Vec::new()));
    assert_eq!(started(&h).await, vec![sent.turn_id], "the withdrawn prompt never started");
    h.finish();
}

/// The other order: the prompt started first, so the withdraw is refused and changes
/// nothing.
#[tokio::test]
async fn a_withdraw_after_the_prompt_started_is_refused() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&text_answer("One.")),
        expect_request(request(vec![
            Message::user("first"),
            Message::assistant("One."),
            setup.prompt(&state, "second"),
        ])),
        answer(&[]),
        hold(),
        answer(&text_answer("Two.")),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("first").await;
    let second = h.prompt("second").await;
    h.wait_for(|e| matches!(e, Event::TurnStarted { turn_id, .. } if *turn_id == second.turn_id))
        .await;

    let params = h.withdraw_params(WithdrawTarget::Turn { turn_id: second.turn_id });
    let refused = h.handle.withdraw(params, Origin::Shell).await;
    h.provider.handled_through(5);

    assert!(matches!(refused, Err(ConversationError::PromptNotWaiting { .. })), "{refused:?}");
    assert!(matches!(h.wait_end(second.turn_id).await, Event::TurnCompleted { .. }));
    assert_eq!(started(&h).await, vec![sent.turn_id, second.turn_id]);
    let kinds = h.kinds().await;
    assert!(!kinds.contains(&"prompt_withdrawn".to_owned()), "{kinds:?}");
    h.finish();
}

/// Esc with unread steers and queued prompts: one append records the interrupt, the
/// withdrawn prompt of this terminal and the steers as one new prompt, which runs
/// before the prompt of the other terminal. The other terminal's prompt stays queued.
#[tokio::test]
async fn an_interrupt_hands_back_steers_and_prompts_in_one_append() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let stopped = [
        Message::user("first"),
        tool_message("call_1", "hang", &json!({})),
        result_message("call_1", STOPPED, true),
    ];
    let mut resent_history = stopped.to_vec();
    resent_history.push(setup.prompt(&state, "steer a\nsteer b"));
    let mut other_history = stopped.to_vec();
    other_history.extend([
        Message::user("steer a\nsteer b"),
        Message::assistant("Both done."),
        setup.prompt(&state, "other terminal"),
    ]);
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
        expect_request(request(resent_history)),
        answer(&text_answer("Both done.")),
        expect_request(request(other_history)),
        answer(&text_answer("Other done.")),
    ];
    let mut h = setup.start(records).await;
    h.toolbox.hold_end.store(true, Ordering::SeqCst);
    let sent = h.prompt("first").await;
    h.toolbox.hang_started.notified().await;
    let a = h.steer_params(Some(sent.turn_id), "steer a", true);
    let a = h.handle.steer(a, Origin::Shell).await.expect("steer accepted");
    let b = h.steer_params(Some(sent.turn_id), "steer b", true);
    let b = h.handle.steer(b, Origin::Shell).await.expect("steer accepted");
    let mine = h.prompt_from(MINE, "mine queued").await;
    let other = h.prompt_from(OTHER, "other terminal").await;

    let params = h.interrupt_params(Some(sent.turn_id), vec![b.seq, a.seq], vec![mine.turn_id]);
    let command_id = params.command_id;
    let result = h.handle.interrupt(params.clone(), Origin::Shell).await.expect("interrupted");
    let state = h.handle.state().await.expect("state");
    let retried = h.handle.interrupt(params, Origin::Shell).await.expect_err("a retry");

    let resent = result.resent.clone().expect("the unread steers are sent again");
    assert_eq!(
        resent,
        ResentSteers {
            turn_id: resent.turn_id,
            seq: Seq::new(result.seq.get() + 2),
            steers: vec![a.seq, b.seq],
        }
    );
    assert_eq!(
        result.withdrawn,
        vec![WithdrawnPrompt {
            turn_id: mine.turn_id,
            seq: Seq::new(result.seq.get() + 1),
            text: "mine queued".to_owned(),
        }]
    );
    assert_eq!(state.running, Some(sent.turn_id));
    assert_eq!(state.queued, vec![resent.turn_id, other.turn_id], "the steers run next");
    assert_eq!(
        replayed(retried, "turn.interrupt"),
        serde_json::to_value(&result).expect("encodes"),
        "a retry answers exactly as the first time, sequence numbers included"
    );
    let envelopes = h.envelopes().await;
    let at = envelopes.iter().position(|e| e.seq == result.seq).expect("the request");
    let batch: Vec<Event> = envelopes[at..at + 3].iter().map(|e| e.event.clone()).collect();
    assert_eq!(
        batch,
        vec![
            Event::TurnInterruptRequested { turn_id: sent.turn_id, origin: Origin::Shell },
            Event::PromptWithdrawn { turn_id: mine.turn_id, origin: Origin::Shell },
            Event::PromptQueued {
                turn_id: resent.turn_id,
                command_id,
                text: "steer a\nsteer b".to_owned(),
                origin: Origin::Shell,
                context: Some(ShellContext::new(&h.cwd)),
                settings: Default::default(),
                steers: vec![a.seq, b.seq],
            },
        ]
    );

    h.toolbox.hold_end.store(false, Ordering::SeqCst);
    h.toolbox.end_released.notify_one();
    assert!(matches!(h.wait_end(sent.turn_id).await, Event::TurnInterrupted { .. }));
    h.wait_end(resent.turn_id).await;
    h.wait_end(other.turn_id).await;
    assert_eq!(started(&h).await, vec![sent.turn_id, resent.turn_id, other.turn_id]);
    let kinds = h.kinds().await;
    assert!(!kinds.contains(&"steering_delivered".to_owned()), "no model call read them");
    h.finish();
}

/// A steer that a model call read is not unread any more: an interrupt does not send
/// it again, and a turn it does not know is skipped.
#[tokio::test]
async fn an_interrupt_skips_steers_that_a_model_call_read() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let first = setup.prompt(&state, "first");
    let records = vec![
        expect_request(request(vec![first.clone()])),
        answer(&[ProviderEvent::TextDelta { text: "Working.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
        expect_request(request(vec![
            first,
            Message::assistant("Working."),
            Message::user("also this"),
        ])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("first").await;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;
    let steer = h.steer_params(Some(sent.turn_id), "also this", true);
    let steer = h.handle.steer(steer, Origin::Shell).await.expect("steer accepted");
    h.provider.handled_through(3);
    h.toolbox.hang_started.notified().await;

    let unknown = unknown_turn(&h, 97);
    let params = h.interrupt_params(Some(sent.turn_id), vec![steer.seq], vec![unknown]);
    let result = h.handle.interrupt(params, Origin::Shell).await.expect("interrupted");

    assert_eq!((result.resent, result.withdrawn), (None, Vec::new()));
    assert!(matches!(h.wait_end(sent.turn_id).await, Event::TurnInterrupted { .. }));
    let events = h.events().await;
    assert_eq!(
        find(&events, |e| matches!(e, Event::SteeringDelivered { .. })),
        Event::SteeringDelivered { turn_id: sent.turn_id, steers: vec![steer.seq] }
    );
    let delivered = events.iter().position(|e| matches!(e, Event::SteeringDelivered { .. }));
    let call = events.iter().position(|e| matches!(e, Event::ToolCallStarted { .. }));
    assert!(delivered < call, "recorded before the model call that reads it");
    let prompts = events.iter().filter(|e| matches!(e, Event::PromptQueued { .. })).count();
    assert_eq!(prompts, 1, "nothing was sent again");
    h.finish();
}

/// An interrupt for a turn that does not run is refused and does none of what it
/// lists: the prompt stays queued and the steer stays unread.
#[tokio::test]
async fn a_refused_interrupt_withdraws_nothing_and_resends_nothing() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
    ];
    let mut h = setup.start(records).await;
    h.toolbox.hold_end.store(true, Ordering::SeqCst);
    let sent = h.prompt("first").await;
    h.toolbox.hang_started.notified().await;
    let steer = h.steer_params(Some(sent.turn_id), "also this", true);
    let steer = h.handle.steer(steer, Origin::Shell).await.expect("steer accepted");
    let queued = h.prompt_from(MINE, "next").await;

    let other = unknown_turn(&h, 96);
    let wrong = h.interrupt_params(Some(other), vec![steer.seq], vec![queued.turn_id]);
    let refused = h.handle.interrupt(wrong, Origin::Shell).await;
    let state = h.handle.state().await.expect("state");
    let right = h.interrupt_params(Some(sent.turn_id), vec![steer.seq], Vec::new());
    let accepted = h.handle.interrupt(right, Origin::Shell).await.expect("interrupted");

    assert!(matches!(refused, Err(ConversationError::TurnMismatch { .. })), "{refused:?}");
    assert_eq!(state.queued, vec![queued.turn_id], "the prompt stays queued");
    let resent = accepted.resent.expect("the steer was still unread");
    assert_eq!(resent.steers, vec![steer.seq]);
    let requested = h
        .events()
        .await
        .iter()
        .filter(|e| matches!(e, Event::TurnInterruptRequested { .. }))
        .count();
    assert_eq!(requested, 1, "the refused interrupt recorded nothing");
    // NOTE: the turn stays held before its end, so the shutdown drops it and the
    // resent prompt never asks the model.
    h.handle.shutdown().await.expect("the actor stops");
}

/// Ctrl+C takes back the unread steers that it lists, in the append that records the
/// interrupt: `steering_withdrawn` names them, the result gives their texts, and no
/// model call reads them. A steer that Esc sends again in the same request takes the
/// values of `resend_as`, the terminal that interrupts, and is not taken back too.
#[tokio::test]
async fn an_interrupt_takes_back_unread_steers_and_resends_with_the_values_it_names() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
    ];
    let mut h = setup.start(records).await;
    h.toolbox.hold_end.store(true, Ordering::SeqCst);
    let sent = h.prompt("first").await;
    h.toolbox.hang_started.notified().await;
    let a = h.steer_params(Some(sent.turn_id), "keep it small", true);
    let a = h.handle.steer(a, Origin::Shell).await.expect("steer accepted");
    let b = h.steer_params(Some(sent.turn_id), "use tabs", true);
    let b = h.handle.steer(b, Origin::Shell).await.expect("steer accepted");

    let mut context = ShellContext::new(&h.cwd);
    context.tty = Some(OTHER.to_owned());
    let settings = TurnSettings { mode: Some(Mode::Cautious), ..TurnSettings::default() };
    let mut params = h.interrupt_params(Some(sent.turn_id), vec![b.seq], Vec::new());
    params.withdraw_steers = vec![a.seq, b.seq];
    params.resend_as = Some(Box::new(LateSteer::Queue {
        context: Some(context.clone()),
        last_command: None,
        settings: settings.clone(),
    }));
    let command_id = params.command_id;
    let result = h.handle.interrupt(params.clone(), Origin::Shell).await.expect("interrupted");
    let retried = h.handle.interrupt(params, Origin::Shell).await.expect_err("a retry");

    assert_eq!(
        result.withdrawn_steers,
        vec![WithdrawnSteer { seq: a.seq, text: "keep it small".to_owned() }],
        "only the steer that is not sent again comes back"
    );
    let resent = result.resent.clone().expect("the steer is sent again");
    assert_eq!(resent.seq, Seq::new(result.seq.get() + 2));
    assert_eq!(
        replayed(retried, "turn.interrupt"),
        serde_json::to_value(&result).expect("encodes"),
        "a retry answers exactly as the first time, sequence numbers included"
    );
    let envelopes = h.envelopes().await;
    let at = envelopes.iter().position(|e| e.seq == result.seq).expect("the request");
    let batch: Vec<Event> = envelopes[at..at + 3].iter().map(|e| e.event.clone()).collect();
    assert_eq!(
        batch,
        vec![
            Event::TurnInterruptRequested { turn_id: sent.turn_id, origin: Origin::Shell },
            Event::SteeringWithdrawn {
                turn_id: sent.turn_id,
                steers: vec![a.seq],
                origin: Origin::Shell,
            },
            Event::PromptQueued {
                turn_id: resent.turn_id,
                command_id,
                text: "use tabs".to_owned(),
                origin: Origin::Shell,
                context: Some(context),
                settings,
                steers: vec![b.seq],
            },
        ]
    );
    let page = h.envelopes().await;
    let messages = crate::exit::user_messages(&page, resent.turn_id);
    assert_eq!(messages, ["first", "use tabs"], "the steer taken back was never sent");
    let kinds = h.kinds().await;
    assert!(!kinds.contains(&"steering_delivered".to_owned()), "no model call read them");
    // NOTE: the turn stays held before its end, so the shutdown drops it and the
    // resent prompt never asks the model.
    h.handle.shutdown().await.expect("the actor stops");
}

#[test]
fn a_stored_result_gets_back_every_sequence_number_of_its_batch() {
    let turn = "019a9b1c-3d00-7a10-8b20-000000000001";
    let interrupt = json!({
        "turn_id": turn,
        "withdrawn": [{"turn_id": turn, "text": "a"}, {"turn_id": turn, "text": "b"}],
        "resent": {"turn_id": turn, "steers": [3, 4]},
    });
    let withdraw = json!({"withdrawn": {"turn_id": turn, "text": "a"}});
    let steer = json!({"turn_id": turn, "queued": true});

    assert_eq!(
        completed_result("turn.interrupt", interrupt, Seq::new(10)),
        json!({
            "turn_id": turn,
            "seq": 10,
            "withdrawn": [
                {"turn_id": turn, "seq": 11, "text": "a"},
                {"turn_id": turn, "seq": 12, "text": "b"},
            ],
            "resent": {"turn_id": turn, "seq": 13, "steers": [3, 4]},
        })
    );
    let taken = json!({
        "turn_id": turn,
        "withdrawn": [{"turn_id": turn, "text": "a"}],
        "withdrawn_steers": [{"seq": 3, "text": "c"}],
        "resent": {"turn_id": turn, "steers": [4]},
    });
    assert_eq!(
        completed_result("turn.interrupt", taken, Seq::new(10)),
        json!({
            "turn_id": turn,
            "seq": 10,
            "withdrawn": [{"turn_id": turn, "seq": 11, "text": "a"}],
            "withdrawn_steers": [{"seq": 3, "text": "c"}],
            "resent": {"turn_id": turn, "seq": 13, "steers": [4]},
        }),
        "steering_withdrawn stands before the resent prompt, and a steer keeps its own seq"
    );
    assert_eq!(
        completed_result("prompt.withdraw", withdraw, Seq::new(7)),
        json!({"withdrawn": {"turn_id": turn, "seq": 7, "text": "a"}})
    );
    assert_eq!(
        completed_result("turn.steer", steer, Seq::new(5)),
        json!({"turn_id": turn, "seq": 5, "queued": true})
    );
}
