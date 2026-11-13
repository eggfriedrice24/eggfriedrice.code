//! Web search through the real daemon: the scenarios `web_search_*` of efr-test-daemon,
//! whose replay provider gives the canonical events of the provider slices (the build
//! spec of web search, 5.3 item 8). The fixture compares every event and request
//! exactly; these tests name the facts that the look of the board TM-Web needs.

use efr_protocol::{Event, EventEnvelope, WebAction, WebError};
use efr_test_daemon::Replay;
use efr_test_support::{Outbound, Record};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

/// The requests of the scenario's model calls, in order, as its fixture holds them.
fn requests(replay: &Replay) -> Vec<Value> {
    replay
        .scenario()
        .transcript()
        .records()
        .filter_map(|record| match record {
            Record::ExpectOutbound(Outbound::ProviderRequest(body)) => Some(body.clone()),
            _ => None,
        })
        .collect()
}

/// The text of the first block of the newest prompt of `request`: its preamble.
fn preamble(request: &Value) -> &str {
    request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|message| message["role"] == "user"))
        .and_then(|message| message["content"][0]["text"].as_str())
        .unwrap_or_default()
}

fn web_kinds(events: &[EventEnvelope]) -> Vec<&str> {
    events
        .iter()
        .map(|envelope| envelope.event.kind())
        .filter(|kind| kind.starts_with("web_call_") || *kind == "turn_web_report")
        .collect()
}

fn reports(events: &[EventEnvelope]) -> Vec<Event> {
    events
        .iter()
        .filter(|envelope| matches!(envelope.event, Event::TurnWebReport { .. }))
        .map(|envelope| envelope.event.clone())
        .collect()
}

