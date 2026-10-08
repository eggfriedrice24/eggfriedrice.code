//! Drafts: what a running turn sends to live clients, on the clock, and that the event
//! log is the same with and without a listener.

use std::time::Duration;

use efr_protocol::{DraftPart, Event, Seq};
use efr_provider::{ContentBlock, Message, ProviderEvent, Request, Role, StopReason};
use efr_test_support::Record;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::broadcast;

use super::unknown_window;
use crate::ConversationDraft;
use crate::context::request_tokens;
use crate::testing::{Harness, Setup, answer, done, expect_request, hold, request, result_message};

fn text(text: &str) -> ProviderEvent {
    ProviderEvent::TextDelta { text: text.to_owned() }
}

fn reasoning(text: &str) -> ProviderEvent {
    ProviderEvent::ReasoningDelta { text: text.to_owned() }
}

/// The records of a turn whose model reasons, writes the input of one `read_file` call
/// in two pieces, reads the result, reasons again and answers in two pieces of text,
/// with the input of the call and the two requests.
fn reasoning_tool_text(setup: &Setup) -> (Vec<Record>, String, [Request; 2]) {
    let state = setup.live_state(&setup.cwd, "read my notes");
    let notes = setup.home().join("notes.txt");
    let input = json!({ "path": notes });
    let arguments = input.to_string();
    let (head, tail) = arguments.split_at(8);
    let first = setup.prompt(&state, "read my notes");
    let output = format!("contents of {}", notes.display());
    let called = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::Reasoning { text: "**Reading the notes**\n\nI open them.".to_owned() },
            ContentBlock::ToolCall {
                call_id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                input: input.clone(),
                freeform: false,
            },
        ],
    );
    let requests = [
        request(vec![first.clone()]),
        request(vec![first, called, result_message("call_1", &output, false)]),
    ];
    let records = vec![
        expect_request(requests[0].clone()),
        answer(&[
            reasoning("**Reading the notes**"),
            reasoning("\n\nI open them."),
            ProviderEvent::ToolCallStart {
                call_id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                freeform: false,
            },
            ProviderEvent::ToolCallDelta {
                call_id: "call_1".to_owned(),
                arguments: head.to_owned(),
            },
            ProviderEvent::ToolCallDelta {
                call_id: "call_1".to_owned(),
                arguments: tail.to_owned(),
            },
            ProviderEvent::ToolCallEnd {
                call_id: "call_1".to_owned(),
                arguments: arguments.clone(),
            },
            done(StopReason::ToolUse, None),
        ]),
        expect_request(requests[1].clone()),
        answer(&[
            reasoning("**Answering**"),
            text("Your notes"),
            text(" say hi."),
            done(StopReason::EndTurn, None),
        ]),
    ];
    (records, arguments, requests)
}

/// Every draft that `receiver` holds now.
fn drained(receiver: &mut broadcast::Receiver<ConversationDraft>) -> Vec<ConversationDraft> {
    let mut drafts = Vec::new();
    while let Ok(draft) = receiver.try_recv() {
        drafts.push(draft);
    }
    drafts
}

/// The next draft other than a `context` one, waiting at most a second of real time.
async fn next_draft(receiver: &mut broadcast::Receiver<ConversationDraft>) -> ConversationDraft {
    loop {
        let draft = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("a draft in time")
            .expect("the channel is open");
        if !matches!(draft.part, DraftPart::Context(_)) {
            return draft;
        }
    }
}

/// The text of one update or completion: its index, its offset (`None` for the
/// completion) and its text.
type Written = (u32, Option<u64>, String);

/// What the log holds: the kinds of all events, and the text of every update and
/// completion.
async fn log_of(h: &Harness) -> (Vec<String>, Vec<Written>) {
    let texts = h
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            Event::AssistantMessageUpdated { index, offset, delta, .. } => {
                Some((index, Some(offset), delta))
            }
            Event::AssistantMessageCompleted { index, text, .. } => Some((index, None, text)),
            _ => None,
        })
        .collect();
    (h.kinds().await, texts)
}

