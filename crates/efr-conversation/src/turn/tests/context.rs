//! The context of a turn: the usage of its last call, the estimate before each call,
//! the guards (the trigger with nothing to compact, the hard cap, an overflow with auto
//! off), and the note for turns that the history leaves out. `compaction.rs` has the
//! compactions themselves.

use efr_protocol::{ContextUse, DraftPart, ErrorCode, Event, ModelInfo, ModelSource, Usage};
use efr_provider::{Message, ProviderEvent, StopReason, TokenUsage};
use efr_test_support::Record;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::context::{message_tokens, request_tokens};
use crate::testing::{
    MODEL, Setup, answer, done, expect_request, failure, request, result_message, text_answer,
    tool_answer, tool_message,
};
use crate::{CompactionConfig, ContextLimits, ConversationDraft, HistoryLimits};

/// The test model with a window of `window` tokens.
fn with_window(setup: &mut Setup, window: u64) -> ContextLimits {
    setup.config.models = vec![ModelInfo {
        id: MODEL.to_owned(),
        efforts: Vec::new(),
        default_effort: None,
        default: true,
        source: ModelSource::Builtin,
        context_window: Some(window),
        max_context_window: None,
        prefer_websockets: false,
    }];
    ContextLimits::new(Some(window), setup.config.compaction)
}

/// The window in which a request of `tokens` tokens is at 90% of the window: past the
/// default trigger (76%) and under the hard cap (95%).
fn window_past_the_trigger(tokens: u64) -> u64 {
    tokens * 100 / 90
}

fn usage(input: u64, output: u64, cached: u64, reasoning: u64) -> ProviderEvent {
    ProviderEvent::Usage(TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: cached,
        reasoning_tokens: reasoning,
        ..TokenUsage::default()
    })
}

/// A text answer whose call reports `input` and `output` tokens.
fn counted_answer(text: &str, input: u64, output: u64) -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::TextDelta { text: text.to_owned() },
        usage(input, output, 0, 0),
        done(StopReason::EndTurn, None),
    ]
}

/// The answer of a provider that refuses the request as larger than the window.
fn overflow() -> Record {
    failure(json!({
        "kind": "api",
        "code": "context_length_exceeded",
        "message": "Your input exceeds the context window of this model.",
    }))
}

/// The `context` drafts that `receiver` holds now.
fn contexts(receiver: &mut broadcast::Receiver<ConversationDraft>) -> Vec<ContextUse> {
    let mut found = Vec::new();
    while let Ok(draft) = receiver.try_recv() {
        if let DraftPart::Context(context) = draft.part {
            found.push(context);
        }
    }
    found
}

/// The compactions that the log holds.
fn compactions(events: &[Event]) -> Vec<efr_protocol::Compaction> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ConversationCompacted(compaction) => Some(compaction.clone()),
            _ => None,
        })
        .collect()
}

/// The data of a `turn_failed` for a context that does not fit.
fn full(tokens: u64, window: u64) -> Option<Value> {
    Some(json!({ "cause": "context_overflow", "tokens": tokens, "window": window }))
}

#[tokio::test]
async fn a_turn_ends_with_the_sums_and_the_context_of_its_last_call() {
    let mut setup = Setup::new();
    let limits = with_window(&mut setup, 10_000);
    let state = setup.live_state(&setup.cwd, "read my notes");
    let notes = setup.home().join("notes.txt");
    let input = json!({ "path": notes });
    let first = setup.prompt(&state, "read my notes");
    let results = result_message("call_1", &format!("contents of {}", notes.display()), false);
    let requests = [
        request(vec![first.clone()]),
        request(vec![first, tool_message("call_1", "read_file", &input), results.clone()]),
    ];
    let mut calling = tool_answer("call_1", "read_file", &input);
    calling.insert(2, usage(1000, 50, 800, 20));
    let records = vec![
        expect_request(requests[0].clone()),
        answer(&calling),
        expect_request(requests[1].clone()),
        answer(&[
            ProviderEvent::TextDelta { text: "They say hi.".to_owned() },
            usage(1200, 30, 1000, 0),
            done(StopReason::EndTurn, None),
        ]),
    ];
    let mut receiver = setup.drafts.subscribe();
    let mut h = setup.start(records).await;

    let sent = h.prompt("read my notes").await;
    let end = h.wait_end(sent.turn_id).await;

    let usage = Usage {
        input_tokens: 2200,
        output_tokens: 80,
        cached_input_tokens: 1800,
        reasoning_tokens: 20,
        context_tokens: 1230,
        ..Usage::default()
    };
    assert_eq!(
        end,
        Event::TurnCompleted {
            turn_id: sent.turn_id,
            usage: Some(usage),
            context: Some(limits.gauge(1230)),
            changes: None,
        },
        "the sums, and the input plus the output of the last call"
    );
    assert_eq!(
        contexts(&mut receiver),
        [
            // The first call has no count yet: the estimate of the whole request.
            limits.gauge(request_tokens(&requests[0])),
            limits.gauge(1050),
            // The second: the count of the first plus the result after it.
            limits.gauge(1050 + message_tokens(&results)),
            limits.gauge(1230),
        ]
    );
    let key = json!(h.conversation_id.to_string());
    assert!(
        requests.iter().all(|request| request.provider_options["prompt_cache_key"] == key),
        "every request names the conversation as its cache key"
    );
    h.finish();
}

