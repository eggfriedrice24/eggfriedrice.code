use std::collections::HashMap;

use efr_protocol::{
    CallId, Compaction, CompactionId, CompactionTrigger, ContextPartKind, ConversationId, Event,
    EventEnvelope, JobInfo, JobState, Seq, TurnId, Usage,
};
use efr_provider::{ContentBlock, Message, Role};
use efr_stdx::id::uuid_v7;
use efr_stdx::time::Clock as _;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{cache, call_label, counted, measure};
use crate::compaction::{Placed, Window, request_tokens, summary_message, with_window};
use crate::context::PRUNED_OUTPUT_STUB;
use crate::notice::{NoticeBody, notice_text};
use crate::testing::{Setup, request, result_message, tool_message};

/// An item of the largest as a test compares it: its kind, turn, tool and label.
type Shown<'a> = (ContextPartKind, Option<u32>, Option<&'a str>, Option<&'a str>);

fn turn(n: u64) -> TurnId {
    TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(n)))
}

/// The job `j<number>` that ran `command` and exited.
fn job(number: u32, command: &str) -> JobInfo {
    let call = CallId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(9)));
    let conversation = ConversationId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(1)));
    let mut job = crate::testing::job_info(conversation, turn(2), call, number, command);
    job.state = JobState::Exited;
    job
}

/// A manual compaction that wrote a summary.
fn compaction() -> Compaction {
    Compaction {
        compaction_id: CompactionId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(11))),
        turn_id: None,
        trigger: CompactionTrigger::Manual,
        focus: None,
        model: "gpt-5.5".to_owned(),
        window: 100_000,
        limit: 76_000,
        tokens_before: 60_000,
        tokens_after: 9_000,
        through_turn: turn(2),
        through_message: None,
        kept_turns: 1,
        pruned_outputs: 0,
        pruned_tokens: 0,
        omitted_turns: 0,
        omitted_messages: 0,
        fresh: None,
        summary: Some("Done.".to_owned()),
        usage: None,
    }
}

fn placed(turn: TurnId, index: u32, message: Message) -> Placed {
    Placed { turn, index, message }
}

/// The number of each turn of `turns`, from 1.
fn numbers(turns: &[TurnId]) -> HashMap<TurnId, u32> {
    turns.iter().copied().zip(1..).collect()
}

/// A prompt as a turn sends it: its preamble, then its text.
fn prompt(setup: &Setup, text: &str) -> Message {
    setup.prompt(&setup.live_state(&setup.cwd, "one"), text)
}

/// A window of two turns: a prompt with its preamble, a shell call and its output, a
/// read and its output, and an answer with provider items.
fn two_turns(setup: &Setup) -> (Window, [TurnId; 2]) {
    let (one, two) = (turn(2), turn(3));
    let mut answer = Message::assistant("All 12 tests pass.");
    answer.provider_raw = Some(json!([{ "id": "msg_1", "encrypted": "x".repeat(300) }]));
    let window = Window {
        head: Vec::new(),
        placed: vec![
            placed(one, 0, prompt(setup, "run the tests")),
            placed(one, 1, tool_message("call_1", "shell", &json!({ "command": "cargo test" }))),
            placed(one, 2, result_message("call_1", &"test ok\n".repeat(400), false)),
            placed(one, 3, answer),
            placed(two, 0, prompt(setup, "read the run file")),
            placed(two, 1, tool_message("call_2", "read_file", &json!({ "path": "src/run.rs" }))),
            placed(two, 2, result_message("call_2", &"fn main() {}\n".repeat(100), false)),
            placed(two, 3, Message::assistant("It starts the daemon.")),
        ],
        omitted: 0,
    };
    (window, [one, two])
}

fn part(measured: &super::Measured, kind: ContextPartKind) -> (u64, u32) {
    measured
        .parts
        .iter()
        .find(|part| part.kind == kind)
        .map_or((0, 0), |part| (part.tokens, part.count))
}

