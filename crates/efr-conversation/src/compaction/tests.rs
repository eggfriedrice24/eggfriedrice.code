use efr_protocol::TurnId;
use efr_provider::{ContentBlock, Message, Role};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    Cut, Placed, SUMMARY_CLOSE, SUMMARY_OPEN, SUMMARY_PROMPT, cut_before, message_tokens,
    minimal_tail, prune, summary_message, summary_prompt, tail_start,
};
use crate::context::{PRUNE_KEEP_TOKENS, PRUNE_MIN_TOKENS, PRUNED_OUTPUT_STUB, TAIL_TOKENS};

/// The headings that the summary prompt asks for, in their order: the record format of
/// a compaction and of the handoff of milestone 4.
const SECTIONS: [&str; 7] = [
    "## Task and state",
    "## Decisions",
    "## Important details",
    "## Files and places",
    "## Open work and next step",
    "## Shell and directories",
    "## System tasks",
];

fn turn(n: u64) -> TurnId {
    format!("00000002-0000-7000-8000-{n:012x}").parse().expect("turn id")
}

fn at(turn_number: u64, index: u32, message: Message) -> Placed {
    Placed { turn: turn(turn_number), index, message }
}

fn call(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolCall {
            call_id: id.to_owned(),
            name: "shell".to_owned(),
            input: json!({"command": "ls"}),
            freeform: false,
        }],
    )
}

/// A result whose output has `bytes` bytes.
fn result(id: &str, bytes: usize) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            call_id: id.to_owned(),
            output: "x".repeat(bytes),
            is_error: false,
        }],
    )
}

