//! How a turn ends: the marker after a turn that did not complete, so the next turn
//! does not answer a cancelled prompt, and why the model stopped when it gave no answer.

use std::sync::{Arc, Mutex};

use efr_protocol::{Event, Origin, StopReason as WireStop, TurnId, TurnStop};
use efr_provider::{Message, ProviderEvent, StopReason};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::history::INTERRUPTED_MARKER;
use crate::testing::{
    Harness, Setup, answer, done, expect_request, failure, request, result_message, text_answer,
    tool_answer, tool_message,
};
use crate::turn::STOPPED;

/// Interrupts the running turn `turn_id` while its `hang` call runs, and waits for its
/// end.
async fn interrupt(h: &mut Harness, turn_id: TurnId) {
    h.toolbox.hang_started.notified().await;
    let params = h.interrupt_params(Some(turn_id), Vec::new(), Vec::new());
    h.handle.interrupt(params, Origin::Shell).await.expect("interrupt accepted");
    let end = h.wait_end(turn_id).await;
    assert!(matches!(end, Event::TurnInterrupted { .. }), "{end:?}");
}

#[tokio::test]
async fn an_interrupted_turn_saves_its_messages_with_the_marker_last() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "wait");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "wait")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("wait").await;
    interrupt(&mut h, sent.turn_id).await;

    let conversation_id = h.conversation_id;
    let saved = h
        .store
        .readers()
        .with(move |conn| efr_store::turn_messages::of_conversation(conn, conversation_id))
        .await
        .expect("saved messages");
    let messages: Vec<Message> = saved[0]
        .messages
        .iter()
        .map(|message| serde_json::from_value(message.clone()).expect("a message"))
        .collect();
    assert_eq!(
        messages[1..].to_vec(),
        vec![
            tool_message("call_1", "hang", &json!({})),
            result_message("call_1", STOPPED, true),
            Message::user(INTERRUPTED_MARKER),
        ]
    );
    h.finish();
}

/// The marker comes after every message of the interrupted request and its answer, so
/// the provider's cache still reads the whole interrupted request.
#[tokio::test]
async fn the_request_after_an_interrupted_turn_starts_with_the_bytes_of_the_interrupted_request() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let first = request(vec![setup.prompt(&state, "first")]);
    let second = request(vec![
        setup.prompt(&state, "first"),
        tool_message("call_1", "hang", &json!({})),
        result_message("call_1", STOPPED, true),
        Message::user(INTERRUPTED_MARKER),
        setup.prompt(&state, "something else"),
    ]);
    assert!(second.messages.starts_with(&first.messages));
    let records = vec![
        expect_request(first),
        answer(&tool_answer("call_1", "hang", &json!({}))),
        expect_request(second),
        answer(&text_answer("Here it is.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("first").await;
    interrupt(&mut h, sent.turn_id).await;
    let next = h.prompt("something else").await;
    let end = h.wait_end(next.turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}

/// A steer that no model call read goes again as a prompt of its own: it follows the
/// marker, so the model reads it as the user's newest message.
#[tokio::test]
async fn a_steer_left_unread_follows_the_marker_as_the_next_prompt() {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "first");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "first")])),
        answer(&tool_answer("call_1", "hang", &json!({}))),
        expect_request(request(vec![
            setup.prompt(&state, "first"),
            tool_message("call_1", "hang", &json!({})),
            result_message("call_1", STOPPED, true),
            Message::user(INTERRUPTED_MARKER),
            setup.prompt(&state, "in French"),
        ])),
        answer(&text_answer("En français.")),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("first").await;
    h.toolbox.hang_started.notified().await;
    let steer = h.steer_params(Some(sent.turn_id), "in French", true);
    let steer = h.handle.steer(steer, Origin::Shell).await.expect("steer accepted");
    let params = h.interrupt_params(Some(sent.turn_id), vec![steer.seq], Vec::new());
    let result = h.handle.interrupt(params, Origin::Shell).await.expect("interrupt accepted");
    let resent = result.resent.expect("the unread steer goes again");
    let end = h.wait_end(resent.turn_id).await;

    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();
}

/// Runs one turn whose model answers with `events`, and returns its `stop`.
async fn stop_of(events: &[ProviderEvent]) -> Option<TurnStop> {
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "read README.md and do the setup");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "read README.md and do the setup")])),
        answer(events),
    ];
    let mut h = setup.start(records).await;
    let sent = h.prompt("read README.md and do the setup").await;
    let end = h.wait_end(sent.turn_id).await;
    h.finish();
    match end {
        Event::TurnCompleted { stop, .. } => stop,
        other => panic!("the turn completes, got {other:?}"),
    }
}

/// The live run (case 9.1): the model answered with nothing, and the user saw only
/// `done in 3.0s`.
#[tokio::test]
async fn a_final_call_with_no_text_and_no_call_records_the_stop_on_turn_completed() {
    let stop = stop_of(&[done(StopReason::Refusal, None)]).await;
    assert_eq!(stop, Some(TurnStop { reason: WireStop::Refusal, empty: true }));
    let stop = stop_of(&[done(StopReason::EndTurn, None)]).await;
    assert_eq!(stop, Some(TurnStop { reason: WireStop::EndTurn, empty: true }));
}

#[tokio::test]
async fn a_final_answer_cut_at_max_tokens_records_the_stop() {
    let stop = stop_of(&[
        ProviderEvent::TextDelta { text: "The setup has three st".to_owned() },
        done(StopReason::MaxTokens, None),
    ])
    .await;
    assert_eq!(stop, Some(TurnStop { reason: WireStop::MaxTokens, empty: false }));
}

#[tokio::test]
async fn an_end_turn_answer_with_text_records_no_stop() {
    assert_eq!(stop_of(&text_answer("Done.")).await, None);
}

/// The lines of the log of a test, as plain text.
#[derive(Debug, Clone, Default)]
struct LogText(Arc<Mutex<Vec<u8>>>);

impl LogText {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for LogText {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogText {
    type Writer = LogText;

    fn make_writer(&'a self) -> LogText {
        self.clone()
    }
}

/// The live run: a turn failed, and the journal had no line for it.
#[tokio::test]
async fn a_failed_turn_logs_one_info_line_with_its_code() {
    let log = LogText::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    // NOTE: the test's runtime has one thread, so the actor's task logs here too.
    let _guard = tracing::subscriber::set_default(subscriber);
    let setup = Setup::new();
    let state = setup.live_state(&setup.cwd, "hello");
    let records = vec![
        expect_request(request(vec![setup.prompt(&state, "hello")])),
        failure(json!({ "kind": "unauthorized" })),
    ];
    let mut h = setup.start(records).await;

    let sent = h.prompt("hello").await;
    let end = h.wait_end(sent.turn_id).await;

    assert!(matches!(end, Event::TurnFailed { .. }), "{end:?}");
    let text = log.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains("the turn failed")).collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].contains("INFO"), "{text}");
    assert!(lines[0].contains("code=unauthorized"), "{text}");
    assert!(lines[0].contains("turn_id="), "the turn's span names it: {text}");
    h.finish();
}