#[test]
fn parts_sum_to_the_request_estimate() {
    let setup = Setup::new();
    let (window, turns) = two_turns(&setup);
    let base = request(Vec::new());

    let measured = measure(&base, &window, &numbers(&turns));

    let whole = request_tokens(&with_window(&base, &window));
    assert_eq!(measured.estimate, whole, "the estimate is the turn's own estimate");
    assert_eq!(measured.parts.iter().map(|part| part.tokens).sum::<u64>(), whole);
    let kinds: Vec<ContextPartKind> = measured.parts.iter().map(|part| part.kind).collect();
    assert_eq!(
        kinds,
        [
            ContextPartKind::System,
            ContextPartKind::Tools,
            ContextPartKind::Prompts,
            ContextPartKind::Preambles,
            ContextPartKind::Answers,
            ContextPartKind::ToolCalls,
            ContextPartKind::ToolOutputs,
        ],
        "a kind with nothing in it is left out"
    );
    assert_eq!(part(&measured, ContextPartKind::Prompts).1, 2);
    assert_eq!(part(&measured, ContextPartKind::Preambles).1, 2);
    assert_eq!(part(&measured, ContextPartKind::Answers).1, 2);
    assert_eq!(part(&measured, ContextPartKind::ToolCalls).1, 2);
    assert_eq!(part(&measured, ContextPartKind::ToolOutputs).1, 2);
    assert_eq!(part(&measured, ContextPartKind::System).1, 1);
    assert_eq!(part(&measured, ContextPartKind::Tools).1, u32::try_from(base.tools.len()).unwrap());
    assert!(
        part(&measured, ContextPartKind::ToolOutputs).0
            > part(&measured, ContextPartKind::Answers).0
    );
}

#[test]
fn pruned_outputs_count_as_pruned() {
    let setup = Setup::new();
    let (mut window, turns) = two_turns(&setup);
    for item in &mut window.placed {
        for block in &mut item.message.content {
            if let ContentBlock::ToolResult { call_id, output, .. } = block
                && call_id == "call_1"
            {
                PRUNED_OUTPUT_STUB.clone_into(output);
            }
        }
    }

    let measured = measure(&request(Vec::new()), &window, &numbers(&turns));

    assert_eq!(part(&measured, ContextPartKind::Pruned).1, 1);
    assert_eq!(part(&measured, ContextPartKind::ToolOutputs).1, 1, "the read keeps its output");
    let pruned = part(&measured, ContextPartKind::Pruned).0;
    assert!(pruned > 0 && pruned < 100, "only the stub counts: {pruned}");
}

#[test]
fn the_summary_and_the_fresh_block_count_as_the_summary() {
    let setup = Setup::new();
    let (mut window, turns) = two_turns(&setup);
    window.head =
        vec![Message::user("<fresh-context>\n</fresh-context>"), summary_message("Done.")];

    let measured = measure(&request(Vec::new()), &window, &numbers(&turns));

    assert_eq!(part(&measured, ContextPartKind::Summary).1, 2);
    assert!(
        measured.largest.iter().all(|item| item.kind != ContextPartKind::Summary),
        "the small head is not among the largest"
    );
}

#[test]
fn largest_lists_five_items_with_labels() {
    let setup = Setup::new();
    let (mut window, turns) = two_turns(&setup);
    let three = turn(4);
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n*** Add File: src/b.rs\n+b\n*** End Patch\n";
    let notice = notice_text(
        &job(3, "cargo build"),
        "$SCRATCH/jobs/j3.out",
        NoticeBody::Ended {
            state: JobState::Exited,
            exit_code: Some(0),
            stopped_by: None,
            tail: &"compiling\n".repeat(40),
        },
    );
    window.placed.extend([
        placed(three, 0, Message::new(Role::User, vec![ContentBlock::Text { text: notice }])),
        placed(three, 1, tool_message("call_3", "apply_patch", &json!(patch))),
        placed(three, 2, result_message("call_3", &"applied\n".repeat(2_500), false)),
    ]);
    let read = result_message("call_2", &"fn main() {}\n".repeat(800), false);
    window.placed[6] = placed(turns[1], 2, read);
    let mut turns = turns.to_vec();
    turns.push(three);

    let measured = measure(&request(Vec::new()), &window, &numbers(&turns));

    let largest: Vec<Shown<'_>> = measured
        .largest
        .iter()
        .map(|item| (item.kind, item.turn, item.tool.as_deref(), item.label.as_deref()))
        .collect();
    assert_eq!(
        largest[..3],
        [
            (
                ContextPartKind::ToolOutputs,
                Some(3),
                Some("apply_patch"),
                Some("src/a.rs, src/b.rs")
            ),
            (ContextPartKind::ToolOutputs, Some(2), Some("read_file"), Some("src/run.rs")),
            (ContextPartKind::ToolOutputs, Some(1), Some("shell"), Some("cargo test")),
        ]
    );
    assert_eq!(largest.len(), 5, "at most five");
    let tokens: Vec<u64> = measured.largest.iter().map(|item| item.tokens).collect();
    assert!(tokens.windows(2).all(|pair| pair[0] >= pair[1]), "largest first: {tokens:?}");
    assert_eq!(part(&measured, ContextPartKind::Notices).1, 1);
}

