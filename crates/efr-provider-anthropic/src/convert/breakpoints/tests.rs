use efr_provider::Role;
use pretty_assertions::assert_eq;

use super::{
    ANCHOR_STEP_TOKENS, Breakpoint, Layout, Shape, Slot, Target, Ttl, place_breakpoints, summary,
};
use crate::CacheTtl;

const H: Ttl = Ttl::OneHour;
const M: Ttl = Ttl::FiveMinutes;

/// A user message that opens a turn: a prompt, or the head after a compaction.
fn prompt(tokens: u64) -> Shape {
    Shape { role: Role::User, opens_turn: true, tokens }
}

/// A user message with tool results, maybe with a steer after them.
fn results(tokens: u64) -> Shape {
    Shape { role: Role::User, opens_turn: false, tokens }
}

fn answer(tokens: u64) -> Shape {
    Shape { role: Role::Assistant, opens_turn: false, tokens }
}

fn system(ttl: Ttl) -> Breakpoint {
    Breakpoint { slot: Slot::System, target: Target::System, ttl }
}

fn at(slot: Slot, index: usize, ttl: Ttl) -> Breakpoint {
    Breakpoint { slot, target: Target::Message(index), ttl }
}

fn place(messages: &[Shape], side_call: bool, ttl: CacheTtl) -> Vec<Breakpoint> {
    let layout = Layout { system: true, tools: true, messages, side_call };
    let marks = place_breakpoints(&layout, ttl);
    assert_rules(&marks);
    // A marker sits on a user message that ends its run, which the API joins into one.
    for mark in &marks {
        if let Target::Message(index) = mark.target {
            assert_eq!(messages[index].role, Role::User, "{marks:?}");
            let next = messages.get(index + 1).map(|shape| shape.role);
            assert_ne!(next, Some(Role::User), "{marks:?} in {messages:?}");
        }
    }
    marks
}

/// The rules of the API that every placement keeps: at most four markers, in the order
/// of the prefix, one per block, and no one-hour marker after a five-minute one.
fn assert_rules(marks: &[Breakpoint]) {
    assert!(marks.len() <= 4, "{marks:?}");
    let indexes: Vec<usize> = marks
        .iter()
        .filter_map(|mark| match mark.target {
            Target::Message(index) => Some(index),
            _ => None,
        })
        .collect();
    assert!(indexes.windows(2).all(|pair| pair[0] < pair[1]), "{marks:?}");
    let first_short = marks.iter().position(|mark| mark.ttl == Ttl::FiveMinutes);
    if let Some(first_short) = first_short {
        assert!(marks[first_short..].iter().all(|mark| mark.ttl == Ttl::FiveMinutes), "{marks:?}");
    }
}

#[test]
fn the_walkthrough_of_two_turns_with_a_tool_loop() {
    let turn_one = [prompt(1_000)];
    let call_two = [prompt(1_000), answer(500), results(300)];
    let call_three = [prompt(1_000), answer(500), results(300), answer(400), results(200)];
    let turn_two = [
        prompt(1_000),
        answer(500),
        results(300),
        answer(400),
        results(200),
        answer(100),
        prompt(800),
    ];
    let cases: [(&[Shape], Vec<Breakpoint>); 4] = [
        // The first call of a turn marks an anchor at its tail.
        (&turn_one, vec![system(H), at(Slot::Tail, 0, H)]),
        // The anchor is also the previous tail: one marker, one hour.
        (&call_two, vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Tail, 2, H)]),
        (
            &call_three,
            vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Previous, 2, H), at(Slot::Tail, 4, H)],
        ),
        // A new turn marks a new anchor: every marker is one hour.
        (
            &turn_two,
            vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Previous, 4, H), at(Slot::Tail, 6, H)],
        ),
    ];
    for (messages, expected) in cases {
        assert_eq!(place(messages, false, CacheTtl::Auto), expected, "{messages:?}");
    }
}

