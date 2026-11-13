use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{ConversationId, DraftPart, Seq, TurnId};
use efr_provider::{CompletionBuilder, ProviderEvent};
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use tokio::sync::broadcast;

use super::Drafter;
use crate::drafts::{LiveStatus, bold_title};
use crate::{ConversationDraft, LiveToolInput, draft_channel};

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";

/// A drafter that sends every change at once, its channel, and a builder.
fn drafter() -> (Drafter, broadcast::Sender<ConversationDraft>, CompletionBuilder) {
    let sender = draft_channel();
    let conversation_id: ConversationId = CONVERSATION.parse().unwrap();
    let turn_id: TurnId = TURN.parse().unwrap();
    let drafter =
        Drafter::new(sender.clone(), Arc::default(), conversation_id, turn_id, Duration::ZERO);
    (drafter, sender, CompletionBuilder::new())
}

/// Feeds `event` to the builder and the drafter, as the turn does.
fn feed(drafter: &mut Drafter, builder: &mut CompletionBuilder, event: &ProviderEvent) {
    builder.push(event).unwrap();
    if drafter.push(event) {
        drafter.offer(Timestamp::UNIX_EPOCH);
    }
}

fn parts(receiver: &mut broadcast::Receiver<ConversationDraft>) -> Vec<DraftPart> {
    let mut parts = Vec::new();
    while let Ok(draft) = receiver.try_recv() {
        parts.push(draft.part);
    }
    parts
}

fn text(text: &str) -> ProviderEvent {
    ProviderEvent::TextDelta { text: text.to_owned() }
}

fn reasoning(text: &str) -> ProviderEvent {
    ProviderEvent::ReasoningDelta { text: text.to_owned() }
}

#[test]
fn a_bold_line_is_a_title_and_other_lines_are_not() {
    assert_eq!(bold_title("**Reading the logs**"), Some("Reading the logs"));
    assert_eq!(bold_title("  ** Spaced **  "), Some("Spaced"));
    assert_eq!(bold_title("**"), None);
    assert_eq!(bold_title("****"), None);
    assert_eq!(bold_title("**Half"), None);
    assert_eq!(bold_title("Plain text"), None);
    assert_eq!(bold_title("**One** and **two**"), None, "two bold runs are no title");
}

#[test]
fn a_title_split_over_deltas_is_found_once_its_line_is_whole() {
    let (mut drafter, sender, mut builder) = drafter();
    let mut receiver = sender.subscribe();
    feed(&mut drafter, &mut builder, &reasoning("**Checking"));
    feed(&mut drafter, &mut builder, &reasoning(" the disk**\n\nThe disk is"));
    feed(&mut drafter, &mut builder, &reasoning(" full.\n\n**Cleaning up**"));
    let titles: Vec<Option<String>> = parts(&mut receiver)
        .into_iter()
        .map(|part| match part {
            DraftPart::Reasoning { title, .. } => title,
            other => panic!("only reasoning: {other:?}"),
        })
        .collect();
    assert_eq!(
        titles,
        vec![None, Some("Checking the disk".to_owned()), Some("Cleaning up".to_owned())]
    );
}

#[test]
fn the_reasoning_of_a_new_model_call_drops_the_title_of_the_last_one() {
    let (mut drafter, sender, mut builder) = drafter();
    let mut receiver = sender.subscribe();
    feed(&mut drafter, &mut builder, &reasoning("**Checking the build**\n\nRun it."));
    drafter.begin_call(0);
    let mut builder = CompletionBuilder::new();
    feed(&mut drafter, &mut builder, &reasoning("**Reading"));
    feed(&mut drafter, &mut builder, &reasoning(" the error**\n"));
    let titles: Vec<Option<String>> = parts(&mut receiver)
        .into_iter()
        .map(|part| match part {
            DraftPart::Reasoning { title, .. } => title,
            other => panic!("only reasoning: {other:?}"),
        })
        .collect();
    assert_eq!(
        titles,
        vec![Some("Checking the build".to_owned()), None, Some("Reading the error".to_owned())]
    );
}

