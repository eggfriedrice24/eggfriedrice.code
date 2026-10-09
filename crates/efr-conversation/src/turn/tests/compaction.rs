//! Compaction inside a turn: auto at the trigger, after an overflow, the breaker, a
//! pruning alone, and the history that later turns send, also after a restart.

use efr_protocol::{Compaction, CompactionTrigger, DraftPart, ErrorCode, Event, TurnId};
use efr_provider::{Message, Request};
use efr_store::turn_messages;
use efr_test_support::Record;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::compaction::{dropped_note, gap_note, request_tokens, summary_message};
use crate::context::{PRUNED_OUTPUT_STUB, kilo};
use crate::testing::{
    HARD_CAP, MODEL, SUMMARY, SUMMARY_2, Setup, TRIGGER, WINDOW, answer, big_file, big_text,
    compacting, compactions, expect_request, failure, fresh, request, result_message,
    run_two_big_turns, summary, text_answer, tool_answer, tool_message, two_big_turns,
};
use crate::{CompactionConfig, ContextLimits};

/// `request` as JSON without its tools, with the test's root directory as `<root>` and
/// each string longer than 200 bytes replaced by its size, so a snapshot shows the order
/// and the shape of the messages.
fn shape(request: &Request, root: &std::path::Path) -> String {
    fn shorten(value: &mut Value) {
        match value {
            Value::String(text) if text.len() > 200 => {
                *text = format!("<{} bytes, starts {:?}>", text.len(), &text[..24]);
            }
            Value::Array(items) => items.iter_mut().for_each(shorten),
            Value::Object(members) => members.values_mut().for_each(shorten),
            _ => {}
        }
    }
    let text = serde_json::to_string(request).expect("a request serializes");
    let root = root.to_str().expect("a UTF-8 root");
    let text = text.replace(root, "<root>");
    let mut value: Value = serde_json::from_str(&text).expect("json");
    if let Some(members) = value.as_object_mut() {
        members.remove("tools");
    }
    shorten(&mut value);
    serde_json::to_string_pretty(&value).expect("json")
}

/// The records of the third turn of [`auto_compaction_records`]: one big read, then the
/// compaction before the second call, which goes on with the summary and the tail.
fn auto_compaction_records(setup: &Setup) -> (Vec<Record>, Request, Vec<Message>) {
    let state = setup.live_state(&setup.cwd, "one");
    let (mut records, history) = two_big_turns(setup);
    let input = big_file(setup, 90_000);
    let mut first = history;
    first.push(setup.prompt(&state, "three"));
    let call = tool_message("call_1", "read_file", &input);
    let result = result_message("call_1", &big_text(90_000), false);
    let mut refused = first.clone();
    refused.extend([call.clone(), result.clone()]);
    let after = vec![fresh(setup), summary_message(SUMMARY), call, result];
    records.extend([
        expect_request(request(first)),
        answer(&tool_answer("call_1", "read_file", &input)),
        expect_request(summary(refused.clone())),
        answer(&text_answer(SUMMARY)),
        expect_request(request(after.clone())),
        answer(&text_answer("done")),
    ]);
    (records, request(refused), after)
}