fn outputs(placed: &[Placed]) -> Vec<usize> {
    placed
        .iter()
        .flat_map(|placed| &placed.message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { output, .. } => Some(output.len()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_summary_prompt_asks_for_every_section_in_order() {
    let mut from = 0;
    for section in SECTIONS {
        let found = SUMMARY_PROMPT[from..].find(&format!("\n{section}\n"));
        let Some(found) = found else {
            panic!("the prompt lacks {section:?} after byte {from}");
        };
        from += found + section.len();
    }
    assert_eq!(SUMMARY_PROMPT.matches("\n## ").count(), SECTIONS.len());
    for rule in
        ["Write `None.`", "Write facts, not a story", "about 2000 words", "Do not call tools"]
    {
        assert!(SUMMARY_PROMPT.contains(rule), "the prompt lacks {rule:?}");
    }
    assert!(!SUMMARY_PROMPT.contains(['\u{2013}', '\u{2014}']), "no en or em dashes");
}

#[test]
fn a_focus_ends_the_summary_prompt_and_a_blank_one_counts_as_none() {
    assert_eq!(summary_prompt(None), SUMMARY_PROMPT.trim_end());
    assert_eq!(summary_prompt(Some("  ")), SUMMARY_PROMPT.trim_end());
    assert_eq!(
        summary_prompt(Some(" the failing test ")),
        format!("{}\n\nKeep in the summary: the failing test", SUMMARY_PROMPT.trim_end())
    );
}

#[test]
fn the_summary_is_a_marked_user_message() {
    assert_eq!(
        summary_message("## Task and state\nFree /var.\n"),
        Message::user(format!("{SUMMARY_OPEN}\n## Task and state\nFree /var.\n{SUMMARY_CLOSE}"))
    );
}

#[test]
fn pruning_stubs_the_old_results_and_keeps_the_newest_forty_thousand_tokens() {
    // Four results of about 25k tokens each: the newest one fits in the kept part, the
    // second newest passes it.
    let placed = vec![
        at(1, 0, Message::user("one")),
        at(1, 1, call("a")),
        at(1, 2, result("a", 100_000)),
        at(1, 3, call("b")),
        at(1, 4, result("b", 100_000)),
        at(2, 0, Message::user("two")),
        at(2, 1, call("c")),
        at(2, 2, result("c", 100_000)),
        at(2, 3, call("d")),
        at(2, 4, result("d", 100_000)),
    ];

    let pruning = prune(&placed);

    let stub = PRUNED_OUTPUT_STUB.len();
    assert_eq!(outputs(&pruning.placed), [stub, stub, stub, 100_000]);
    assert_eq!(pruning.outputs, 3);
    assert!(pruning.worth_it());
    assert!(pruning.tokens > 3 * 24_000 && pruning.tokens < 3 * 25_100, "{}", pruning.tokens);
    assert_eq!(pruning.cut, Some(Cut { through_turn: turn(2), through_message: Some(3) }));
    assert_eq!(pruning.after, 8);
    assert!(message_tokens(&placed[9].message) <= PRUNE_KEEP_TOKENS);
}

#[test]
fn a_pruning_that_frees_less_than_twenty_thousand_tokens_is_not_worth_it() {
    let mut placed = vec![at(1, 0, Message::user("one"))];
    for (index, id) in ["a", "b", "c"].into_iter().enumerate() {
        let index = u32::try_from(index).expect("small") * 2;
        placed.push(at(1, index + 1, call(id)));
        placed.push(at(1, index + 2, result(id, 20_000)));
    }
    placed.push(at(2, 0, Message::user("two")));
    placed.push(at(2, 1, call("d")));
    placed.push(at(2, 2, result("d", 170_000)));

    let pruning = prune(&placed);

    assert_eq!(pruning.outputs, 3, "every old result reads the stub");
    assert!(pruning.tokens < PRUNE_MIN_TOKENS, "{}", pruning.tokens);
    assert!(!pruning.worth_it());
}

#[test]
fn the_newest_call_and_its_result_are_never_pruned_however_large() {
    let placed = vec![
        at(1, 0, Message::user("one")),
        at(1, 1, call("a")),
        at(1, 2, result("a", 100_000)),
        at(1, 3, call("b")),
        at(1, 4, result("b", 400_000)),
    ];

    let pruning = prune(&placed);

    assert_eq!(outputs(&pruning.placed), [PRUNED_OUTPUT_STUB.len(), 400_000]);
    assert_eq!(pruning.cut, Some(Cut { through_turn: turn(1), through_message: Some(3) }));
}

#[test]
fn nothing_pruned_has_no_cut() {
    let placed = vec![at(1, 0, Message::user("one")), at(1, 1, Message::assistant("hi"))];

    let pruning = prune(&placed);

    assert_eq!(pruning.placed, placed);
    assert_eq!((pruning.outputs, pruning.tokens, pruning.cut, pruning.after), (0, 0, None, 0));
}

#[test]
fn the_tail_keeps_the_newest_twenty_thousand_tokens_and_never_starts_with_results() {
    let placed = vec![
        at(1, 0, Message::user("one")),
        at(1, 1, call("a")),
        at(1, 2, result("a", 40_000)),
        at(1, 3, Message::assistant("first done")),
        at(2, 0, Message::user("two")),
        at(2, 1, call("b")),
        at(2, 2, result("b", 30_000)),
        at(2, 3, call("c")),
        at(2, 4, result("c", 30_000)),
    ];
    let last_two: u64 = placed[2..].iter().map(|placed| message_tokens(&placed.message)).sum();
    assert!(last_two > TAIL_TOKENS, "the budget ends inside the second result of turn 1");

    let start = tail_start(&placed);

    // The budget ends before the result of `a`, so the tail starts after it.
    assert_eq!(start, 3);
    assert_eq!(
        cut_before(&placed, start),
        Some(Cut { through_turn: turn(1), through_message: Some(3) })
    );
}

#[test]
fn a_result_that_starts_the_budget_takes_its_call_into_the_tail() {
    // The result of `a` fills the budget to the token, so its call does not fit.
    let newest: u64 = message_tokens(&call("b")) + message_tokens(&result("b", 40_000));
    let overhead = serde_json::to_vec(&result("a", 0)).expect("json").len();
    let room = usize::try_from(TAIL_TOKENS - newest).expect("small") * 4 - overhead;
    let placed = vec![
        at(1, 0, Message::user("one")),
        at(1, 1, Message::assistant("hello")),
        at(2, 0, Message::user("two")),
        at(2, 1, call("a")),
        at(2, 2, result("a", room)),
        at(2, 3, call("b")),
        at(2, 4, result("b", 40_000)),
    ];
    let fitting: u64 = placed[4..].iter().map(|placed| message_tokens(&placed.message)).sum();
    assert_eq!(fitting, TAIL_TOKENS);

    let start = tail_start(&placed);

    assert_eq!(start, 3, "the call of the result that starts the budget comes along");
    assert_eq!(
        cut_before(&placed, start),
        Some(Cut { through_turn: turn(2), through_message: Some(1) })
    );
}

#[test]
fn the_tail_holds_at_least_the_newest_call_with_its_result() {
    let placed = vec![
        at(1, 0, Message::user("one")),
        at(1, 1, Message::assistant("hello")),
        at(2, 0, Message::user("two")),
        at(2, 1, call("a")),
        at(2, 2, result("a", 200_000)),
    ];

    assert_eq!(minimal_tail(&placed), 3);
    assert_eq!(tail_start(&placed), 3);
}

#[test]
fn the_tail_holds_at_least_the_newest_prompt_and_its_turn_starts_after_the_cut() {
    let placed = vec![
        at(1, 0, Message::user("one")),
        at(1, 1, Message::assistant("hello")),
        at(2, 0, Message::user("y".repeat(200_000))),
    ];

    let start = tail_start(&placed);

    assert_eq!(start, 2);
    assert_eq!(
        cut_before(&placed, start),
        Some(Cut { through_turn: turn(1), through_message: None })
    );
}

#[test]
fn when_everything_fits_in_the_tail_nothing_lies_before_the_cut() {
    let placed = vec![at(1, 0, Message::user("one")), at(1, 1, Message::assistant("hello"))];

    assert_eq!(tail_start(&placed), 0);
    assert_eq!(cut_before(&placed, 0), None);
}

#[test]
fn a_cut_covers_the_turns_before_it_and_the_messages_before_its_message() {
    let order = |id: TurnId| (1..=3).map(turn).position(|turn| turn == id);
    let inside = Cut { through_turn: turn(2), through_message: Some(2) };
    let whole = Cut { through_turn: turn(2), through_message: None };
    let hello = Message::assistant("x");

    assert!(inside.covers(&at(1, 5, hello.clone()), order));
    assert!(inside.covers(&at(2, 1, hello.clone()), order));
    assert!(!inside.covers(&at(2, 2, hello.clone()), order));
    assert!(!inside.covers(&at(3, 0, hello.clone()), order));
    assert!(whole.covers(&at(2, 9, hello.clone()), order));
    assert!(!whole.covers(&at(3, 0, hello.clone()), order));
    assert!(!whole.covers(&at(7, 0, hello), order), "an unknown turn counts as after");
}