#[test]
fn text_drafts_follow_the_joined_text_of_the_message() {
    let (mut drafter, sender, mut builder) = drafter();
    let mut receiver = sender.subscribe();
    feed(&mut drafter, &mut builder, &text("First."));
    feed(&mut drafter, &mut builder, &reasoning("thinking"));
    feed(&mut drafter, &mut builder, &text("Second."));
    let texts: Vec<DraftPart> = parts(&mut receiver)
        .into_iter()
        .filter(|part| matches!(part, DraftPart::Text { .. }))
        .collect();
    // The builder joins text blocks with a blank line, as the updates and the
    // completion do, so the offsets of drafts and updates agree.
    assert_eq!(
        texts,
        vec![
            DraftPart::Text { index: 0, offset: 0, delta: "First.".to_owned() },
            DraftPart::Text { index: 0, offset: 6, delta: "\n\nSecond.".to_owned() },
        ]
    );
}

#[test]
fn tool_input_counts_bytes_per_call_and_the_end_sets_the_whole_size() {
    let (mut drafter, sender, mut builder) = drafter();
    let mut receiver = sender.subscribe();
    let start = |id: &str, name: &str| ProviderEvent::ToolCallStart {
        call_id: id.to_owned(),
        name: name.to_owned(),
        freeform: false,
    };
    let delta = |id: &str, arguments: &str| ProviderEvent::ToolCallDelta {
        call_id: id.to_owned(),
        arguments: arguments.to_owned(),
    };
    feed(&mut drafter, &mut builder, &start("a", "shell"));
    feed(&mut drafter, &mut builder, &start("b", "write_file"));
    feed(&mut drafter, &mut builder, &delta("b", "{\"path\""));
    feed(&mut drafter, &mut builder, &delta("a", "{}"));
    let end = ProviderEvent::ToolCallEnd { call_id: "b".to_owned(), arguments: "{}".to_owned() };
    feed(&mut drafter, &mut builder, &end);
    let input = |call: u32, tool: &str, bytes: u64, tail: Option<&str>| DraftPart::ToolInput {
        call,
        tool: tool.to_owned(),
        bytes,
        tail: tail.map(str::to_owned),
    };
    assert_eq!(
        parts(&mut receiver),
        vec![
            input(0, "shell", 0, None),
            input(1, "write_file", 0, None),
            input(1, "write_file", 7, Some("{\"path\"")),
            input(0, "shell", 2, Some("{}")),
            input(1, "write_file", 2, Some("{}")),
        ]
    );
}

#[test]
fn without_a_listener_nothing_moves_and_the_next_listener_gets_it_all() {
    let (mut drafter, sender, mut builder) = drafter();
    feed(&mut drafter, &mut builder, &text("Hello"));
    feed(&mut drafter, &mut builder, &text(" world"));
    let mut receiver = sender.subscribe();
    feed(&mut drafter, &mut builder, &text("!"));
    assert_eq!(
        parts(&mut receiver),
        vec![DraftPart::Text { index: 0, offset: 0, delta: "Hello world!".to_owned() }]
    );
}

#[test]
fn a_new_model_call_starts_its_text_and_its_calls_again() {
    let (mut drafter, sender, mut builder) = drafter();
    let mut receiver = sender.subscribe();
    feed(&mut drafter, &mut builder, &text("One."));
    drafter.begin_call(1);
    let mut builder = CompletionBuilder::new();
    builder.push(&text("Two.")).unwrap();
    assert!(drafter.push(&text("Two.")));
    drafter.offer(Timestamp::UNIX_EPOCH);
    assert_eq!(
        parts(&mut receiver),
        vec![
            DraftPart::Text { index: 0, offset: 0, delta: "One.".to_owned() },
            DraftPart::Text { index: 1, offset: 0, delta: "Two.".to_owned() },
        ]
    );
}

#[test]
fn events_without_a_draft_part_change_nothing() {
    let (mut drafter, _sender, _builder) = drafter();
    assert!(!drafter.push(&text("")));
    assert!(!drafter.push(&reasoning("")));
    assert!(!drafter.push(&ProviderEvent::Raw(serde_json::json!({}))));
    let unknown =
        ProviderEvent::ToolCallDelta { call_id: "nobody".to_owned(), arguments: "x".to_owned() };
    assert!(!drafter.push(&unknown), "a call that never started");
}