#[tokio::test]
async fn drafts_carry_the_reasoning_the_tool_input_and_the_text_as_they_arrive() {
    let mut setup = Setup::new();
    setup.config.draft_interval = Duration::ZERO;
    let (records, arguments, requests) = reasoning_tool_text(&setup);
    let mut receiver = setup.drafts.subscribe();
    let conversation_id = setup.conversation_id;
    let mut h = setup.start(records).await;

    let sent = h.prompt("read my notes").await;
    h.wait_end(sent.turn_id).await;

    let drafts = drained(&mut receiver);
    assert!(
        drafts
            .iter()
            .all(|draft| draft.conversation_id == conversation_id && draft.turn_id == sent.turn_id),
        "{drafts:?}"
    );
    // Each draft names the last event that the turn recorded before it.
    let envelopes = h.envelopes().await;
    let seq_of = |kind: &str| {
        envelopes.iter().find(|envelope| envelope.event.kind() == kind).map(|envelope| envelope.seq)
    };
    let after: Vec<Option<Seq>> = drafts.iter().map(|draft| Some(draft.after_seq)).collect();
    let started = seq_of("turn_started");
    let completed = seq_of("tool_call_completed");
    let updated = seq_of("assistant_message_updated");
    assert_eq!(after, [vec![started; 6], vec![completed; 3], vec![updated]].concat());
    let parts: Vec<DraftPart> = drafts.into_iter().map(|draft| draft.part).collect();
    // NOTE: no call reports its usage here, so each call has only the estimate before it.
    let estimate =
        |request: &Request| DraftPart::Context(unknown_window().gauge(request_tokens(request)));
    let title = |title: &str| Some(title.to_owned());
    let reading = "**Reading the notes**".len() as u64;
    let first_call = reading + "\n\nI open them.".len() as u64;
    assert_eq!(
        parts,
        vec![
            estimate(&requests[0]),
            DraftPart::Reasoning {
                offset: 0,
                delta: "**Reading the notes**".to_owned(),
                title: title("Reading the notes"),
            },
            DraftPart::Reasoning {
                offset: reading,
                delta: "\n\nI open them.".to_owned(),
                title: title("Reading the notes"),
            },
            DraftPart::ToolInput { call: 0, tool: "read_file".to_owned(), bytes: 0 },
            DraftPart::ToolInput { call: 0, tool: "read_file".to_owned(), bytes: 8 },
            DraftPart::ToolInput {
                call: 0,
                tool: "read_file".to_owned(),
                bytes: arguments.len() as u64,
            },
            estimate(&requests[1]),
            // The reasoning of the second model call continues after a blank line.
            DraftPart::Reasoning {
                offset: first_call,
                delta: "\n\n**Answering**".to_owned(),
                title: title("Answering"),
            },
            DraftPart::Text { index: 0, offset: 0, delta: "Your notes".to_owned() },
            DraftPart::Text { index: 0, offset: 10, delta: " say hi.".to_owned() },
        ]
    );
    h.finish();
}

#[tokio::test]
async fn the_event_log_is_the_same_with_and_without_a_listener() {
    let mut logs = Vec::new();
    for listen in [false, true] {
        let mut setup = Setup::new();
        setup.config.draft_interval = Duration::ZERO;
        let (records, _, _) = reasoning_tool_text(&setup);
        let mut receiver = setup.drafts.subscribe();
        if !listen {
            drop(receiver);
            receiver = broadcast::channel(1).1;
        }
        let mut h = setup.start(records).await;
        let sent = h.prompt("read my notes").await;
        h.wait_end(sent.turn_id).await;
        assert_eq!(drained(&mut receiver).is_empty(), !listen, "listen: {listen}");
        logs.push(log_of(&h).await);
        h.finish();
    }
    assert_eq!(logs[0], logs[1], "drafts never reach the log");
    assert!(!logs[0].0.iter().any(|kind| kind.contains("draft")));
}

#[tokio::test]
async fn drafts_are_coalesced_on_the_clock_and_the_first_goes_at_once() {
    let mut setup = Setup::new();
    setup.config.draft_interval = Duration::from_millis(16);
    let state = setup.live_state(&setup.cwd, "count");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "count")])),
        answer(&[text("one"), text(" two"), text(" three")]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut receiver = setup.drafts.subscribe();
    let mut h = setup.start(records).await;

    let sent = h.prompt("count").await;
    let first = next_draft(&mut receiver).await;
    assert_eq!(first.part, DraftPart::Text { index: 0, offset: 0, delta: "one".to_owned() });
    h.clock.wait_for_sleeps(1).await;
    assert!(drained(&mut receiver).is_empty(), "the rest waits for the interval");
    h.clock.advance(Duration::from_millis(16));
    let flushed = next_draft(&mut receiver).await;
    assert_eq!(
        flushed.part,
        DraftPart::Text { index: 0, offset: 3, delta: " two three".to_owned() },
        "the held text goes when the interval ends, without what went before"
    );
    h.provider.handled_through(3);
    h.wait_end(sent.turn_id).await;
    h.finish();
}

#[tokio::test]
async fn a_turn_that_nobody_follows_live_sets_no_draft_timer() {
    let mut setup = Setup::new();
    setup.config.draft_interval = Duration::from_millis(16);
    let state = setup.live_state(&setup.cwd, "count");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "count")])),
        answer(&[text("one"), text(" two"), text(" three")]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("count").await;
    h.wait_for(
        |e| matches!(e, Event::AssistantMessageUpdated { delta, .. } if delta.ends_with("three")),
    )
    .await;
    assert_eq!(h.clock.pending_sleeps(), 0, "no listener, no timer");
    h.provider.handled_through(3);
    h.wait_end(sent.turn_id).await;
    h.finish();
}