#[test]
fn the_walkthrough_of_a_steer_a_summary_and_a_compaction() {
    // A steer is a user message of its own after the results, in one run with them:
    // the run opens a turn, so the call that sends it marks an anchor. The results
    // never ended a request, so no marker goes on them.
    let steered = [prompt(1_000), answer(500), results(300), prompt(100)];
    // The summary request of a compaction inside a turn ends with the summary prompt,
    // a message of its own after the last results; a side call marks no anchor, and P
    // reads the results before, where the call before put its T.
    let summary = [
        prompt(1_000),
        answer(500),
        results(300),
        answer(400),
        results(200),
        answer(100),
        results(800),
        prompt(100),
    ];
    // The first call after a compaction inside a turn: the head (fresh block, summary)
    // is a user message for each part, one run that opens a turn, then the kept tail.
    let compacted_in_turn = [prompt(3_000), prompt(3_000), answer(300), results(200)];
    // After a compaction at the start of a turn the new prompt ends the run of the head.
    let compacted_at_start = [prompt(3_000), prompt(3_000), prompt(1_000)];
    let cases: [(&[Shape], bool, Vec<Breakpoint>); 4] = [
        (&steered, false, vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Tail, 3, H)]),
        (
            &summary,
            true,
            vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Previous, 4, H), at(Slot::Tail, 7, H)],
        ),
        (&compacted_in_turn, false, vec![system(H), at(Slot::Anchor, 1, H), at(Slot::Tail, 3, H)]),
        (&compacted_at_start, false, vec![system(H), at(Slot::Tail, 2, H)]),
    ];
    for (messages, side_call, expected) in cases {
        assert_eq!(place(messages, side_call, CacheTtl::Auto), expected, "{messages:?}");
    }
}

#[test]
fn a_prompt_after_a_turn_that_ended_on_a_user_message_marks_its_anchor() {
    // An interrupt while a tool ran: the turn ended on its results, and the new prompt
    // follows them as a message of its own.
    let interrupted = [prompt(1_000), answer(500), results(300), prompt(200)];
    // A model call that failed before an answer: the turn ended on its prompt.
    let failed = [prompt(1_000), answer(500), prompt(300), prompt(200)];
    // Each ends in a run of two user messages that opens a turn: the call marks an
    // anchor at the new prompt, and the anchor before is the earlier prompt, where the
    // last call that got an answer put its T.
    for messages in [&interrupted, &failed] {
        assert_eq!(
            place(messages, false, CacheTtl::Auto),
            vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Tail, 3, H)],
            "{messages:?}"
        );
    }
    // The next call of the new turn finds the anchor at the new prompt.
    let next = [prompt(1_000), answer(500), results(300), prompt(200), answer(100), results(100)];
    assert_eq!(
        place(&next, false, CacheTtl::Auto),
        vec![system(H), at(Slot::Anchor, 3, H), at(Slot::Tail, 5, H)]
    );
}

#[test]
fn a_long_tool_loop_moves_its_anchor_forward() {
    // Every call of a long tool loop: the messages after A never hold more than the
    // step and the call that crossed it.
    let mut messages = vec![prompt(1_000)];
    for _ in 0..40 {
        messages.push(answer(1_500));
        messages.push(results(1_500));
        let marks = place(&messages, false, CacheTtl::Auto);
        let anchor = marks
            .iter()
            .find(|mark| mark.slot == Slot::Anchor)
            .and_then(|mark| match mark.target {
                Target::Message(index) => Some(index),
                _ => None,
            })
            .unwrap();
        let after: u64 = messages[anchor + 1..].iter().map(|shape| shape.tokens).sum();
        assert!(after <= ANCHOR_STEP_TOKENS + 3_000, "{after} after the anchor at {anchor}");
    }
}

#[test]
fn a_tool_loop_marks_a_new_anchor_once_it_grows_past_the_step() {
    // 18,000 tokens after the anchor at the first results, more than the step at the
    // second ones.
    let grown = [prompt(1_000), answer(15_000), results(3_000), answer(2_000), results(1_000)];
    assert!((18_000..21_000).contains(&ANCHOR_STEP_TOKENS));
    assert_eq!(
        place(&grown, false, CacheTtl::Auto),
        vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Previous, 2, H), at(Slot::Tail, 4, H)]
    );
    // The next call finds the anchor that the call before marked.
    let next = [
        prompt(1_000),
        answer(15_000),
        results(3_000),
        answer(2_000),
        results(1_000),
        answer(500),
        results(500),
    ];
    assert_eq!(
        place(&next, false, CacheTtl::Auto),
        vec![system(H), at(Slot::Anchor, 4, H), at(Slot::Tail, 6, H)]
    );
}