#[tokio::test]
async fn an_auto_compaction_at_the_trigger_summarizes_and_the_turn_goes_on() {
    let setup = compacting();
    let (records, before, after) = auto_compaction_records(&setup);
    assert!(request_tokens(&before) >= TRIGGER, "{}", request_tokens(&before));
    let mut receiver = setup.drafts.subscribe();
    let mut h = setup.start(records).await;
    let (_, two) = run_two_big_turns(&mut h).await;

    let three = h.prompt("three").await.turn_id;
    let end = h.wait_end(three).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    let drafts: Vec<DraftPart> = std::iter::from_fn(|| receiver.try_recv().ok())
        .map(|draft| draft.part)
        .filter(|part| matches!(part, DraftPart::Context(_) | DraftPart::Compacting { .. }))
        .collect();
    let limits = ContextLimits::new(Some(WINDOW), CompactionConfig::default());
    let at = drafts
        .iter()
        .position(|part| matches!(part, DraftPart::Compacting { trigger: CompactionTrigger::Auto }))
        .expect("a compacting draft");
    assert_eq!(
        drafts[at - 1],
        DraftPart::Context(limits.gauge(request_tokens(&before))),
        "the estimate that reached the trigger: {drafts:?}"
    );
    assert_eq!(
        drafts[at + 1],
        DraftPart::Context(limits.gauge(request_tokens(&request(after.clone())))),
        "the estimate of the compacted request: {drafts:?}"
    );
    let kinds = h.kinds().await;
    let from = kinds.iter().rposition(|kind| kind == "tool_call_completed").expect("a call");
    assert_eq!(kinds[from + 1], "conversation_compacted");
    let compacted = compactions(&h).await;
    let [compaction] = compacted.as_slice() else {
        panic!("one compaction: {compacted:?}");
    };
    assert_eq!(
        compaction,
        &Compaction {
            compaction_id: compaction.compaction_id,
            turn_id: Some(three),
            trigger: CompactionTrigger::Auto,
            focus: None,
            model: MODEL.to_owned(),
            window: WINDOW,
            limit: TRIGGER,
            tokens_before: request_tokens(&before),
            tokens_after: request_tokens(&request(after)),
            through_turn: three,
            through_message: Some(1),
            kept_turns: 1,
            pruned_outputs: 0,
            pruned_tokens: 0,
            omitted_turns: 0,
            omitted_messages: 0,
            summary: Some(SUMMARY.to_owned()),
            usage: None,
        }
    );
    // The summary covers the first two turns and the prompt of the third; nothing of
    // them is needed again.
    let id = h.conversation_id;
    let saved = h
        .store
        .readers()
        .with(move |conn| turn_messages::of_conversation(conn, id))
        .await
        .expect("saved turns");
    let saved: Vec<TurnId> = saved.iter().map(|turn| turn.turn_id).collect();
    assert_eq!(saved, [three]);
    assert!(!saved.contains(&two));
    h.finish();
}

/// The request of the turn after [`auto_compaction_records`]: the fresh block, the
/// summary, the tail of the third turn and the new prompt.
fn after_compaction(setup: &Setup, after: &[Message]) -> Request {
    let state = setup.live_state(&setup.cwd, "one");
    let mut messages = after.to_vec();
    messages.push(Message::assistant("done"));
    messages.push(setup.prompt(&state, "four"));
    request(messages)
}

#[tokio::test]
async fn the_next_turn_sends_the_fresh_block_the_summary_and_the_tail() {
    let setup = compacting();
    let (mut records, _, after) = auto_compaction_records(&setup);
    let next = after_compaction(&setup, &after);
    let root = setup.home().parent().expect("the test's root").to_path_buf();
    insta::assert_snapshot!(shape(&next, &root));
    records.extend([expect_request(next), answer(&text_answer("ok 4"))]);
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;
    let three = h.prompt("three").await.turn_id;
    h.wait_end(three).await;

    let four = h.prompt("four").await.turn_id;
    let end = h.wait_end(four).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}

#[tokio::test]
async fn after_a_restart_the_request_is_rebuilt_from_the_compaction_and_the_tail() {
    let setup = compacting();
    let (records, _, after) = auto_compaction_records(&setup);
    let next = after_compaction(&setup, &after);
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;
    let three = h.prompt("three").await.turn_id;
    h.wait_end(three).await;
    h.finish();
    let cwd = h.cwd.clone();

    // The cache and the fresh block of the actor are gone: the summary comes from the
    // store's compactions, the tail from the saved messages, the block from the disk.
    let records = vec![expect_request(next), answer(&text_answer("ok 4"))];
    let mut h = h.restart(records, "replay").await;
    let four = h.prompt_in(&cwd, "four").await.turn_id;
    let end = h.wait_end(four).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}

const OVERFLOW: &str = r#"{"kind": "api", "code": "context_length_exceeded", "message": "Your input exceeds the context window of this model."}"#;

