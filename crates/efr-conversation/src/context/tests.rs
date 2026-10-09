use efr_protocol::{
    ContextUse, ConversationId, EffectiveSettings, ErrorBody, ErrorCode, Event, EventEnvelope,
    Mode, OverriddenSettings, Seq, TurnId, Usage,
};
use efr_provider::{Message, Request};
use efr_stdx::id::uuid_v7;
use efr_stdx::time::Clock as _;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    CompactionConfig, ContextLimits, DEFAULT_CONTEXT_WINDOW, Full, HARD_CAP_PERCENT, Meter,
    context_base, context_full, estimate_tokens, kilo, message_tokens, request_tokens,
};

/// The model of the turns of the tests.
const MODEL: &str = "gpt-5.5";

#[test]
fn the_trigger_is_auto_at_percent_of_the_window_and_the_cap_95() {
    let limits = ContextLimits::new(Some(272_000), CompactionConfig::default());

    assert_eq!(limits.window, 272_000);
    assert_eq!(limits.trigger, 206_720);
    assert_eq!(limits.hard_cap, 258_400);
    assert_eq!(limits.limit(), 206_720);
    assert_eq!(
        limits.gauge(89_000),
        ContextUse { tokens: 89_000, limit: 206_720, window: 272_000 }
    );
}

#[test]
fn with_auto_off_a_client_counts_against_the_hard_cap() {
    let limits = ContextLimits::new(Some(200_000), CompactionConfig::new(false, 76));

    assert_eq!(limits.limit(), 200_000 * HARD_CAP_PERCENT / 100);
}

#[test]
fn an_unknown_window_counts_as_the_default_and_auto_at_stays_in_range() {
    let unknown = ContextLimits::new(None, CompactionConfig::default());
    let zero = ContextLimits::new(Some(0), CompactionConfig::new(true, 0));
    let full = ContextLimits::new(Some(100_000), CompactionConfig::new(true, 150));

    assert_eq!(unknown.window, DEFAULT_CONTEXT_WINDOW);
    assert_eq!(zero.trigger, DEFAULT_CONTEXT_WINDOW / 100);
    assert_eq!(full.trigger, 99_000);
}

#[test]
fn the_estimate_is_four_bytes_a_token_rounded_up() {
    assert_eq!(estimate_tokens(0), 0);
    assert_eq!(estimate_tokens(1), 1);
    assert_eq!(estimate_tokens(4_000), 1_000);
    assert_eq!(estimate_tokens(4_001), 1_001);
}

#[test]
fn the_meter_counts_from_the_last_call_and_adds_what_came_after() {
    let request = Request { messages: vec![Message::user("x".repeat(400))], ..Request::new("m") };
    let json = serde_json::to_vec(&request).unwrap();
    let mut meter = Meter::new(None);
    assert_eq!(meter.estimate(&request), (json.len() as u64).div_ceil(4), "the whole request");
    assert_eq!(meter.known(), None);

    meter.counted(1_000);
    let result = Message::user("y".repeat(40));
    meter.add(&result);
    assert_eq!(meter.estimate(&request), 1_000 + message_tokens(&result));
    meter.counted(1_200);
    assert_eq!(meter.known(), Some(1_200), "a real count replaces the estimate");

    meter.reset();
    assert_eq!(meter.estimate(&request), request_tokens(&request), "a compaction drops the base");
}

fn envelope(seq: u64, event: Event) -> EventEnvelope {
    let conversation_id = ConversationId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(1)));
    EventEnvelope {
        seq: Seq::new(seq),
        conversation_id: Some(conversation_id),
        at: TestClock::new().now(),
        event,
    }
}

#[test]
fn the_base_is_the_context_at_the_end_of_the_newest_turn() {
    let turn = TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(2)));
    let limits = ContextLimits::new(Some(10_000), CompactionConfig::default());
    let usage = Usage { context_tokens: 700, ..Usage::default() };
    let ended = |tokens: u64| Event::TurnCompleted {
        turn_id: turn,
        usage: Some(usage),
        context: Some(limits.gauge(tokens)),
        changes: None,
    };
    let uncounted = Event::TurnInterrupted { turn_id: turn, usage: Some(usage), context: None };
    let estimated = Event::TurnFailed {
        turn_id: turn,
        error: ErrorBody::new(ErrorCode::Internal, "the context is full"),
        usage: None,
        context: Some(limits.gauge(900)),
    };

    assert_eq!(context_base(&[envelope(1, ended(800)), envelope(2, ended(900))], MODEL), Some(900));
    assert_eq!(
        context_base(&[envelope(1, ended(800)), envelope(2, uncounted)], MODEL),
        None,
        "the newest turn is in no count"
    );
    assert_eq!(
        context_base(&[envelope(1, ended(800)), envelope(2, estimated)], MODEL),
        None,
        "no call of the newest turn reported a count"
    );
    assert_eq!(context_base(&[], MODEL), None);
    assert_eq!(
        context_base(
            &[envelope(1, ended(800)), envelope(2, Event::TurnCancelled { turn_id: turn })],
            MODEL
        ),
        None,
        "a cancelled turn, as after a restart, is in no count"
    );
    let started = Event::TurnStarted {
        turn_id: turn,
        cwd: "/home/u".into(),
        scope: efr_protocol::Scope::Machine,
        settings: None,
    };
    assert_eq!(
        context_base(&[envelope(1, ended(800)), envelope(2, started)], MODEL),
        None,
        "a turn without an end is in no count"
    );
}