#[test]
fn a_side_call_never_marks_an_anchor() {
    // A summary request after a finished turn ends with a text message, like a prompt.
    let summary = [prompt(1_000), answer(500), prompt(200)];
    assert_eq!(
        place(&summary, true, CacheTtl::Auto),
        vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Tail, 2, H)]
    );
    let big = [prompt(1_000), answer(50_000), results(200)];
    assert_eq!(
        place(&big, true, CacheTtl::Auto),
        vec![system(H), at(Slot::Anchor, 0, H), at(Slot::Tail, 2, H)]
    );
}

#[test]
fn a_fixed_time_to_live_puts_one_time_on_every_marker() {
    let messages = [prompt(1_000), answer(500), results(300), answer(400), results(200)];
    for (ttl, time) in [(CacheTtl::FiveMinutes, M), (CacheTtl::OneHour, H)] {
        assert_eq!(
            place(&messages, false, ttl),
            vec![
                system(time),
                at(Slot::Anchor, 0, time),
                at(Slot::Previous, 2, time),
                at(Slot::Tail, 4, time),
            ]
        );
    }
}

#[test]
fn the_system_marker_falls_back_to_the_last_tool_or_to_nothing() {
    let messages = [prompt(10)];
    let tools_only = Layout { system: false, tools: true, messages: &messages, side_call: false };
    assert_eq!(
        place_breakpoints(&tools_only, CacheTtl::Auto),
        vec![
            Breakpoint { slot: Slot::System, target: Target::LastTool, ttl: H },
            at(Slot::Tail, 0, H),
        ]
    );
    let bare = Layout { system: false, tools: false, messages: &messages, side_call: false };
    assert_eq!(place_breakpoints(&bare, CacheTtl::Auto), vec![at(Slot::Tail, 0, H)]);
}

#[test]
fn a_body_without_a_user_tail_marks_only_the_system() {
    assert_eq!(place(&[], false, CacheTtl::Auto), vec![system(H)]);
    assert_eq!(place(&[prompt(10), answer(10)], false, CacheTtl::Auto), vec![system(H)]);
}

#[test]
fn every_placement_keeps_the_rules_of_the_api() {
    // Every history of up to four calls, each with a small or a large tool loop, with
    // or without a new prompt, and with a run of two user messages (a steer after the
    // results, or a prompt after a turn that ended early), under every time to live
    // and as a side call.
    let kinds: [&[Shape]; 6] = [
        &[results(100)],
        &[results(25_000)],
        &[prompt(100)],
        &[prompt(25_000)],
        &[results(100), prompt(100)],
        &[prompt(100), prompt(25_000)],
    ];
    let mut histories: Vec<Vec<Shape>> = vec![vec![prompt(100)]];
    for _ in 0..3 {
        let mut longer = Vec::new();
        for history in &histories {
            for kind in kinds {
                let mut next = history.clone();
                next.push(answer(300));
                next.extend_from_slice(kind);
                longer.push(next);
            }
        }
        histories.extend(longer);
    }
    for history in &histories {
        for ttl in [CacheTtl::Auto, CacheTtl::FiveMinutes, CacheTtl::OneHour] {
            for side_call in [false, true] {
                place(history, side_call, ttl);
            }
        }
    }
}

#[test]
fn a_ttl_is_written_as_the_api_names_it() {
    assert_eq!(Ttl::FiveMinutes.as_str(), "5m");
    assert_eq!(Ttl::OneHour.as_str(), "1h");
}

#[test]
fn the_log_names_each_place_with_its_time() {
    let call = [prompt(1_000), answer(500), results(300), answer(200), results(300)];
    assert_eq!(summary(&place(&call, false, CacheTtl::Auto)), "S1h,A1h,P1h,T1h");
    assert_eq!(summary(&place(&call[..1], false, CacheTtl::Auto)), "S1h,T1h");
    assert_eq!(summary(&place(&call, false, CacheTtl::FiveMinutes)), "S5m,A5m,P5m,T5m");
    let bare = Layout { system: false, tools: false, messages: &[], side_call: false };
    assert_eq!(summary(&place_breakpoints(&bare, CacheTtl::Auto)), "none");
}
