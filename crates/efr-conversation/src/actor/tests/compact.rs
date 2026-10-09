//! Manual compaction (`conversation.compact`): it summarizes between turns with the
//! user's focus, never starts a turn, is refused while a turn runs or with nothing to
//! compact, and a prompt that arrives meanwhile waits for it.

use efr_protocol::{CommandId, Compaction, CompactionTrigger, ConversationCompact, Event};
use efr_provider::{Message, ProviderEvent, Request, StopReason};
use efr_test_support::{Record, TestRng};
use pretty_assertions::assert_eq;

use crate::ConversationError;
use crate::compaction::{
    LeftOut, dropped_note, gap_note, request_tokens, summary_message, summary_request,
};
use crate::testing::{
    Harness, MODEL, SUMMARY, Setup, TRIGGER, WINDOW, answer, compacting, compactions, done,
    expect_request, failure, fresh, hold, request, run_two_big_turns, text_answer, two_big_turns,
};

const OVERFLOW: &str = r#"{"kind": "api", "code": "context_length_exceeded", "message": "Your input exceeds the context window of this model."}"#;

const FOCUS: &str = "the failing test";

fn params(h: &mut Harness, focus: Option<&str>) -> ConversationCompact {
    ConversationCompact {
        command_id: h.command_id(),
        conversation_id: h.conversation_id,
        focus: focus.map(str::to_owned),
    }
}

/// The records of two big turns, then the summary request of a manual compaction with
/// [`FOCUS`], and the history that the next turn sends.
fn manual_records(setup: &Setup) -> (Vec<Record>, Request, Vec<Message>) {
    let (mut records, history) = two_big_turns(setup);
    records.push(expect_request(summary_request(
        &request(Vec::new()),
        history.clone(),
        Some(FOCUS),
        LeftOut::default(),
    )));
    let after =
        vec![fresh(setup), summary_message(SUMMARY), history[2].clone(), history[3].clone()];
    (records, request(history), after)
}

#[tokio::test]
async fn a_manual_compaction_summarizes_with_the_focus_and_starts_no_turn() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "one");
    let (mut records, before, after) = manual_records(&setup);
    let fresh_text = fresh(&setup).text();
    records.push(answer(&text_answer(SUMMARY)));
    let mut next = after.clone();
    next.push(setup.prompt(&state, "three"));
    records.extend([expect_request(request(next)), answer(&text_answer("ok 3"))]);
    let mut h = setup.start(records).await;
    let (one, _) = run_two_big_turns(&mut h).await;

    let params = params(&mut h, Some(FOCUS));
    let result = h.handle.compact(params).await.expect("compacted");

    let expected = Compaction {
        compaction_id: result.compaction.compaction_id,
        turn_id: None,
        trigger: CompactionTrigger::Manual,
        focus: Some(FOCUS.to_owned()),
        model: MODEL.to_owned(),
        window: WINDOW,
        limit: TRIGGER,
        tokens_before: request_tokens(&before),
        tokens_after: request_tokens(&request(after)),
        through_turn: one,
        through_message: None,
        kept_turns: 1,
        pruned_outputs: 0,
        pruned_tokens: 0,
        omitted_turns: 0,
        omitted_messages: 0,
        fresh: Some(fresh_text),
        summary: Some(SUMMARY.to_owned()),
        usage: None,
    };
    assert_eq!(result.compaction, expected);
    let envelopes = h.envelopes().await;
    let last = envelopes.last().expect("events");
    assert_eq!(last.seq, result.seq);
    assert_eq!(last.event, Event::ConversationCompacted(expected));
    let state_now = h.handle.state().await.expect("state");
    assert_eq!((state_now.running, state_now.compacting), (None, false), "no turn starts");

    let three = h.prompt("three").await.turn_id;
    let end = h.wait_end(three).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert_eq!(compactions(&h).await.len(), 1);
    h.finish();
}

#[tokio::test]
async fn a_prompt_sent_during_a_manual_compaction_waits_for_it() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "one");
    let (mut records, _, after) = manual_records(&setup);
    records.push(hold());
    let held = records.len();
    records.push(answer(&text_answer(SUMMARY)));
    let mut next = after;
    next.push(setup.prompt(&state, "three"));
    records.extend([expect_request(request(next)), answer(&text_answer("ok 3"))]);
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;

    let params = params(&mut h, Some(FOCUS));
    let handle = h.handle.clone();
    let compaction = tokio::spawn(async move { handle.compact(params).await });
    while !h.handle.state().await.expect("state").compacting {
        tokio::task::yield_now().await;
    }
    let three = h.prompt("three").await;
    assert!(three.queued, "the prompt waits for the compaction");
    let busy = h.handle.compact(ConversationCompact {
        command_id: CommandId::from_uuid(efr_stdx::id::uuid_v7(&h.clock, &TestRng::new(77))),
        conversation_id: h.conversation_id,
        focus: None,
    });
    assert!(
        matches!(busy.await, Err(ConversationError::CompactionBusy { .. })),
        "a second compaction is refused"
    );
    h.provider.handled_through(held);

    compaction.await.expect("the task").expect("compacted");
    let end = h.wait_end(three.turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}