#[test]
fn the_count_of_another_model_is_no_base() {
    let turn = TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(3)));
    let limits = ContextLimits::new(Some(1_000_000), CompactionConfig::default());
    let started = |model: &str| Event::TurnStarted {
        turn_id: turn,
        cwd: "/home/u".into(),
        scope: efr_protocol::Scope::Machine,
        settings: Some(EffectiveSettings {
            mode: Mode::Cautious,
            model: model.to_owned(),
            effort: None,
            overridden: OverriddenSettings::default(),
            fallback: None,
        }),
    };
    let ended = Event::TurnCompleted {
        turn_id: turn,
        usage: Some(Usage { context_tokens: 500_000, ..Usage::default() }),
        context: Some(limits.gauge(500_887)),
        changes: None,
    };
    let claude = [envelope(1, started("claude-opus-5-5")), envelope(2, ended.clone())];

    assert_eq!(context_base(&claude, "claude-opus-5-5"), Some(500_887));
    assert_eq!(context_base(&claude, "gpt-5.5"), None, "a switch drops the base");
    assert_eq!(
        context_base(&[envelope(2, ended)], "gpt-5.5"),
        Some(500_887),
        "a turn whose start is not in the page keeps its count"
    );
}

#[test]
fn a_full_context_names_the_cause_and_the_way_out() {
    let limits = ContextLimits::new(Some(272_000), CompactionConfig::default());

    let refused = context_full(Full::Refused, 281_000, &limits);
    let cap = context_full(Full::Cap, 260_000, &limits);
    let failed = context_full(
        Full::CompactionFailed {
            refused: false,
            failure: "the provider is rate limiting requests".to_owned(),
        },
        260_000,
        &limits,
    );
    let failed_refused = context_full(
        Full::CompactionFailed { refused: true, failure: "the summary was empty".to_owned() },
        281_000,
        &limits,
    );
    let tail = context_full(Full::TailTooLarge { refused: false }, 260_000, &limits);
    let tail_refused = context_full(Full::TailTooLarge { refused: true }, 281_000, &limits);

    assert_eq!(refused.code, ErrorCode::Internal);
    assert_eq!(
        refused.message,
        "the context is full: the model refused about 281k of 272k tokens; run ,compact or \
         start a new conversation"
    );
    assert_eq!(
        refused.data,
        Some(json!({ "cause": "context_overflow", "tokens": 281_000, "window": 272_000 }))
    );
    assert_eq!(
        cap.message,
        "the context is full: about 260k of 272k tokens, above the cap of 258k; run ,compact \
         or start a new conversation"
    );
    assert_eq!(
        failed.message,
        "the context is full and the compaction failed (the provider is rate limiting \
         requests): about 260k of 272k tokens, above the cap of 258k; run ,compact or start a \
         new conversation"
    );
    assert_eq!(
        failed_refused.message,
        "the context is full and the compaction failed (the summary was empty): the model \
         refused about 281k of 272k tokens; run ,compact or start a new conversation"
    );
    // Nothing lies before the tail: a manual compaction frees nothing either.
    assert_eq!(
        tail.message,
        "the context is full and the newest messages alone do not fit: about 260k of 272k \
         tokens, above the cap of 258k; start a new conversation"
    );
    assert_eq!(
        tail_refused.message,
        "the context is full and the newest messages alone do not fit: the model refused \
         about 281k of 272k tokens; start a new conversation"
    );
}

#[test]
fn token_counts_read_as_a_person_says_them() {
    assert_eq!(kilo(950), "950");
    assert_eq!(kilo(3_240), "3.2k");
    assert_eq!(kilo(24_400), "24k");
    assert_eq!(kilo(281_000), "281k");
}