/// The records of a third turn whose first request the provider refuses as too large.
/// The summary request of the same history would be refused too, so it leaves out the
/// oldest message, and the history after the compaction says so.
fn overflow_records(setup: &Setup) -> (Vec<Record>, Request, Vec<Message>) {
    let state = setup.live_state(&setup.cwd, "one");
    let (mut records, history) = two_big_turns(setup);
    let mut first = history;
    first.push(setup.prompt(&state, "three"));
    let mut fitting = vec![dropped_note(1)];
    fitting.extend(first[1..].iter().cloned());
    let after = vec![
        fresh(setup),
        summary_message(SUMMARY),
        gap_note(0, 1).expect("a note"),
        Message::assistant("ok 2"),
        setup.prompt(&state, "three"),
    ];
    records.extend([
        expect_request(request(first.clone())),
        failure(serde_json::from_str(OVERFLOW).expect("json")),
        expect_request(summary(fitting)),
        answer(&text_answer(SUMMARY)),
        expect_request(request(after.clone())),
    ]);
    (records, request(first), after)
}

#[tokio::test]
async fn an_overflow_compacts_once_and_sends_the_call_again() {
    let setup = compacting();
    let (mut records, refused, after) = overflow_records(&setup);
    records.push(answer(&text_answer("done")));
    let mut h = setup.start(records).await;
    let (_, two) = run_two_big_turns(&mut h).await;

    let three = h.prompt("three").await.turn_id;
    let end = h.wait_end(three).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    let compacted = compactions(&h).await;
    let [compaction] = compacted.as_slice() else {
        panic!("one compaction: {compacted:?}");
    };
    assert_eq!(compaction.trigger, CompactionTrigger::Overflow);
    assert_eq!(compaction.tokens_before, request_tokens(&refused));
    assert_eq!(compaction.tokens_after, request_tokens(&request(after)));
    assert_eq!((compaction.through_turn, compaction.through_message), (two, Some(1)));
    assert_eq!(compaction.kept_turns, 2);
    assert_eq!((compaction.omitted_turns, compaction.omitted_messages), (0, 1));
    h.finish();
}

#[tokio::test]
async fn a_second_overflow_of_the_same_call_fails_the_turn_and_names_the_cause() {
    let setup = compacting();
    let (mut records, _, after) = overflow_records(&setup);
    records.push(failure(serde_json::from_str(OVERFLOW).expect("json")));
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;

    let three = h.prompt("three").await.turn_id;
    let end = h.wait_end(three).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails: {end:?}");
    };
    // The estimate of the request that the provider refused the second time.
    let tokens = request_tokens(&request(after));
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(
        error.data,
        Some(json!({ "cause": "context_overflow", "tokens": tokens, "window": WINDOW }))
    );
    assert_eq!(
        error.message,
        format!(
            "the context is still full after a compaction: the model refused about {} of \
             100k tokens; run ,compact or start a new conversation",
            kilo(tokens)
        )
    );
    assert_eq!(compactions(&h).await.len(), 1);
    h.finish();
}

#[tokio::test]
async fn with_auto_off_an_overflow_fails_the_turn_without_a_compaction() {
    let mut setup = compacting();
    setup.config.compaction = CompactionConfig::new(false, 76);
    let state = setup.live_state(&setup.cwd, "one");
    let (mut records, history) = two_big_turns(&setup);
    let mut first = history;
    first.push(setup.prompt(&state, "three"));
    records.extend([
        expect_request(request(first)),
        failure(serde_json::from_str(OVERFLOW).expect("json")),
    ]);
    let mut h = setup.start(records).await;
    run_two_big_turns(&mut h).await;

    let three = h.prompt("three").await.turn_id;
    let end = h.wait_end(three).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails: {end:?}");
    };
    assert!(error.message.contains("run ,compact"), "{}", error.message);
    assert!(compactions(&h).await.is_empty());
    h.finish();
}