/// A drafter that sends every change at once, with the status it writes its parts to.
fn drafter_with_status() -> (Drafter, Arc<LiveStatus>, broadcast::Sender<ConversationDraft>) {
    let sender = draft_channel();
    let status = Arc::new(LiveStatus::default());
    let drafter = Drafter::new(
        sender.clone(),
        Arc::clone(&status),
        CONVERSATION.parse().unwrap(),
        TURN.parse().unwrap(),
        Duration::ZERO,
    );
    (drafter, status, sender)
}

fn start(id: &str, name: &str) -> ProviderEvent {
    ProviderEvent::ToolCallStart { call_id: id.to_owned(), name: name.to_owned(), freeform: false }
}

#[test]
fn the_live_turn_holds_the_parts_also_without_a_listener() {
    let (mut drafter, status, _sender) = drafter_with_status();
    assert_eq!(status.live_turn(), None, "no model call started");
    drafter.begin_call(0);
    let mut builder = CompletionBuilder::new();
    feed(&mut drafter, &mut builder, &reasoning("**Looking** around\n**Reading the logs**\n"));
    feed(&mut drafter, &mut builder, &text("Grüße, "));
    feed(&mut drafter, &mut builder, &text("世界 😀"));
    feed(&mut drafter, &mut builder, &start("a", "shell"));
    let arguments = "{\"c".to_owned();
    feed(
        &mut drafter,
        &mut builder,
        &ProviderEvent::ToolCallDelta { call_id: "a".to_owned(), arguments },
    );
    drafter.recorded(Seq::new(7));

    let live = status.live_turn().unwrap();
    assert_eq!(live.turn_id, TURN.parse().unwrap());
    assert_eq!(live.after_seq, Seq::new(7));
    assert_eq!(live.index, 0);
    assert_eq!(live.text, "Grüße, 世界 😀");
    assert_eq!(live.text, builder.text());
    assert_eq!(live.text_bytes(), "Grüße, 世界 😀".len() as u64, "bytes, not chars");
    assert_eq!(live.reasoning, "**Looking** around\n**Reading the logs**\n");
    assert_eq!(live.title.as_deref(), Some("Reading the logs"));
    assert_eq!(live.tools, vec![LiveToolInput { call: 0, tool: "shell".to_owned(), bytes: 3 }]);
}

#[test]
fn the_live_turn_drops_what_the_log_has_and_what_ended() {
    let (mut drafter, status, _sender) = drafter_with_status();
    drafter.begin_call(0);
    let mut builder = CompletionBuilder::new();
    feed(&mut drafter, &mut builder, &reasoning("**Plan**"));
    feed(&mut drafter, &mut builder, &text("One."));
    feed(&mut drafter, &mut builder, &start("a", "shell"));
    drafter.answer_ended();
    assert!(status.live_turn().unwrap().tools.is_empty(), "the calls follow as events");
    drafter.message_completed(1);
    let live = status.live_turn().unwrap();
    assert_eq!((live.index, live.text.as_str()), (1, ""), "the log has the text");
    assert_eq!(live.reasoning, "**Plan**", "reasoning is never in the log");

    drafter.begin_call(1);
    let mut builder = CompletionBuilder::new();
    feed(&mut drafter, &mut builder, &reasoning("Next"));
    let live = status.live_turn().unwrap();
    assert_eq!(live.reasoning, "**Plan**\n\nNext");
    assert_eq!(live.title, None, "the title of the last call says nothing about this one");

    status.clear(true);
    assert!(status.live_turn().is_some(), "a compaction keeps the parts");
    status.clear(false);
    assert_eq!(status.live_turn(), None, "the end of the turn drops them");
}

#[test]
fn a_bold_last_line_of_the_last_call_is_no_title_of_the_next() {
    let (mut drafter, sender, mut builder) = drafter();
    let mut receiver = sender.subscribe();
    feed(&mut drafter, &mut builder, &reasoning("Look.\n**Done looking**"));
    drafter.begin_call(0);
    feed(&mut drafter, &mut builder, &reasoning("Now the fix"));
    let titles: Vec<Option<String>> = parts(&mut receiver)
        .into_iter()
        .map(|part| match part {
            DraftPart::Reasoning { title, .. } => title,
            other => panic!("only reasoning: {other:?}"),
        })
        .collect();
    assert_eq!(titles, vec![Some("Done looking".to_owned()), None]);
}

