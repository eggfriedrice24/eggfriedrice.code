use std::io;

use futures::stream;
use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;

use super::{Completion, CompletionBuilder};
use crate::{ContentBlock, Message, ProviderError, ProviderEvent, Role, StopReason, TokenUsage};

fn text(text: &str) -> ProviderEvent {
    ProviderEvent::TextDelta { text: text.to_owned() }
}

fn reasoning(text: &str) -> ProviderEvent {
    ProviderEvent::ReasoningDelta { text: text.to_owned() }
}

fn start(call_id: &str, name: &str) -> ProviderEvent {
    ProviderEvent::ToolCallStart { call_id: call_id.to_owned(), name: name.to_owned() }
}

fn delta(call_id: &str, arguments: &str) -> ProviderEvent {
    ProviderEvent::ToolCallDelta { call_id: call_id.to_owned(), arguments: arguments.to_owned() }
}

fn end(call_id: &str, arguments: &str) -> ProviderEvent {
    ProviderEvent::ToolCallEnd { call_id: call_id.to_owned(), arguments: arguments.to_owned() }
}

fn done(stop_reason: StopReason) -> ProviderEvent {
    ProviderEvent::Done { stop_reason, provider_raw: None }
}

fn usage(input: u64, output: u64) -> TokenUsage {
    TokenUsage { input_tokens: input, output_tokens: output, ..TokenUsage::default() }
}

fn build(events: &[ProviderEvent]) -> Result<Completion, ProviderError> {
    let mut builder = CompletionBuilder::new();
    for event in events {
        builder.push(event)?;
    }
    builder.finish()
}

fn call(call_id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolCall { call_id: call_id.to_owned(), name: name.to_owned(), input }
}

#[test]
fn text_deltas_grow_one_block() {
    let completion = build(&[
        text("Here are "),
        text(""),
        text("the files."),
        ProviderEvent::Usage(usage(100, 5)),
        done(StopReason::EndTurn),
    ])
    .unwrap();
    assert_eq!(
        completion,
        Completion {
            message: Message::assistant("Here are the files."),
            stop_reason: StopReason::EndTurn,
            usage: Some(usage(100, 5)),
        }
    );
}

#[test]
fn reasoning_text_and_calls_keep_their_order() {
    let completion = build(&[
        reasoning("The user wants "),
        reasoning("a listing."),
        text("Checking."),
        start("call_1", "shell"),
        delta("call_1", "{\"command\":"),
        delta("call_1", "\"ls\"}"),
        end("call_1", "{\"command\":\"ls\"}"),
        text("Then more."),
        done(StopReason::ToolUse),
    ])
    .unwrap();
    assert_eq!(
        completion.message.content,
        [
            ContentBlock::Reasoning { text: "The user wants a listing.".to_owned() },
            ContentBlock::Text { text: "Checking.".to_owned() },
            call("call_1", "shell", json!({"command": "ls"})),
            ContentBlock::Text { text: "Then more.".to_owned() },
        ]
    );
    assert_eq!(completion.message.role, Role::Assistant);
    assert_eq!(completion.stop_reason, StopReason::ToolUse);
    assert_eq!(completion.usage, None);
}

#[test]
fn interleaved_calls_sit_where_they_started() {
    let completion = build(&[
        start("a", "read_file"),
        start("b", "shell"),
        delta("b", "{}"),
        end("b", "{\"command\":\"pwd\"}"),
        end("a", "{\"path\":\"Cargo.toml\"}"),
        done(StopReason::ToolUse),
    ])
    .unwrap();
    assert_eq!(
        completion.message.content,
        [
            call("a", "read_file", json!({"path": "Cargo.toml"})),
            call("b", "shell", json!({"command": "pwd"})),
        ]
    );
}

#[rstest]
#[case::object("{\"command\":\"ls\"}", json!({"command": "ls"}))]
#[case::empty("", json!({}))]
#[case::blank("  \n", json!({}))]
#[case::not_json("{\"command\": ls", json!("{\"command\": ls"))]
#[case::not_an_object("[1, 2]", json!([1, 2]))]
fn end_arguments_become_the_input(#[case] arguments: &str, #[case] input: serde_json::Value) {
    let completion =
        build(&[start("c", "shell"), end("c", arguments), done(StopReason::ToolUse)]).unwrap();
    assert_eq!(completion.message.content, [call("c", "shell", input)]);
}

