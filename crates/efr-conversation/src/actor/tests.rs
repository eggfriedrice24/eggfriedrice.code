use efr_protocol::{
    ApprovalDecision, ApprovalRespond, ConversationId, Event, Origin, Seq, TurnInterrupt, TurnSteer,
};
use efr_provider::{Message, ProviderEvent, StopReason};
use efr_store::receipts::ReceiptOutcome;
use efr_test_support::TestRng;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::testing::{
    Setup, answer, done, expect_request, hold, request, text_answer, tool_answer,
};
use crate::{ConversationError, ConversationStart};

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
            Message::user("first"),
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
    };
    let steered = h.handle.steer(steer).await;
    assert!(matches!(steered, Err(ConversationError::NoRunningTurn { .. })), "{steered:?}");
    let interrupt = TurnInterrupt {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        turn_id: None,
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
    };
    let wrong = h.handle.steer(steer).await;
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