/// One provider event of a generated stream.
#[derive(Debug, Clone)]
enum Step {
    Text(String),
    Reasoning(String),
    Call,
    Arguments(String),
}

fn step() -> impl Strategy<Value = Step> {
    let piece = proptest::sample::select(vec!["", "a", "b c", "é", "世界", "😀", "\n", "**T**\n"]);
    let pieces = proptest::collection::vec(piece, 0..4).prop_map(|pieces| pieces.concat());
    prop_oneof![
        4 => pieces.clone().prop_map(Step::Text),
        2 => pieces.clone().prop_map(Step::Reasoning),
        1 => Just(Step::Call),
        1 => pieces.prop_map(Step::Arguments),
    ]
}

proptest! {
    /// Any stream of events gives live parts that equal the builder's text, and drafts
    /// that, joined at their offsets, equal the live parts.
    #[test]
    fn the_live_parts_and_the_drafts_follow_the_builder(
        steps in proptest::collection::vec(step(), 0..40),
    ) {
        let (mut drafter, status, sender) = drafter_with_status();
        let mut receiver = sender.subscribe();
        drafter.begin_call(0);
        let mut builder = CompletionBuilder::new();
        let mut calls = 0;
        for step in steps {
            let event = match step {
                Step::Text(text) => ProviderEvent::TextDelta { text },
                Step::Reasoning(text) => ProviderEvent::ReasoningDelta { text },
                Step::Call => {
                    calls += 1;
                    start(&format!("c{calls}"), "shell")
                }
                Step::Arguments(arguments) if calls > 0 => {
                    ProviderEvent::ToolCallDelta { call_id: format!("c{calls}"), arguments }
                }
                Step::Arguments(_) => continue,
            };
            feed(&mut drafter, &mut builder, &event);
        }
        let (mut text, mut reasoning) = (String::new(), String::new());
        for part in parts(&mut receiver) {
            match part {
                DraftPart::Text { offset, delta, .. } => {
                    prop_assert_eq!(offset, text.len() as u64);
                    text.push_str(&delta);
                }
                DraftPart::Reasoning { offset, delta, .. } => {
                    prop_assert_eq!(offset, reasoning.len() as u64);
                    reasoning.push_str(&delta);
                }
                _ => {}
            }
        }
        let live = status.live_turn().unwrap();
        prop_assert_eq!(&live.text, &builder.text());
        prop_assert_eq!(&live.text, &text);
        prop_assert_eq!(&live.reasoning, &reasoning);
    }
}