#[test]
fn done_brings_the_raw_items_and_the_last_usage_counts() {
    let raw = json!([{"type": "reasoning", "encrypted_content": "gAAA"}]);
    let completion = build(&[
        ProviderEvent::Raw(json!({"type": "response.created"})),
        text("ok"),
        ProviderEvent::Usage(usage(1, 1)),
        ProviderEvent::Usage(usage(10, 2)),
        ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: Some(raw.clone()) },
        ProviderEvent::Raw(json!({"type": "keep-alive"})),
    ])
    .unwrap();
    assert_eq!(completion.message.provider_raw, Some(raw));
    assert_eq!(completion.usage, Some(usage(10, 2)));
}

#[rstest]
#[case::delta_without_start(vec![delta("c", "{}")], "arguments for a tool call that is not open")]
#[case::end_without_start(vec![end("c", "{}")], "the end of a tool call that is not open")]
#[case::end_twice(
    vec![start("c", "shell"), end("c", "{}"), end("c", "{}")],
    "the end of a tool call that is not open"
)]
#[case::delta_after_end(
    vec![start("c", "shell"), end("c", "{}"), delta("c", "x")],
    "arguments for a tool call that is not open"
)]
#[case::start_twice(vec![start("c", "shell"), start("c", "shell")], "a tool call started twice")]
#[case::restart_after_end(
    vec![start("c", "shell"), end("c", "{}"), start("c", "shell")],
    "a tool call started twice"
)]
#[case::text_after_done(vec![done(StopReason::EndTurn), text("late")], "an event after done")]
#[case::second_done(vec![done(StopReason::EndTurn), done(StopReason::EndTurn)], "a second done")]
fn out_of_order_events_are_refused(#[case] events: Vec<ProviderEvent>, #[case] expected: &str) {
    match build(&events) {
        Err(ProviderError::InvalidStream { problem }) => assert_eq!(problem, expected),
        other => panic!("{events:?} gave {other:?}"),
    }
}

#[test]
fn a_stream_without_done_is_incomplete() {
    assert!(matches!(build(&[text("partial")]), Err(ProviderError::Incomplete)));
    assert!(matches!(build(&[]), Err(ProviderError::Incomplete)));
}

#[test]
fn a_call_that_never_ends_is_refused() {
    let result = build(&[start("c", "shell"), delta("c", "{"), done(StopReason::ToolUse)]);
    assert!(matches!(
        result,
        Err(ProviderError::InvalidStream { problem: "a tool call never ended" })
    ));
}

#[test]
fn text_so_far_joins_the_text_blocks() {
    let mut builder = CompletionBuilder::new();
    assert_eq!(builder.text(), "");
    for event in [reasoning("hidden"), text("One"), text(" two."), start("c", "shell")] {
        builder.push(&event).unwrap();
    }
    assert_eq!(builder.text(), "One two.");
    for event in [end("c", "{}"), text("Three.")] {
        builder.push(&event).unwrap();
    }
    assert_eq!(builder.text(), "One two.\n\nThree.");
}

#[tokio::test]
async fn collect_reads_a_stream_to_its_end() {
    let events = vec![Ok(text("hi")), Ok(done(StopReason::EndTurn))];
    let completion = Completion::collect(stream::iter(events)).await.unwrap();
    assert_eq!(completion.message, Message::assistant("hi"));
}

#[tokio::test]
async fn collect_returns_the_first_error_item() {
    let events = vec![
        Ok(text("hi")),
        Err(ProviderError::Transport { source: Box::new(io::Error::other("reset")) }),
        Ok(done(StopReason::EndTurn)),
    ];
    let result = Completion::collect(stream::iter(events)).await;
    assert!(matches!(result, Err(ProviderError::Transport { .. })));
}

#[tokio::test]
async fn collect_reports_a_stream_cut_short() {
    let result = Completion::collect(stream::iter(vec![Ok(text("hi"))])).await;
    assert!(matches!(result, Err(ProviderError::Incomplete)));
}