#[tokio::test]
async fn the_breaker_stops_compacting_after_two_misses_and_the_turn_fails_at_the_cap() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "hello");
    let input = big_file(&setup, 320_000);
    let output = big_text(320_000);
    let (call_1, result_1) =
        (tool_message("call_1", "read_file", &input), result_message("call_1", &output, false));
    let (call_2, result_2) =
        (tool_message("call_2", "read_file", &input), result_message("call_2", &output, false));
    let first =
        vec![setup.prompt(&state, "hello"), Message::assistant("hi"), setup.prompt(&state, "go")];
    let mut refused = first.clone();
    refused.extend([call_1.clone(), result_1.clone()]);
    let after_1 = vec![fresh(&setup), summary_message(SUMMARY), call_1.clone(), result_1.clone()];
    let mut before_2 = after_1.clone();
    before_2.extend([call_2.clone(), result_2.clone()]);
    let pruned_1 = result_message("call_1", PRUNED_OUTPUT_STUB, false);
    let pruned = vec![
        fresh(&setup),
        summary_message(SUMMARY),
        call_1,
        pruned_1,
        call_2.clone(),
        result_2.clone(),
    ];
    let after_2 = vec![fresh(&setup), summary_message(SUMMARY_2), call_2, result_2];
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        answer(&text_answer("hi")),
        expect_request(request(first)),
        answer(&tool_answer("call_1", "read_file", &input)),
        expect_request(summary(refused.clone())),
        answer(&text_answer(SUMMARY)),
        expect_request(request(after_1.clone())),
        answer(&tool_answer("call_2", "read_file", &input)),
        expect_request(summary(pruned)),
        answer(&text_answer(SUMMARY_2)),
        expect_request(request(after_2.clone())),
        answer(&tool_answer("call_3", "read_file", &input)),
    ];
    let mut h = setup.start(records).await;
    let hello = h.prompt("hello").await.turn_id;
    h.wait_end(hello).await;

    let go = h.prompt("go").await.turn_id;
    let end = h.wait_end(go).await;

    let compacted = compactions(&h).await;
    let [one, two] = compacted.as_slice() else {
        panic!("two compactions: {compacted:?}");
    };
    assert_eq!(one.tokens_after, request_tokens(&request(after_1)));
    assert!(one.tokens_after >= TRIGGER, "a miss: {}", one.tokens_after);
    assert_eq!(two.tokens_before, request_tokens(&request(before_2)));
    assert_eq!(two.pruned_outputs, 1, "the summary request needed the pruned copy");
    assert!(two.tokens_after >= TRIGGER, "a second miss: {}", two.tokens_after);
    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails at the cap: {end:?}");
    };
    assert!(
        error.message.starts_with("the context is full: compaction did not free enough room"),
        "{}",
        error.message
    );
    let tokens = error.data.as_ref().and_then(|data| data["tokens"].as_u64()).unwrap_or(0);
    assert!(tokens > HARD_CAP, "{tokens}");
    h.finish();
}

#[tokio::test]
async fn a_pruning_that_frees_enough_compacts_without_a_summary() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "go");
    let input = big_file(&setup, 80_000);
    let output = big_text(80_000);
    let ids = ["call_1", "call_2", "call_3", "call_4"];
    let mut messages = vec![setup.prompt(&state, "go")];
    let mut records = Vec::new();
    for id in ids {
        records.push(expect_request(request(messages.clone())));
        records.push(answer(&tool_answer(id, "read_file", &input)));
        messages.push(tool_message(id, "read_file", &input));
        messages.push(result_message(id, &output, false));
    }
    let before = request(messages.clone());
    for (at, id) in ids.iter().take(3).enumerate() {
        messages[2 + at * 2] = result_message(id, PRUNED_OUTPUT_STUB, false);
    }
    records.push(expect_request(request(messages.clone())));
    records.push(answer(&text_answer("done")));
    // The next turn rebuilds the stubs from the compaction in the store.
    let mut history = messages.clone();
    history.push(Message::assistant("done"));
    history.push(setup.prompt(&state, "next"));
    records.push(expect_request(request(history)));
    records.push(answer(&text_answer("ok")));
    let mut h = setup.start(records).await;

    let go = h.prompt("go").await.turn_id;
    h.wait_end(go).await;
    let next = h.prompt("next").await.turn_id;
    let end = h.wait_end(next).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    let compacted = compactions(&h).await;
    let [compaction] = compacted.as_slice() else {
        panic!("one compaction: {compacted:?}");
    };
    assert_eq!(compaction.summary, None);
    assert_eq!(compaction.pruned_outputs, 3);
    assert!(compaction.pruned_tokens >= 20_000, "{}", compaction.pruned_tokens);
    assert_eq!(compaction.tokens_before, request_tokens(&before));
    assert_eq!(compaction.tokens_after, request_tokens(&request(messages)));
    assert_eq!((compaction.through_turn, compaction.through_message), (go, Some(7)));
    h.finish();
}