/// The tail of each tool input draft that `receiver` holds now.
fn tails(receiver: &mut broadcast::Receiver<ConversationDraft>) -> Vec<(u64, String)> {
    parts(receiver)
        .into_iter()
        .filter_map(|part| match part {
            DraftPart::ToolInput { bytes, tail, .. } => Some((bytes, tail.unwrap_or_default())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_tool_input_draft_carries_the_newest_raw_bytes_cut_at_a_character_and_safe_to_print() {
    let (mut drafter, sender, mut builder) = drafter();
    let mut receiver = sender.subscribe();
    feed(&mut drafter, &mut builder, &start("a", "shell"));
    let more = |arguments: &str| ProviderEvent::ToolCallDelta {
        call_id: "a".to_owned(),
        arguments: arguments.to_owned(),
    };
    // A command of 3.8 KB, with two-byte characters, an escape and a raw newline.
    let mut written = String::from("{\"command\":\"");
    feed(&mut drafter, &mut builder, &more(&written));
    for n in 0..200 {
        let piece = format!("é{n:03}\x1b\n\\n");
        written.push_str(&piece);
        feed(&mut drafter, &mut builder, &more(&piece));
    }
    let drafts = tails(&mut receiver);
    assert_eq!(drafts.len(), 202, "a draft for each change, as the interval is 0");
    for (bytes, tail) in &drafts[1..] {
        assert!(tail.len() <= super::INPUT_TAIL, "{tail:?}");
        // The tail is the raw input before `bytes`, with each control character as a
        // space of the same length.
        let start = *bytes as usize - tail.len();
        let raw = &written[start..*bytes as usize];
        assert_eq!(*tail, raw.replace(['\x1b', '\n'], " "));
        assert!(!tail.chars().any(char::is_control), "{tail:?}");
    }
    let (bytes, tail) = drafts.last().unwrap();
    assert_eq!(*bytes, written.len() as u64);
    // The cut at 256 bytes falls inside an `é`, so the tail starts after it.
    assert!(tail.len() == 255 || tail.len() == 256, "{}", tail.len());
    assert!(tail.ends_with("\u{e9}199  \\n"), "{tail:?}");
    // The end of the call sets the whole input, and the tail stays its newest bytes.
    let end = ProviderEvent::ToolCallEnd { call_id: "a".to_owned(), arguments: written.clone() };
    feed(&mut drafter, &mut builder, &end);
    assert!(tails(&mut receiver).is_empty(), "nothing changed, so no draft");
}

#[test]
fn a_retry_goes_at_once_and_the_status_keeps_it_until_the_call_opens() {
    use efr_provider::{CallNotice, CallNotices as _};
    let (drafter, status, sender) = drafter_with_status();
    let mut receiver = sender.subscribe();
    let notices = drafter.call_notices();
    notices.notice(CallNotice::Connecting);
    let wait = Duration::from_millis(480);
    notices.notice(CallNotice::Retrying { attempt: 1, wait, status: Some(503) });
    notices.notice(CallNotice::Retrying { attempt: 2, wait, status: None });
    let retry = |attempt: u32, status: Option<u16>, reason: &str| DraftPart::Retrying {
        attempt,
        wait_ms: 480,
        status,
        reason: reason.to_owned(),
    };
    assert_eq!(
        parts(&mut receiver),
        [
            DraftPart::Connecting,
            retry(1, Some(503), "the provider answered 503"),
            retry(2, None, "the connection failed"),
        ]
    );
    let held: Vec<DraftPart> = status.drafts().into_iter().map(|draft| draft.part).collect();
    assert_eq!(held, [retry(2, None, "the connection failed")], "only the newest");
    notices.notice(CallNotice::Connected);
    assert!(status.drafts().is_empty(), "the provider took the call");
    notices.notice(CallNotice::Connecting);
    drafter.call_opened();
    assert!(status.drafts().is_empty(), "the call opened");
}

/// A step of the provider's web tool ends the message inside the model call: the text
/// after it is the next message, whose live text and text drafts start again at offset
/// 0 and equal the builder's text after the step.
#[test]
fn a_message_that_ends_at_a_web_step_starts_the_next_one_at_offset_zero() {
    let (mut drafter, status, sender) = drafter_with_status();
    let mut receiver = sender.subscribe();
    drafter.begin_call(0);
    let mut builder = CompletionBuilder::new();
    feed(&mut drafter, &mut builder, &text("I'll check."));
    let step = ProviderEvent::HostedCallStart {
        call_id: "srvtoolu_1".to_owned(),
        action: efr_provider::HostedAction::Search { query: None },
    };
    // NOTE: the turn completes message 0 and moves the drafter before the builder takes
    // the start (stream.rs).
    drafter.message_completed(1);
    drafter.next_message(1);
    feed(&mut drafter, &mut builder, &step);
    feed(&mut drafter, &mut builder, &text("It works"));
    feed(&mut drafter, &mut builder, &text("."));

    let live = status.live_turn().unwrap();
    assert_eq!((live.index, live.text.as_str()), (1, "It works."));
    assert_eq!(live.text, builder.segment_text());
    assert_eq!(
        parts(&mut receiver),
        [
            DraftPart::Text { index: 0, offset: 0, delta: "I'll check.".to_owned() },
            DraftPart::Text { index: 1, offset: 0, delta: "It works".to_owned() },
            DraftPart::Text { index: 1, offset: 8, delta: ".".to_owned() },
        ]
    );
}