#[test]
fn a_notice_item_names_its_job() {
    let setup = Setup::new();
    let one = turn(2);
    let notice = notice_text(
        &job(7, "make"),
        "$SCRATCH/jobs/j7.out",
        NoticeBody::Ended {
            state: JobState::Exited,
            exit_code: Some(2),
            stopped_by: None,
            tail: "",
        },
    );
    let window = Window {
        head: Vec::new(),
        placed: vec![placed(
            one,
            0,
            Message::new(
                Role::User,
                vec![
                    ContentBlock::Text { text: setup.live_state(&setup.cwd, "x").render() },
                    ContentBlock::Text { text: notice },
                ],
            ),
        )],
        omitted: 0,
    };

    let measured = measure(&request(Vec::new()), &window, &numbers(&[one]));

    let item = measured
        .largest
        .iter()
        .find(|item| item.kind == ContextPartKind::Notices)
        .expect("the notice is an item");
    assert_eq!(item.label.as_deref(), Some("j7"));
    assert_eq!(part(&measured, ContextPartKind::Preambles).1, 1, "a wake turn has a preamble");
    assert_eq!(part(&measured, ContextPartKind::Prompts).1, 0, "and no prompt");
}

#[test]
fn a_label_is_one_short_line() {
    let long = "x".repeat(300);
    let label = call_label("shell", &json!({ "command": format!("{long}\nsecond") })).unwrap();
    assert_eq!(label, format!("{} ...", "x".repeat(120)));
    assert_eq!(
        call_label("shell", &json!({ "command": "make\nmake install" })).as_deref(),
        Some("make ...")
    );
    assert_eq!(
        call_label("write_file", &json!({ "path": "a.txt", "content": "secret" })).as_deref(),
        Some("a.txt")
    );
    assert_eq!(call_label("settings", &json!({ "changes": [] })), None);
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

fn completed(turn_id: TurnId, usage: Option<Usage>) -> Event {
    Event::TurnCompleted {
        turn_id,
        usage,
        context: Some(efr_protocol::ContextUse { tokens: 9_000, limit: 76_000, window: 100_000 }),
        changes: None,
        stop: None,
        undo: None,
    }
}

#[test]
fn cache_block_sums_the_conversation() {
    let first = Usage {
        input_tokens: 20_000,
        output_tokens: 500,
        cached_input_tokens: 0,
        reasoning_tokens: 0,
        cache_write_tokens: 20_000,
        cache_write_1h_tokens: 20_000,
        context_tokens: 20_500,
    };
    let second = Usage {
        input_tokens: 21_000,
        output_tokens: 700,
        cached_input_tokens: 20_000,
        reasoning_tokens: 100,
        cache_write_tokens: 1_000,
        cache_write_1h_tokens: 1_000,
        context_tokens: 21_700,
    };
    let ends = [
        envelope(3, completed(turn(2), Some(first))),
        envelope(5, completed(turn(3), None)),
        envelope(9, completed(turn(4), Some(second))),
    ];
    let mut compaction = compaction();
    compaction.usage = Some(Usage::new(4_000, 300));
    let summary = cache(&ends, &[compaction]);

    assert_eq!(summary.last_turn, Some(second));
    assert_eq!(summary.turns, 2, "a turn without usage is not counted");
    assert_eq!(
        summary.conversation,
        Usage {
            input_tokens: 45_000,
            output_tokens: 1_500,
            cached_input_tokens: 20_000,
            reasoning_tokens: 100,
            cache_write_tokens: 21_000,
            cache_write_1h_tokens: 21_000,
            context_tokens: 0,
        },
        "the summary request counts too"
    );
}

#[test]
fn the_count_is_gone_after_a_compaction() {
    let usage = Usage { context_tokens: 9_100, ..Usage::new(9_000, 100) };
    let ended = envelope(3, completed(turn(2), Some(usage)));
    let count = counted(std::slice::from_ref(&ended)).expect("a count");
    assert_eq!((count.tokens, count.turn_id), (9_000, turn(2)));

    let compacted = envelope(4, Event::ConversationCompacted(compaction()));
    assert_eq!(counted(&[ended.clone(), compacted]), None);
    assert_eq!(counted(&[envelope(3, completed(turn(2), None))]), None, "no real count");
}
