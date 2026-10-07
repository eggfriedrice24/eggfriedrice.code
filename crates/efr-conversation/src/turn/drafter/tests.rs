use std::time::Duration;

use efr_protocol::{ConversationId, DraftPart, TurnId};
use efr_provider::{CompletionBuilder, ProviderEvent};
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use tokio::sync::broadcast;

use super::{Drafter, bold_title};
use crate::{ConversationDraft, draft_channel};

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";

/// A drafter that sends every change at once, its channel, and a builder.
fn drafter() -> (Drafter, broadcast::Sender<ConversationDraft>, CompletionBuilder) {
    let sender = draft_channel();
    let conversation_id: ConversationId = CONVERSATION.parse().unwrap();
    let turn_id: TurnId = TURN.parse().unwrap();
    let drafter = Drafter::new(sender.clone(), conversation_id, turn_id, Duration::ZERO);
    (drafter, sender, CompletionBuilder::new())
}

/// Feeds `event` to the builder and the drafter, as the turn does.
fn feed(drafter: &mut Drafter, builder: &mut CompletionBuilder, event: &ProviderEvent) {
    builder.push(event).unwrap();
    if drafter.push(event) {
        drafter.offer(Timestamp::UNIX_EPOCH, builder, 0);
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
    drafter.begin_call();
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
    let input = |call: u32, tool: &str, bytes: u64| DraftPart::ToolInput {
        call,
        tool: tool.to_owned(),
        bytes,
    };
    assert_eq!(
        parts(&mut receiver),
        vec![
            input(0, "shell", 0),
            input(1, "write_file", 0),
            input(1, "write_file", 7),
            input(0, "shell", 2),
            input(1, "write_file", 2),
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
    drafter.begin_call();
    let mut builder = CompletionBuilder::new();
    builder.push(&text("Two.")).unwrap();
    assert!(drafter.push(&text("Two.")));
    drafter.offer(Timestamp::UNIX_EPOCH, &builder, 1);
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