fn completed_texts(events: &[EventEnvelope]) -> Vec<String> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::AssistantMessageCompleted { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A Claude turn (TM-Web 1, 3, 8, 10): the live web in the request, the message ends
/// before the first search, two searches at once end in their own order, the failed
/// search ends with its reason and does not count, and the answer has two marks
/// before the punctuation.
#[tokio::test]
async fn web_search_claude() {
    let replay = Replay::run("web_search_claude").await.unwrap();
    let requests = requests(&replay);
    assert_eq!(
        requests[0]["hosted"],
        json!([{ "kind": "web_search", "access": "live", "max_uses": 20 }])
    );
    assert!(!preamble(&requests[0]).contains("Web search"), "the live web needs no line");

    let events = replay.events().await.unwrap();
    let ends: Vec<(WebAction, Option<WebError>, Option<usize>)> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::WebCallCompleted { action, error, results, .. } => {
                Some((*action, *error, results.as_ref().map(Vec::len)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        [
            (WebAction::Search, None, Some(2)),
            (WebAction::Search, None, Some(1)),
            (WebAction::Search, Some(WebError::TooManyRequests), None),
        ]
    );
    assert_eq!(
        completed_texts(&events),
        [
            "I'll check what changed.".to_owned(),
            "The developers allowed Linux in Easy Anti-Cheat with patch 1.4 \
             [[1]](<https://areweanticheatyet.com/game/rob-run>). ProtonDB reports that it runs \
             with Proton 10 [[2]](<https://www.protondb.com/app/2384710>)."
                .to_owned(),
        ]
    );
    let reports = reports(&events);
    let [Event::TurnWebReport { searches, stored_copy, sources, .. }] = reports.as_slice() else {
        panic!("one report: {reports:?}");
    };
    assert_eq!((*searches, *stored_copy, sources.len()), (2, false, 2));
    assert_eq!(events.last().map(|envelope| envelope.event.kind()), Some("turn_completed"));
    replay.stop().await.unwrap();
}

/// Two OpenAI turns in cautious mode (TM-Web 5, 13): the stored copy in the request and
/// its line in the preamble; a search, an open and a find, of which only the search
/// counts; the text with the model's own link and the unplaced source; the note of the
/// stored copy in the first report only; the second request sends the items of the
/// first answer back.
#[tokio::test]
async fn web_search_openai() {
    let replay = Replay::run("web_search_openai").await.unwrap();
    let requests = requests(&replay);
    for request in &requests {
        assert_eq!(
            request["hosted"],
            json!([{ "kind": "web_search", "access": "stored", "max_uses": 20 }])
        );
        assert!(preamble(request).contains(
            "Web search reads a stored copy of the web, which can be some days old. The user \
             gets the live web with \",mode auto\"."
        ));
    }
    let first_answer = &requests[1]["messages"][1];
    assert_eq!(first_answer["role"], "assistant");
    assert_eq!(
        first_answer["provider_raw"],
        json!([{ "id": "ws_1a", "type": "web_search_call", "status": "completed" }])
    );

    let events = replay.events().await.unwrap();
    let notes: Vec<(u32, bool, bool, usize)> = reports(&events)
        .into_iter()
        .map(|event| match event {
            Event::TurnWebReport { searches, stored_copy, stored_copy_note, sources, .. } => {
                (searches, stored_copy, stored_copy_note, sources.len())
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(notes, [(1, true, true, 1), (1, true, false, 1)]);
    assert!(completed_texts(&events).iter().all(|text| !text.contains("[[")), "no mark on OpenAI");
    replay.stop().await.unwrap();
}

/// A manual turn (TM-Web 9): no web tool in the request, the line in the preamble, and
/// no event of web search.
#[tokio::test]
async fn web_search_manual() {
    let replay = Replay::run("web_search_manual").await.unwrap();
    let requests = requests(&replay);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].get("hosted"), None, "manual sends no web tool");
    assert!(preamble(&requests[0]).contains(
        "Web search is off in manual mode. The user turns it on with \",mode cautious\" in \
         this terminal."
    ));
    let events = replay.events().await.unwrap();
    assert_eq!(web_kinds(&events), Vec::<&str>::new());
    replay.stop().await.unwrap();
}

/// A paused turn goes on: the second request ends with the paused answer and its items
/// as they came, and the search that the pause left open ends in the second answer with
/// the call id of its start.
#[tokio::test]
async fn web_search_pause() {
    let replay = Replay::run("web_search_pause").await.unwrap();
    let requests = requests(&replay);
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["messages"].as_array().unwrap();
    let last = messages.last().unwrap();
    assert_eq!(last["role"], "assistant", "the continuation ends with the paused answer");
    assert_eq!(last["provider_raw"][1]["type"], "server_tool_use");

    let events = replay.events().await.unwrap();
    assert_eq!(web_kinds(&events), ["web_call_started", "web_call_completed", "turn_web_report"]);
    let ids: Vec<_> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::WebCallStarted { call_id, .. } | Event::WebCallCompleted { call_id, .. } => {
                Some(*call_id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(ids[0], ids[1]);
    assert!(
        matches!(events.last().map(|e| &e.event), Some(Event::TurnCompleted { stop: None, .. })),
        "the turn did not end on the pause"
    );
    replay.stop().await.unwrap();
}

/// A search and a shell call in one answer: the request after it ends with the shell's
/// result and no steer, the search ends in that call's answer, and the steer goes in the
/// call after it.
#[tokio::test]
async fn web_search_with_tool() {
    let replay = Replay::run("web_search_with_tool").await.unwrap();
    let requests = requests(&replay);
    assert_eq!(requests.len(), 3);
    let second = requests[1]["messages"].as_array().unwrap();
    assert_eq!(second.last().unwrap()["content"][0]["kind"], "tool_result");
    assert!(!second.iter().any(|message| message["content"][0]["text"] == "and the old ones"));
    let third = requests[2]["messages"].as_array().unwrap();
    assert_eq!(third.last().unwrap()["content"][0]["text"], "and the old ones");

    let events = replay.events().await.unwrap();
    let at = |kind: &str| events.iter().position(|e| e.event.kind() == kind).unwrap();
    assert!(at("turn_steered") < at("tool_call_completed"));
    assert!(at("tool_call_completed") < at("web_call_completed"));
    assert!(at("web_call_completed") < at("steering_delivered"));
    replay.stop().await.unwrap();
}
