//! The request prefix on the OpenAI path: the real `openai-api` provider against a local
//! Responses server, over many turns of one conversation and a daemon restart. The
//! provider's prompt cache reads an earlier request only when the next request starts
//! with the same `input` items, so the report below says, for each request, whether it
//! keeps the `input` of the request before it as its prefix, or where it first edits
//! it.

use efr_protocol::{ConversationId, Event, PromptSendResult};
use efr_test_daemon::{ResponsesAnswer, ResponsesServer, TTY, TestDaemon, events_until};
use serde_json::Value;

/// The turns before the restart: past the 50 turns that the history kept exact.
const TURNS_BEFORE: u128 = 53;

/// The turns after the restart.
const TURNS_AFTER: u128 = 2;

/// Sends the prompt number `n` and waits for the end of its turn.
async fn turn(daemon: &TestDaemon, n: u128) -> ConversationId {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let text = format!("prompt {n}");
    let sent: PromptSendResult = client.call(daemon.prompt(n, &text, TTY)).await.unwrap();
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let turn_id = sent.turn_id;
    let events = events_until(&mut follow, |event| {
        matches!(
            event,
            Event::TurnCompleted { turn_id: t, .. } | Event::TurnFailed { turn_id: t, .. }
                if *t == turn_id
        )
    })
    .await
    .unwrap();
    assert_eq!(events.last().unwrap().event.kind(), "turn_completed", "{events:#?}");
    sent.conversation_id
}

/// One line for each request after the first: whether its `input` starts with the
/// `input` of the request before it, or the first item that it edits.
fn prefix_report(bodies: &[Value]) -> String {
    let mut lines = Vec::new();
    for (at, pair) in bodies.windows(2).enumerate() {
        let (before, after) = (&pair[0], &pair[1]);
        let number = at + 2;
        for key in ["instructions", "tools"] {
            if before[key] != after[key] {
                lines.push(format!("request {number}: changes {key}"));
            }
        }
        let empty = Vec::new();
        let old = before["input"].as_array().unwrap_or(&empty);
        let new = after["input"].as_array().unwrap_or(&empty);
        let edit = old.iter().zip(new).position(|(old, new)| old != new);
        let line = match edit {
            None if new.len() >= old.len() => {
                format!("request {number}: keeps the {} items before it", old.len())
            }
            None => format!("request {number}: drops items from {} on", new.len()),
            Some(item) => {
                let role = old[item]["role"].as_str().unwrap_or("none");
                format!("request {number}: edits item {item} of {} ({role})", old.len())
            }
        };
        lines.push(line);
    }
    lines.join("\n")
}

#[tokio::test]
async fn each_request_starts_with_the_request_before_it_across_turns_and_a_restart() {
    let server = ResponsesServer::start().await;
    for n in 1..=TURNS_BEFORE + TURNS_AFTER {
        server.push(ResponsesAnswer::text(&format!("answer {n}")));
    }
    let mut daemon = TestDaemon::builder().responses(&server).persistent().start().await.unwrap();

    let conversation = turn(&daemon, 1).await;
    for n in 2..=TURNS_BEFORE {
        assert_eq!(turn(&daemon, n).await, conversation);
    }
    daemon.restart().await.unwrap();
    for n in TURNS_BEFORE + 1..=TURNS_BEFORE + TURNS_AFTER {
        assert_eq!(turn(&daemon, n).await, conversation, "the terminal keeps its conversation");
    }

    let bodies: Vec<Value> = server.received().into_iter().map(|request| request.body).collect();
    assert_eq!(bodies.len() as u128, TURNS_BEFORE + TURNS_AFTER);
    insta::assert_snapshot!(prefix_report(&bodies));
    daemon.stop().await.unwrap();
}