#[tokio::test]
async fn a_retry_of_the_running_compaction_waits_for_its_answer() {
    let setup = compacting();
    let (mut records, _, _) = manual_records(&setup);
    records.push(hold());
    let held = records.len();
    records.push(answer(&text_answer(SUMMARY)));
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;

    let params = params(&mut h, Some(FOCUS));
    let handle = h.handle.clone();
    let first = tokio::spawn({
        let params = params.clone();
        async move { handle.compact(params).await }
    });
    while !h.handle.state().await.expect("state").compacting {
        tokio::task::yield_now().await;
    }
    let mut retry = Box::pin(h.handle.compact(params));
    assert!(futures::poll!(&mut retry).is_pending(), "the retry waits");
    // NOTE: the actor answers in order, so the retry has reached it after this answer.
    assert!(h.handle.state().await.expect("state").compacting);
    h.provider.handled_through(held);

    let first = first.await.expect("the task").expect("compacted");
    let retry = retry.await.expect("the same answer");

    assert_eq!(retry, first);
    assert_eq!(compactions(&h).await.len(), 1);
    h.finish();
}

#[tokio::test]
async fn a_manual_compaction_is_refused_while_a_turn_runs() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&[ProviderEvent::TextDelta { text: "One.".to_owned() }]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut h = setup.start(records).await;
    let first = h.prompt("first").await.turn_id;
    h.wait_for(|e| matches!(e, Event::AssistantMessageUpdated { .. })).await;

    let params = params(&mut h, None);
    let refused = h.handle.compact(params).await;

    assert!(matches!(refused, Err(ConversationError::CompactionBusy { .. })), "{refused:?}");
    h.provider.handled_through(3);
    h.wait_end(first).await;
    assert!(compactions(&h).await.is_empty());
    h.finish();
}

#[tokio::test]
async fn a_conversation_whose_history_fits_in_the_tail_has_nothing_to_compact() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "hello");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        answer(&text_answer("hi")),
    ];
    let mut h = setup.start(records).await;
    let hello = h.prompt("hello").await.turn_id;
    h.wait_end(hello).await;
    let before = h.events().await.len();

    let params = params(&mut h, Some(FOCUS));
    let refused = h.handle.compact(params).await;

    assert!(matches!(refused, Err(ConversationError::NothingToCompact { .. })), "{refused:?}");
    assert_eq!(h.events().await.len(), before, "nothing is recorded");
    h.finish();
}

#[tokio::test]
async fn a_summary_cut_off_at_the_output_limit_records_nothing() {
    let setup = compacting();
    let (mut records, _, _) = manual_records(&setup);
    records.push(answer(&[
        ProviderEvent::TextDelta { text: "## Task and state\nRead the big".to_owned() },
        done(StopReason::MaxTokens, None),
    ]));
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;
    let before = h.events().await.len();

    let params = params(&mut h, Some(FOCUS));
    let refused = h.handle.compact(params).await;

    assert!(matches!(refused, Err(ConversationError::IncompleteSummary)), "{refused:?}");
    assert_eq!(h.events().await.len(), before, "nothing is recorded");
    h.finish();
}

#[tokio::test]
async fn a_summary_request_that_does_not_fit_leaves_out_the_oldest_messages_and_says_so() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "one");
    let (mut records, history) = two_big_turns(&setup);
    let base = request(Vec::new());
    let mut fitting = vec![dropped_note(1)];
    fitting.extend(history[1..].iter().cloned());
    let after = vec![
        fresh(&setup),
        summary_message(SUMMARY),
        gap_note(0, 1).expect("a note"),
        history[2].clone(),
        history[3].clone(),
    ];
    let mut next = after.clone();
    next.push(setup.prompt(&state, "three"));
    records.extend([
        expect_request(summary_request(&base, history.clone(), None, LeftOut::default())),
        failure(serde_json::from_str(OVERFLOW).expect("json")),
        // The prompt names the message that the request leaves out.
        expect_request(summary_request(&base, fitting, None, LeftOut { turns: 0, messages: 1 })),
        answer(&text_answer(SUMMARY)),
        expect_request(request(next)),
        answer(&text_answer("ok 3")),
    ]);
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;

    let params = params(&mut h, None);
    let result = h.handle.compact(params).await.expect("compacted");

    assert_eq!((result.compaction.omitted_turns, result.compaction.omitted_messages), (0, 1));
    assert_eq!(result.compaction.tokens_after, request_tokens(&request(after)));
    // The next turn rebuilds the note from the compaction in the store.
    let three = h.prompt("three").await.turn_id;
    let end = h.wait_end(three).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}