#[tokio::test]
async fn a_cache_key_in_the_config_wins_over_the_conversation_id() {
    let mut setup = Setup::new();
    setup.config.provider_options.insert("prompt_cache_key".to_owned(), json!("mine"));
    let state = setup.live_state(&setup.cwd, "hello");
    let mut expected = request(vec![setup.prompt(&state, "hello")]);
    expected.provider_options.insert("prompt_cache_key".to_owned(), json!("mine"));
    let records = vec![expect_request(expected), answer(&text_answer("Hi."))];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    h.wait_end(sent.turn_id).await;

    h.finish();
}

#[tokio::test]
async fn the_next_turn_estimates_from_the_context_at_the_end_of_the_last_one() {
    let mut setup = Setup::new();
    let limits = with_window(&mut setup, 10_000);
    let state = setup.live_state(&setup.cwd, "first");
    let second = setup.prompt(&state, "second");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&counted_answer("One.", 900, 100)),
        expect_request(request(vec![
            setup.prompt(&state, "first"),
            Message::assistant("One."),
            second.clone(),
        ])),
        answer(&counted_answer("Two.", 1400, 20)),
    ];
    let mut receiver = setup.drafts.subscribe();
    let mut h = setup.start(records).await;

    let sent = h.prompt("first").await;
    h.wait_end(sent.turn_id).await;
    contexts(&mut receiver);
    let sent = h.prompt("second").await;
    h.wait_end(sent.turn_id).await;

    assert_eq!(
        contexts(&mut receiver),
        [limits.gauge(1000 + message_tokens(&second)), limits.gauge(1420)],
        "the count at the end of the first turn plus the new prompt"
    );
    h.finish();
}

#[tokio::test]
async fn a_request_above_the_hard_cap_is_never_sent_when_auto_compaction_is_off() {
    let mut setup = Setup::new();
    setup.config.compaction = CompactionConfig::new(false, 76);
    let state = setup.live_state(&setup.cwd, "hello");
    let estimate = request_tokens(&request(vec![setup.prompt(&state, "hello")]));
    // NOTE: the hard cap is 95% of the window, so the request is just above it.
    let limits = with_window(&mut setup, estimate);
    let mut h = setup.start(Vec::new()).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, context, .. } = end else {
        panic!("the turn fails, got {end:?}");
    };
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.data, full(estimate, estimate));
    assert!(error.message.starts_with("the context is full"), "{}", error.message);
    assert!(error.message.ends_with("run ,compact or start a new conversation"));
    assert_eq!(context, Some(limits.gauge(estimate)));
    assert!(compactions(&h.events().await).is_empty(), "auto compaction is off");
    // The transcript has no request: the replay fails on any.
    h.finish();
}

#[tokio::test]
async fn an_auto_compaction_that_frees_nothing_lets_the_turn_go_on_under_the_hard_cap() {
    let mut setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hello");
    let first = request(vec![setup.prompt(&state, "hello")]);
    with_window(&mut setup, window_past_the_trigger(request_tokens(&first)));
    let records = vec![expect_request(first), answer(&text_answer("Hi."))];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    assert!(
        compactions(&h.events().await).is_empty(),
        "a compaction that frees nothing records nothing"
    );
    h.finish();
}