#[tokio::test]
async fn a_failed_summary_above_the_hard_cap_names_the_failure_and_its_cause() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "hello");
    let input = big_file(&setup, 400_000);
    let output = big_text(400_000);
    let call = tool_message("call_1", "read_file", &input);
    let result = result_message("call_1", &output, false);
    let first =
        vec![setup.prompt(&state, "hello"), Message::assistant("hi"), setup.prompt(&state, "go")];
    let mut refused = first.clone();
    refused.extend([call, result]);
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        answer(&text_answer("hi")),
        expect_request(request(first)),
        answer(&tool_answer("call_1", "read_file", &input)),
        expect_request(summary(refused.clone())),
        failure(json!({ "kind": "rate_limited", "retry_after_ms": 11_000 })),
    ];
    assert!(request_tokens(&request(refused.clone())) > HARD_CAP);
    let mut h = setup.start(records).await;
    let hello = h.prompt("hello").await.turn_id;
    h.wait_end(hello).await;

    let go = h.prompt("go").await.turn_id;
    let end = h.wait_end(go).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails: {end:?}");
    };
    assert!(
        error.message.starts_with("the context is full and the compaction failed (the provider"),
        "{}",
        error.message
    );
    assert!(error.message.contains("above the cap of 95k"), "{}", error.message);
    assert!(compactions(&h).await.is_empty());
    h.finish();
}

#[tokio::test]
async fn after_an_overflow_pruning_alone_is_not_trusted_and_a_summary_follows() {
    let setup = compacting();
    let state = setup.live_state(&setup.cwd, "go");
    let input = big_file(&setup, 80_000);
    let output = big_text(80_000);
    let ids = ["call_1", "call_2", "call_3"];
    let mut messages = vec![setup.prompt(&state, "go")];
    let mut records = Vec::new();
    for id in ids {
        records.push(expect_request(request(messages.clone())));
        records.push(answer(&tool_answer(id, "read_file", &input)));
        messages.push(tool_message(id, "read_file", &input));
        messages.push(result_message(id, &output, false));
    }
    // The estimate is below the trigger, and the provider refuses it all the same.
    let refused = request(messages.clone());
    assert!(request_tokens(&refused) < TRIGGER);
    let mut pruned = messages.clone();
    for at in [2, 4] {
        pruned[at] = result_message(ids[at / 2 - 1], PRUNED_OUTPUT_STUB, false);
    }
    let after =
        vec![fresh(&setup), summary_message(SUMMARY), messages[5].clone(), messages[6].clone()];
    records.extend([
        expect_request(refused.clone()),
        failure(serde_json::from_str(OVERFLOW).expect("json")),
        expect_request(summary(pruned)),
        answer(&text_answer(SUMMARY)),
        expect_request(request(after)),
        answer(&text_answer("done")),
    ]);
    let mut h = setup.start(records).await;

    let go = h.prompt("go").await.turn_id;
    let end = h.wait_end(go).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    let compacted = compactions(&h).await;
    let [compaction] = compacted.as_slice() else {
        panic!("one compaction: {compacted:?}");
    };
    assert_eq!(compaction.trigger, CompactionTrigger::Overflow);
    assert_eq!(compaction.summary.as_deref(), Some(SUMMARY));
    assert_eq!(compaction.pruned_outputs, 2);
    h.finish();
}