#[tokio::test]
async fn the_breaker_stays_open_from_turn_to_turn_while_the_context_is_over_the_trigger() {
    let mut setup = Setup::new();
    // NOTE: a long prompt, so the preambles that the later requests send again are small
    // beside it.
    let hello = format!("hello\n{}", "x".repeat(60_000));
    let state = setup.live_state(&setup.cwd, &hello);
    let first = request(vec![setup.prompt(&state, &hello)]);
    // At 80% of the window, past the trigger; nothing lies before the tail, so each
    // compaction is a miss.
    let limits = with_window(&mut setup, request_tokens(&first) * 100 / 80);
    let second = request(vec![
        setup.prompt(&state, &hello),
        Message::assistant("Hi."),
        setup.prompt(&state, "again"),
    ]);
    let third = request(vec![
        setup.prompt(&state, &hello),
        Message::assistant("Hi."),
        setup.prompt(&state, "again"),
        Message::assistant("Hi again."),
        setup.prompt(&state, "more"),
    ]);
    assert!(request_tokens(&third) <= limits.hard_cap);
    let records = vec![
        expect_request(first),
        answer(&text_answer("Hi.")),
        expect_request(second),
        answer(&text_answer("Hi again.")),
        expect_request(third),
        answer(&text_answer("Still here.")),
    ];
    let mut receiver = setup.drafts.subscribe();
    let mut h = setup.start(records).await;

    let mut tries = Vec::new();
    for text in [hello.as_str(), "again", "more"] {
        let sent = h.prompt(text).await;
        h.wait_end(sent.turn_id).await;
        let mut compacting = 0;
        while let Ok(draft) = receiver.try_recv() {
            if matches!(draft.part, DraftPart::Compacting { .. }) {
                compacting += 1;
            }
        }
        tries.push(compacting);
    }

    assert_eq!(tries, [1, 1, 0], "two misses open the breaker for the next turns");
    h.finish();
}

#[tokio::test]
async fn an_auto_compaction_that_frees_nothing_above_the_hard_cap_fails_the_turn() {
    let mut setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hello");
    let estimate = request_tokens(&request(vec![setup.prompt(&state, "hello")]));
    with_window(&mut setup, estimate);
    let mut h = setup.start(Vec::new()).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails, got {end:?}");
    };
    assert_eq!(error.data, full(estimate, estimate));
    assert!(error.message.starts_with("the context is full: about"), "{}", error.message);
    assert!(compactions(&h.events().await).is_empty());
    h.finish();
}

#[tokio::test]
async fn an_overflow_with_auto_compaction_off_fails_the_turn_at_once() {
    let mut setup = Setup::new();
    setup.config.compaction = CompactionConfig::new(false, 76);
    let state = setup.live_state(&setup.cwd, "hello");
    let first = request(vec![setup.prompt(&state, "hello")]);
    let records = vec![expect_request(first.clone()), overflow()];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    let Event::TurnFailed { error, .. } = end else {
        panic!("the turn fails, got {end:?}");
    };
    assert_eq!(error.data, full(request_tokens(&first), 128_000));
    assert!(error.message.contains("the model refused"), "{}", error.message);
    assert!(error.message.contains(",compact"), "{}", error.message);
    assert!(compactions(&h.events().await).is_empty());
    h.finish();
}

#[tokio::test]
async fn the_safety_net_scales_with_the_window_and_never_drops_turns_below_the_trigger() {
    // The default limits (512 KiB) on a window of 272000 tokens: one prompt of 600 KB
    // is about 150000 tokens, below the trigger of 206720, and above the byte limit.
    let mut setup = Setup::new();
    let limits = with_window(&mut setup, 272_000);
    setup.config.history = HistoryLimits::default();
    let big = format!("first\n{}", "x".repeat(600_000));
    let state = setup.live_state(&setup.cwd, &big);
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, &big)])),
        answer(&text_answer("One.")),
        expect_request(request(vec![
            setup.prompt(&state, &big),
            Message::assistant("One."),
            setup.prompt(&state, "second"),
        ])),
        answer(&text_answer("Two.")),
        expect_request(request(vec![
            setup.prompt(&state, &big),
            Message::assistant("One."),
            setup.prompt(&state, "second"),
            Message::assistant("Two."),
            setup.prompt(&state, "third"),
        ])),
        answer(&text_answer("Three.")),
    ];
    assert!(request_tokens(&request(vec![Message::user(big.clone())])) < limits.trigger);
    let mut h = setup.start(records).await;

    for text in [big.as_str(), "second", "third"] {
        let sent = h.prompt(text).await;
        h.wait_end(sent.turn_id).await;
    }

    h.finish();
}

#[tokio::test]
async fn a_page_of_events_that_misses_a_turn_leaves_out_no_turn() {
    // A page of 6 events holds the second turn's start but not the first's. The list of
    // turns comes from the turns, and the first turn's saved messages go as they are.
    let mut setup = Setup::new();
    setup.config.history = HistoryLimits::new(6, 512 * 1024);
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
        expect_request(request(vec![
            setup.prompt(&state, "first"),
            Message::assistant("One."),
            setup.prompt(&state, "second"),
            Message::assistant("Two."),
            setup.prompt(&state, "third"),
        ])),
        answer(&text_answer("Three.")),
    ];
    let mut h = setup.start(records).await;

    for text in ["first", "second", "third"] {
        let sent = h.prompt(text).await;
        h.wait_end(sent.turn_id).await;
    }

    h.finish();
}
