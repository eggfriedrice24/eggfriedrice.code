//! The steps of the provider's web tool in a turn: the events of each step, the end of
//! the message at a step, the count, the numbered sources and the marks in the text,
//! `turn_web_report`, the continuation of a paused turn, and the requests that hold
//! their notes and steers for a step that waits (the build spec of web search, 5.3).
//!
//! The provider events are the canonical ones that the provider slices give: a Claude
//! answer gives placed citations, an OpenAI answer unplaced ones. The facts are those of
//! the frames of the board TM-Web that the client draws: 4 (three searches at once,
//! each with its own time), 9 (manual: no tool, the line in the preamble, no event),
//! 10 (the marks before the punctuation, the sources 1, 2, 3), 13 (the count, the stored
//! copy and its note once) and 14 (a failed search does not count).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use efr_protocol::{
    DraftPart, Event, EventEnvelope, Mode, Origin, StopReason as WireStop, TurnStop, WebAction,
    WebError, WebSource,
};
use efr_provider::{
    ContentBlock, HostedAction, HostedError, HostedOutcome, Message, ModelInfo, Provider,
    ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, Role, StopReason, WebResult,
    WebSupport,
};
use efr_test_support::design::Frame;
use efr_test_support::{ReplayProvider, TestClock, Transcript};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::preamble::{LiveState, WEB_OFF_MANUAL, WEB_STORED};
use crate::testing::{
    Harness, MODEL, SUMMARY, Setup, answer, done, expect_request, hold, request, result_message,
    text_answer,
};
use crate::web::{WebPlan, WhyOff};
use crate::{ConversationDraft, ConversationStart, WebSearch, notice};

/// One event of a scripted answer, after the clock moved by the wait.
type Step = (Duration, Result<ProviderEvent, ProviderError>);

/// A model that answers from a script, keeps each request, says that it has `support`,
/// and moves the test's clock before each event by the event's wait, so each event of
/// the log has the time that the script gives it. A summary request gets [`SUMMARY`];
/// a request after the script gets `Done.`.
#[derive(Debug)]
struct Scripted {
    id: ProviderId,
    support: WebSupport,
    clock: TestClock,
    models: Vec<ModelInfo>,
    answers: Mutex<VecDeque<Vec<Step>>>,
    requests: Mutex<Vec<Request>>,
}

impl Scripted {
    fn new(clock: &TestClock, support: WebSupport, answers: Vec<Vec<Step>>) -> Arc<Self> {
        Arc::new(Scripted {
            id: ProviderId::new("replay").expect("provider id"),
            support,
            clock: clock.clone(),
            models: Vec::new(),
            answers: Mutex::new(answers.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn with_models(mut self: Arc<Self>, models: Vec<ModelInfo>) -> Arc<Self> {
        Arc::get_mut(&mut self).expect("not shared yet").models = models;
        self
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    fn web_search(&self) -> WebSupport {
        self.support
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let steps: Vec<Step> = if request.side_call {
            now(text_answer(SUMMARY))
        } else {
            let next = self.answers.lock().unwrap_or_else(PoisonError::into_inner).pop_front();
            next.unwrap_or_else(|| now(text_answer("Done.")))
        };
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        let clock = self.clock.clone();
        Ok(Box::pin(futures::stream::iter(steps).map(move |(wait, event)| {
            if !wait.is_zero() {
                clock.advance(wait);
            }
            event
        })))
    }
}

/// `events` with no wait before any of them.
fn now(events: Vec<ProviderEvent>) -> Vec<Step> {
    events.into_iter().map(|event| (Duration::ZERO, Ok(event))).collect()
}

fn at0(event: ProviderEvent) -> Step {
    (Duration::ZERO, Ok(event))
}

fn after(secs: u64, event: ProviderEvent) -> Step {
    (Duration::from_secs(secs), Ok(event))
}

fn text(text: &str) -> ProviderEvent {
    ProviderEvent::TextDelta { text: text.to_owned() }
}

fn search(query: Option<&str>) -> HostedAction {
    HostedAction::Search { query: query.map(str::to_owned) }
}

fn start(id: &str, action: HostedAction) -> ProviderEvent {
    ProviderEvent::HostedCallStart { call_id: id.to_owned(), action }
}

/// The end of the search `id` with `count` results.
fn found(id: &str, query: Option<&str>, count: usize) -> ProviderEvent {
    let results = (0..count)
        .map(|i| WebResult::new(format!("https://example.com/{id}/{i}"), Some(format!("Page {i}"))))
        .collect();
    ProviderEvent::HostedCallEnd {
        call_id: id.to_owned(),
        action: search(query),
        outcome: HostedOutcome::Done { results: Some(results) },
    }
}

fn ended(id: &str, action: HostedAction) -> ProviderEvent {
    ProviderEvent::HostedCallEnd {
        call_id: id.to_owned(),
        action,
        outcome: HostedOutcome::Done { results: None },
    }
}

fn failed(id: &str, query: &str, error: HostedError) -> ProviderEvent {
    ProviderEvent::HostedCallEnd {
        call_id: id.to_owned(),
        action: search(Some(query)),
        outcome: HostedOutcome::Failed { error },
    }
}

fn cite(url: &str, title: Option<&str>, placed: bool) -> ProviderEvent {
    ProviderEvent::Citation { url: url.to_owned(), title: title.map(str::to_owned), placed }
}

const AREWE: &str = "https://areweanticheatyet.com/game/rob-run";
const PROTONDB: &str = "https://www.protondb.com/app/2384710";
const STEAMDECK: &str = "https://www.steamdeck.com/verified/2384710";

/// The queries of TM-Web 14, in the order the model started them.
const QUERIES: [&str; 6] = [
    "\"ROB RUN\" Proton Easy Anti-Cheat",
    "\"ROB RUN\" areweanticheatyet",
    "ROB RUN protondb",
    "\"ROB RUN\" Steam Deck verified",
    "Proton EasyAntiCheat Runtime install 2026",
    "\"ROB RUN\" 1.4 patch notes Linux",
];

/// A Claude answer of the turn of TM-Web 14 and 10: a sentence, two searches one after
/// the other, three at once (TM-Web 4: they end at 5 s, 6 s and 7 s after their start,
/// in another order than they started), one search that fails, and the answer with
/// three placed citations, as the Anthropic provider gives them: each at the stop of its
/// text block, before the punctuation that follows it.
fn rob_run_answer() -> Vec<Step> {
    let q = QUERIES;
    vec![
        at0(text("I'll check what changed.")),
        at0(start("srvtoolu_1", search(Some(q[0])))),
        after(5, found("srvtoolu_1", Some(q[0]), 10)),
        at0(start("srvtoolu_2", search(Some(q[1])))),
        after(4, found("srvtoolu_2", Some(q[1]), 8)),
        at0(start("srvtoolu_3", search(Some(q[2])))),
        at0(start("srvtoolu_4", search(Some(q[3])))),
        at0(start("srvtoolu_5", search(Some(q[4])))),
        after(5, found("srvtoolu_4", Some(q[3]), 9)),
        after(1, found("srvtoolu_3", Some(q[2]), 10)),
        after(1, found("srvtoolu_5", Some(q[4]), 10)),
        at0(start("srvtoolu_6", search(Some(q[5])))),
        after(1, failed("srvtoolu_6", q[5], HostedError::TooManyRequests)),
        at0(text("The developers allowed Linux in Easy Anti-Cheat with patch 1.4")),
        at0(cite(AREWE, Some("ROB RUN · Are We Anti-Cheat Yet?"), true)),
        at0(cite(AREWE, Some("ROB RUN · Are We Anti-Cheat Yet?"), true)),
        at0(text(
            ". ProtonDB reports that it runs with Proton 10 and the Proton EasyAntiCheat \
             Runtime installed",
        )),
        at0(cite(PROTONDB, Some("ROB RUN · ProtonDB"), true)),
        at0(text(", and it is Verified on Steam Deck")),
        at0(cite(STEAMDECK, Some("Steam Deck Verified: ROB RUN"), true)),
        at0(text(". Your setup forces Proton 9 for it, and the runtime is not installed.")),
        at0(done(
            StopReason::EndTurn,
            Some(json!([{ "type": "server_tool_use", "id": "srvtoolu_1" }])),
        )),
    ]
}

/// The text of the answer of [`rob_run_answer`] after its searches, with its marks.
fn rob_run_text() -> String {
    format!(
        "The developers allowed Linux in Easy Anti-Cheat with patch 1.4 [[1]](<{AREWE}>). \
         ProtonDB reports that it runs with Proton 10 and the Proton EasyAntiCheat Runtime \
         installed [[2]](<{PROTONDB}>), and it is Verified on Steam Deck [[3]](<{STEAMDECK}>). \
         Your setup forces Proton 9 for it, and the runtime is not installed."
    )
}

/// `text` as a terminal shows it without OSC 8: each mark ` [[n]](<url>)` as ` [n]`.
fn without_destinations(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(" [[") {
        out.push_str(&rest[..at]);
        let mark = &rest[at..];
        let Some(close) = mark.find("]](<") else {
            out.push_str(mark);
            return out;
        };
        let Some(end) = mark.find(">)") else {
            out.push_str(mark);
            return out;
        };
        out.push_str(&format!(" [{}]", &mark[3..close]));
        rest = &mark[end + 2..];
    }
    out.push_str(rest);
    out
}

/// The hosted tools of a turn on a provider with the live web, by default settings.
fn live_web() -> Vec<efr_provider::HostedTool> {
    vec![efr_provider::HostedTool::WebSearch {
        access: efr_provider::WebAccess::Live,
        max_uses: Some(20),
        urls_from_tool_results: false,
    }]
}

/// Starts a conversation whose paced replay provider has the live web and answers from
/// `records`.
async fn start_replay(setup: Setup, records: Vec<efr_test_support::Record>) -> Harness {
    let provider = ReplayProvider::new(&Transcript::from_records(records))
        .expect("transcript")
        .with_web_search(WebSupport::Live)
        .paced();
    let start = ConversationStart::New { origin: Origin::Shell, tty: None, route: None };
    setup.start_provider(Arc::new(provider), start, None).await
}

/// Starts a conversation on `model` in `mode` with `web.search = setting`.
async fn start_on(
    mut setup: Setup,
    mode: Mode,
    setting: WebSearch,
    model: Arc<Scripted>,
) -> Harness {
    setup.config.mode = mode;
    setup.config.web.search = setting;
    setup.start_model(model).await
}

/// The events of the log without the updates, whose number depends on the chunks.
fn steady(events: &[Event]) -> Vec<&str> {
    events.iter().map(Event::kind).filter(|kind| *kind != "assistant_message_updated").collect()
}

/// The `(index, text)` of each completed message.
fn messages(events: &[Event]) -> Vec<(u32, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantMessageCompleted { index, text, .. } => Some((*index, text.clone())),
            _ => None,
        })
        .collect()
}

/// A web step as it ended: its action, query, count of results, error, and the seconds
/// between its start and its end.
type Ended = (WebAction, Option<String>, Option<usize>, Option<WebError>, i64);

/// Each web step as it ended, in the order of the ends.
fn steps(envelopes: &[EventEnvelope]) -> Vec<Ended> {
    let mut started = std::collections::HashMap::new();
    let mut out = Vec::new();
    for envelope in envelopes {
        match &envelope.event {
            Event::WebCallStarted { call_id, .. } => {
                started.insert(*call_id, envelope.at);
            }
            Event::WebCallCompleted { call_id, action, query, results, error, .. } => {
                let secs =
                    started.get(call_id).map_or(-1, |at| envelope.at.as_second() - at.as_second());
                out.push((*action, query.clone(), results.as_ref().map(Vec::len), *error, secs));
            }
            _ => {}
        }
    }
    out
}

/// TM-Web 4, 10, 13 and 14 from one Claude turn: the message ends before the first
/// search and the answer is the next message; each search is one start and one end with
/// its own time; three searches at once start in a row and end in their own order; the
/// failed search does not count; the marks stand before the punctuation; the report
/// counts 5 searches and lists the cited pages 1, 2, 3; and the next request carries
/// the whole answer with the provider's items.
#[tokio::test]
async fn a_claude_turn_records_its_searches_its_marks_and_its_report() {
    let setup = Setup::new();
    let model = Scripted::new(&setup.clock, WebSupport::Live, vec![rob_run_answer()]);
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, Arc::clone(&model)).await;

    let sent = h.prompt("ROB RUN quits at start under Proton. find out why").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { stop: None, .. }), "{end:?}");

    let events = h.events().await;
    let mut kinds = vec!["conversation_created", "prompt_queued", "turn_started"];
    kinds.push("assistant_message_completed");
    kinds.extend(["web_call_started", "web_call_completed"].repeat(2));
    kinds.extend(["web_call_started"; 3]);
    kinds.extend(["web_call_completed"; 3]);
    kinds.extend(["web_call_started", "web_call_completed"]);
    kinds.extend(["assistant_message_completed", "turn_web_report", "turn_completed"]);
    assert_eq!(steady(&events), kinds, "a web step is no tool call");
    assert_eq!(
        messages(&events),
        [(0, "I'll check what changed.".to_owned()), (1, rob_run_text())],
        "the message ends at the first search; the marks stand before the punctuation"
    );

    // TM-Web 14: the rows of the turn, from the same facts, in the order of the ends
    // here (the client writes them in the order of the starts).
    let frame = Frame::load("TM-Web", 14).expect("frame");
    let rows: Vec<&str> = frame.texts()[6..=11].to_vec();
    let mut expected: Vec<_> = rows
        .iter()
        .map(|row| {
            let (failed, row) = match row.strip_prefix("  ✓ web ") {
                Some(row) => (false, row),
                None => (true, row.strip_prefix("  ! web ").expect("a web row")),
            };
            let (query, tail) = row.split_once("  ").expect("two spaces end the query");
            let (count, secs) = tail.split_once(" · ").expect("the count or reason, the time");
            let secs: i64 = secs.trim_end_matches('s').parse().expect("seconds");
            let (results, error) = if failed {
                assert_eq!(count, "too many requests");
                (None, Some(WebError::TooManyRequests))
            } else {
                let count = count.trim_end_matches(" results").parse().expect("a count");
                (Some(count), None)
            };
            (WebAction::Search, Some(query.to_owned()), results, error, secs)
        })
        .collect();
    // NOTE: the three searches at once (TM-Web 4) end in the order 4, 3, 5.
    expected.swap(2, 3);
    assert_eq!(steps(&h.envelopes().await), expected);

    // TM-Web 4: the three searches at once start in a row, before any of them ends.
    let at_once: Vec<&str> = steady(&events)[8..14].to_vec();
    assert_eq!(at_once, [["web_call_started"; 3], ["web_call_completed"; 3]].concat());

    // TM-Web 10: the answer without the destinations of its marks is the board's text,
    // and the report lists the cited pages in the order of their first citation.
    let frame = Frame::load("TM-Web", 10).expect("frame");
    let texts = frame.texts();
    assert_eq!(without_destinations(&rob_run_text()), texts[0..=2].join(" "));
    let Some(Event::TurnWebReport { searches, stored_copy, stored_copy_note, sources, .. }) =
        events.iter().find(|event| matches!(event, Event::TurnWebReport { .. })).cloned()
    else {
        panic!("the turn has a report");
    };
    // TM-Web 13 and 14: five searches ran; the failed one does not count.
    assert_eq!((searches, stored_copy, stored_copy_note), (5, false, false));
    let rows: Vec<String> = sources
        .iter()
        .enumerate()
        .map(|(n, WebSource { url, title })| {
            format!("[{}] {url}  {}", n + 1, title.as_deref().unwrap_or_default())
        })
        .collect();
    assert_eq!(rows, texts[4..=6]);

    // The next request carries the whole answer as one assistant message with the
    // provider's items, so Claude reads its searches and their encrypted content again.
    let next = h.prompt("and now?").await;
    h.wait_end(next.turn_id).await;
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    let sent_back = &requests[1].messages[1];
    assert_eq!(sent_back.role, Role::Assistant);
    assert_eq!(
        sent_back.content,
        [
            ContentBlock::Text { text: "I'll check what changed.".to_owned() },
            ContentBlock::Text { text: rob_run_text() },
        ]
    );
    assert_eq!(
        sent_back.provider_raw,
        Some(json!([{ "type": "server_tool_use", "id": "srvtoolu_1" }]))
    );
}

/// The offsets of the updates and the drafts start again at 0 in the message after a
/// search, and their deltas give the completed text exactly: a client that joined late
/// shows no doubled and no lost text.
#[tokio::test]
async fn updates_and_drafts_start_again_in_the_message_after_a_search() {
    let setup = Setup::new();
    let mut drafts = setup.drafts.subscribe();
    let model = Scripted::new(&setup.clock, WebSupport::Live, vec![rob_run_answer()]);
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, model).await;

    let sent = h.prompt("ROB RUN quits at start under Proton. find out why").await;
    h.wait_end(sent.turn_id).await;

    let events = h.events().await;
    let mut updated: Vec<String> = vec![String::new(); 2];
    for event in &events {
        if let Event::AssistantMessageUpdated { index, offset, delta, .. } = event {
            let text = &mut updated[*index as usize];
            assert_eq!(*offset as usize, text.len(), "message {index}");
            text.push_str(delta);
        }
    }
    assert_eq!(updated, ["I'll check what changed.".to_owned(), rob_run_text()]);
    let mut drafted: Vec<String> = vec![String::new(); 2];
    while let Ok(ConversationDraft { part, .. }) = drafts.try_recv() {
        if let DraftPart::Text { index, offset, delta } = part {
            let text = &mut drafted[index as usize];
            assert_eq!(offset as usize, text.len(), "draft of message {index}");
            text.push_str(&delta);
        }
    }
    assert_eq!(drafted, updated);
    assert!(h.handle.live_turn().is_none_or(|live| live.text.is_empty()));
}

/// An OpenAI answer: a search, an open and a find, and the citations unplaced at the
/// end of the message. The text stays as the model wrote it, with its own Markdown link;
/// the sources come in the report; only the search counts.
#[tokio::test]
async fn an_openai_answer_keeps_its_text_and_lists_its_unplaced_sources() {
    let setup = Setup::new();
    let page = Some(AREWE.to_owned());
    let answer = now(vec![
        start("ws_1", search(None)),
        ended("ws_1", search(Some("\"ROB RUN\" areweanticheatyet"))),
        start("ws_2", HostedAction::Open { url: None }),
        ended("ws_2", HostedAction::Open { url: page.clone() }),
        start(
            "ws_3",
            HostedAction::Find { pattern: "Easy Anti-Cheat".to_owned(), url: page.clone() },
        ),
        ended("ws_3", HostedAction::Find { pattern: "Easy Anti-Cheat".to_owned(), url: page }),
        text(
            "The developers allowed Linux with patch 1.4 ([areweanticheatyet.com](https://areweanticheatyet.com/game/rob-run)).",
        ),
        cite(AREWE, Some("ROB RUN · Are We Anti-Cheat Yet?"), false),
        done(StopReason::EndTurn, None),
    ]);
    let model = Scripted::new(&setup.clock, WebSupport::StoredOrLive, vec![answer]);
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, model).await;

    let sent = h.prompt("does ROB RUN work on Linux").await;
    h.wait_end(sent.turn_id).await;

    let events = h.events().await;
    assert_eq!(
        messages(&events),
        [(
            0,
            "The developers allowed Linux with patch 1.4 \
             ([areweanticheatyet.com](https://areweanticheatyet.com/game/rob-run))."
                .to_owned()
        )]
    );
    let ends: Vec<_> = steps(&h.envelopes().await)
        .into_iter()
        .map(|(action, query, results, error, _)| (action, query, results, error))
        .collect();
    assert_eq!(
        ends,
        [
            (WebAction::Search, Some("\"ROB RUN\" areweanticheatyet".to_owned()), None, None),
            (WebAction::Open, None, None, None),
            (WebAction::Find, None, None, None),
        ],
        "the end has the query that the start lacked; OpenAI gives no count"
    );
    let completed: Vec<&Event> =
        events.iter().filter(|event| matches!(event, Event::WebCallCompleted { .. })).collect();
    assert!(
        matches!(completed[2], Event::WebCallCompleted { url: Some(url), pattern: Some(pattern), .. }
            if url == AREWE && pattern == "Easy Anti-Cheat"),
        "{completed:?}"
    );
    assert_eq!(
        events.iter().find(|event| matches!(event, Event::TurnWebReport { .. })),
        Some(&Event::TurnWebReport {
            turn_id: sent.turn_id,
            searches: 1,
            stored_copy: false,
            stored_copy_note: false,
            sources: vec![WebSource {
                url: AREWE.to_owned(),
                title: Some("ROB RUN · Are We Anti-Cheat Yet?".to_owned()),
            }],
        })
    );
}

/// TM-Web 13: on a provider with a stored copy, a cautious turn reads it; its report
/// says so, and only the first such report of the conversation has the note, also after
/// a restart of the actor, which reads the earlier report from the log.
#[tokio::test]
async fn the_stored_copy_is_said_with_its_note_once_per_conversation() {
    let setup = Setup::new();
    let one_search = || {
        now(vec![
            start("ws_1", search(None)),
            ended("ws_1", search(Some("rust 2026"))),
            text("Rust 2026 is out."),
            done(StopReason::EndTurn, None),
        ])
    };
    let clock = setup.clock.clone();
    let model = Scripted::new(&clock, WebSupport::StoredOrLive, vec![one_search(), one_search()]);
    let mut h = start_on(setup, Mode::Cautious, WebSearch::Mode, Arc::clone(&model)).await;

    let mut notes = Vec::new();
    for prompt in ["what changed in rust 2026", "and in 2025"] {
        let sent = h.prompt(prompt).await;
        h.wait_end(sent.turn_id).await;
    }
    let restarted = Scripted::new(&clock, WebSupport::StoredOrLive, vec![one_search()]);
    let mut h = h.restart_model(Arc::clone(&restarted) as Arc<dyn Provider>).await;
    let sent = h.prompt("and in 2024").await;
    h.wait_end(sent.turn_id).await;
    for event in h.events().await {
        if let Event::TurnWebReport { searches, stored_copy, stored_copy_note, .. } = event {
            notes.push((searches, stored_copy, stored_copy_note));
        }
    }
    assert_eq!(notes, [(1, true, true), (1, true, false), (1, true, false)]);
    let requests = [model.requests(), restarted.requests()].concat();
    assert!(requests.iter().all(|request| request.hosted
        == [efr_provider::HostedTool::WebSearch {
            access: efr_provider::WebAccess::Stored,
            max_uses: Some(20),
            urls_from_tool_results: false,
        }]));
    // The preamble of each prompt says what the stored copy is.
    let first = &requests[0].messages[0].content[0];
    assert!(matches!(first, ContentBlock::Text { text } if text.contains(WEB_STORED)), "{first:?}");
}

/// TM-Web 9: in manual mode the request carries no web tool, the preamble tells the
/// model that search is off and how the user turns it on, and efr records nothing about
/// web search.
#[tokio::test]
async fn a_manual_turn_has_no_tool_and_the_line_and_no_web_event() {
    let setup = Setup::new();
    let state = LiveState {
        mode: Mode::Manual,
        web: WebPlan::Off(WhyOff::Manual),
        ..setup.live_state(&setup.cwd, "does ROB RUN work on Linux by now?")
    };
    let model = Scripted::new(&setup.clock, WebSupport::Live, Vec::new());
    let mut h = start_on(setup, Mode::Manual, WebSearch::Live, Arc::clone(&model)).await;

    let sent = h.prompt("does ROB RUN work on Linux by now?").await;
    h.wait_end(sent.turn_id).await;

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].hosted, []);
    let preamble = state.render();
    assert!(preamble.contains(WEB_OFF_MANUAL), "{preamble}");
    assert_eq!(
        requests[0].messages[0].content[0],
        ContentBlock::Text { text: preamble },
        "the preamble of the turn has the line"
    );
    let kinds = h.kinds().await;
    assert!(
        !kinds.iter().any(|kind| kind.starts_with("web_") || kind == "turn_web_report"),
        "{kinds:?}"
    );
}

/// A turn that failed inside a search: the message before it is completed once, the
/// search that never ended ends as not finished, no report comes (no search ran), and
/// the next request carries the text that streamed.
#[tokio::test]
async fn a_failed_turn_ends_its_open_search_as_not_finished() {
    let setup = Setup::new();
    let answer = vec![
        at0(text("Looking.")),
        at0(start("srvtoolu_1", search(Some("rust 2026")))),
        (Duration::from_secs(2), Err(ProviderError::Incomplete)),
    ];
    let model = Scripted::new(&setup.clock, WebSupport::Live, vec![answer]);
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, Arc::clone(&model)).await;

    let sent = h.prompt("what changed in rust 2026").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnFailed { .. }), "{end:?}");
    let events = h.events().await;
    assert_eq!(messages(&events), [(0, "Looking.".to_owned())]);
    assert_eq!(
        steady(&events)[3..],
        ["assistant_message_completed", "web_call_started", "web_call_completed", "turn_failed"]
    );
    assert_eq!(
        steps(&h.envelopes().await),
        [(WebAction::Search, Some("rust 2026".to_owned()), None, Some(WebError::NotFinished), 2)]
    );

    let next = h.prompt("go on").await;
    h.wait_end(next.turn_id).await;
    let requests = model.requests();
    assert_eq!(requests[1].messages[1], Message::assistant("Looking."));
}

/// After an interrupt in the text after a search, only that text is completed as its
/// own message; the transcript keeps the whole text of the call.
#[tokio::test]
async fn an_interrupt_after_a_search_completes_only_the_open_message() {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    let state =
        LiveState { mode: Mode::Auto, ..setup.live_state(&setup.cwd, "what changed in rust 2026") };
    let first = setup.prompt(&state, "what changed in rust 2026");
    let records = vec![
        expect_request(Request { hosted: live_web(), ..request(vec![first]) }),
        answer(&[
            text("Looking."),
            start("srvtoolu_1", search(Some("rust 2026"))),
            found("srvtoolu_1", Some("rust 2026"), 2),
            text("Rust 2026 has"),
        ]),
        hold(),
        answer(&[done(StopReason::EndTurn, None)]),
    ];
    let mut h = start_replay(setup, records).await;

    let sent = h.prompt("what changed in rust 2026").await;
    h.wait_for(|event| matches!(event, Event::AssistantMessageUpdated { index: 1, .. })).await;
    let params = h.interrupt_params(Some(sent.turn_id), Vec::new(), Vec::new());
    h.handle.interrupt(params, Origin::Shell).await.expect("interrupt accepted");
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnInterrupted { .. }), "{end:?}");

    let events = h.events().await;
    assert_eq!(messages(&events), [(0, "Looking.".to_owned()), (1, "Rust 2026 has".to_owned())]);
    assert!(
        events.iter().any(|event| matches!(event, Event::TurnWebReport { searches: 1, .. })),
        "an interrupted turn reports its searches too"
    );
}

/// A paused turn goes on: the next request ends with the paused answer as it came, with
/// no note between, and the step that the pause left open ends in the next answer with
/// the id of its start. A note that came during the pause waits for the call after.
#[tokio::test]
async fn a_paused_turn_goes_on_and_its_search_ends_in_the_next_answer() {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    let state = LiveState { mode: Mode::Auto, ..setup.live_state(&setup.cwd, "check rust 2026") };
    let first = setup.prompt(&state, "check rust 2026");
    let web = |messages| Request { hosted: live_web(), ..request(messages) };
    let paused_raw = json!([{ "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": { "query": "rust 2026" } }]);
    let paused = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text { text: "Checking.".to_owned() }],
        provider_raw: Some(paused_raw.clone()),
    };
    let notes = setup.home().join("notes.txt");
    let path = json!({ "path": notes });
    let goes_on = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Text { text: "Found it.".to_owned() },
            ContentBlock::ToolCall {
                call_id: "toolu_1".to_owned(),
                name: "read_file".to_owned(),
                input: path.clone(),
                freeform: false,
            },
        ],
        provider_raw: None,
    };
    let note = "the port 8080 of job 1 is forwarded".to_owned();
    let read = result_message("toolu_1", &format!("contents of {}", notes.display()), false);
    let records = vec![
        expect_request(web(vec![first.clone()])),
        answer(&[text("Checking."), start("srvtoolu_1", search(Some("rust 2026")))]),
        hold(),
        answer(&[done(StopReason::PauseTurn, Some(paused_raw))]),
        expect_request(web(vec![first.clone(), paused.clone()])),
        answer(&[
            found("srvtoolu_1", None, 3),
            text("Found it."),
            ProviderEvent::ToolCallStart {
                call_id: "toolu_1".to_owned(),
                name: "read_file".to_owned(),
                freeform: false,
            },
            ProviderEvent::ToolCallEnd {
                call_id: "toolu_1".to_owned(),
                arguments: path.to_string(),
            },
            done(StopReason::ToolUse, None),
        ]),
        expect_request(web(vec![
            first,
            paused,
            goes_on,
            read,
            Message::user(notice::notes_text(std::slice::from_ref(&note))),
        ])),
        answer(&text_answer("Rust 2026 is out.")),
    ];
    let mut h = start_replay(setup, records).await;

    let sent = h.prompt("check rust 2026").await;
    h.wait_for(|event| matches!(event, Event::WebCallStarted { .. })).await;
    h.toolbox.notes.lock().unwrap_or_else(PoisonError::into_inner).0.push(note);
    h.provider.handled_through(3);
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { stop: None, .. }), "{end:?}");
    h.finish();

    let events = h.events().await;
    let started = events.iter().find_map(|event| match event {
        Event::WebCallStarted { call_id, .. } => Some(*call_id),
        _ => None,
    });
    let completed = events.iter().find_map(|event| match event {
        Event::WebCallCompleted { call_id, query, results, .. } => {
            Some((*call_id, query.clone(), results.as_ref().map(Vec::len)))
        }
        _ => None,
    });
    assert_eq!(completed, started.map(|id| (id, Some("rust 2026".to_owned()), Some(3))));
    assert_eq!(
        messages(&events),
        [
            (0, "Checking.".to_owned()),
            (1, "Found it.".to_owned()),
            (2, "Rust 2026 is out.".to_owned())
        ]
    );
}

/// A provider that pauses every answer: the turn goes on five times, then ends as one
/// that the provider paused.
#[tokio::test]
async fn a_turn_that_pauses_again_and_again_stops_after_five_continuations() {
    let setup = Setup::new();
    let pause = || now(vec![text("Still searching."), done(StopReason::PauseTurn, None)]);
    let model = Scripted::new(&setup.clock, WebSupport::Live, (0..10).map(|_| pause()).collect());
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, Arc::clone(&model)).await;

    let sent = h.prompt("search everything").await;
    let end = h.wait_end(sent.turn_id).await;
    assert!(
        matches!(
            end,
            Event::TurnCompleted {
                stop: Some(TurnStop { reason: WireStop::PauseTurn, empty: false }),
                ..
            }
        ),
        "{end:?}"
    );
    assert_eq!(model.requests().len(), 1 + super::super::hosted::MAX_PAUSES as usize);
}

/// A search and a client tool call in one answer (Anthropic: `stop_reason: tool_use`,
/// no result block yet): the request after it ends with the results of the tool call,
/// with no steer after them, and the provider runs the search in that call. The steer
/// waits for the call after it.
#[tokio::test]
async fn a_steer_waits_one_call_after_a_search_that_waits_for_a_tool_result() {
    let mut setup = Setup::new();
    setup.config.mode = Mode::Auto;
    let state = LiveState { mode: Mode::Auto, ..setup.live_state(&setup.cwd, "check the notes") };
    let first = setup.prompt(&state, "check the notes");
    let web = |messages| Request { hosted: live_web(), ..request(messages) };
    let notes = setup.home().join("notes.txt");
    let path = json!({ "path": notes });
    let raw = json!([{ "type": "server_tool_use", "id": "srvtoolu_1" }, { "type": "tool_use", "id": "toolu_1" }]);
    let both = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolCall {
            call_id: "toolu_1".to_owned(),
            name: "read_file".to_owned(),
            input: path.clone(),
            freeform: false,
        }],
        provider_raw: Some(raw.clone()),
    };
    let read = result_message("toolu_1", &format!("contents of {}", notes.display()), false);
    let records = vec![
        expect_request(web(vec![first.clone()])),
        answer(&[
            start("srvtoolu_1", search(Some("notes.txt format"))),
            ProviderEvent::ToolCallStart {
                call_id: "toolu_1".to_owned(),
                name: "read_file".to_owned(),
                freeform: false,
            },
        ]),
        hold(),
        answer(&[
            ProviderEvent::ToolCallEnd {
                call_id: "toolu_1".to_owned(),
                arguments: path.to_string(),
            },
            done(StopReason::ToolUse, Some(raw)),
        ]),
        expect_request(web(vec![first.clone(), both.clone(), read.clone()])),
        answer(&[
            found("srvtoolu_1", None, 4),
            text("Read them."),
            done(StopReason::EndTurn, None),
        ]),
        expect_request(web(vec![
            first,
            both,
            read,
            Message::assistant("Read them."),
            Message::user("and the old ones"),
        ])),
        answer(&text_answer("The old ones too.")),
    ];
    let mut h = start_replay(setup, records).await;

    let sent = h.prompt("check the notes").await;
    h.wait_for(|event| matches!(event, Event::WebCallStarted { .. })).await;
    let steer = h.steer_params(Some(sent.turn_id), "and the old ones", false);
    h.handle.steer(steer, Origin::Shell).await.expect("steer accepted");
    h.provider.handled_through(3);
    let end = h.wait_end(sent.turn_id).await;
    assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    h.finish();

    let kinds: Vec<String> = h.kinds().await;
    let at = |kind: &str| kinds.iter().position(|k| k == kind).expect(kind);
    assert!(at("web_call_completed") < at("steering_delivered"), "{kinds:?}");
    assert!(at("tool_call_completed") < at("web_call_completed"), "{kinds:?}");
    assert!(kinds.iter().any(|kind| kind == "turn_web_report"));
}

/// The `[n]` of a page that is cited again later in the turn is the same number, also
/// in a later answer of the turn.
#[tokio::test]
async fn a_page_keeps_its_number_for_the_whole_turn() {
    let setup = Setup::new();
    let first = now(vec![
        start("srvtoolu_1", search(Some("a"))),
        found("srvtoolu_1", Some("a"), 2),
        text("One"),
        cite(AREWE, None, true),
        text("."),
        ProviderEvent::ToolCallStart {
            call_id: "toolu_1".to_owned(),
            name: "read_file".to_owned(),
            freeform: false,
        },
        ProviderEvent::ToolCallEnd {
            call_id: "toolu_1".to_owned(),
            arguments: json!({ "path": "a" }).to_string(),
        },
        done(StopReason::ToolUse, None),
    ]);
    let second = now(vec![
        text("Two"),
        cite(PROTONDB, None, true),
        text(" and one"),
        cite(AREWE, None, true),
        text("."),
        done(StopReason::EndTurn, None),
    ]);
    let model = Scripted::new(&setup.clock, WebSupport::Live, vec![first, second]);
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, model).await;

    let sent = h.prompt("cite twice").await;
    h.wait_end(sent.turn_id).await;
    let events = h.events().await;
    assert_eq!(
        messages(&events),
        [
            (0, format!("One [[1]](<{AREWE}>).")),
            (1, format!("Two [[2]](<{PROTONDB}>) and one [[1]](<{AREWE}>).")),
        ]
    );
    let sources = events.iter().find_map(|event| match event {
        Event::TurnWebReport { sources, .. } => {
            Some(sources.iter().map(|s| s.url.clone()).collect::<Vec<_>>())
        }
        _ => None,
    });
    assert_eq!(sources, Some(vec![AREWE.to_owned(), PROTONDB.to_owned()]));
}

/// A switch of the model drops the provider's items, and the new model reads the
/// canonical answer: its text split at the search, with the marks, so the URLs of the
/// sources stay (the build spec of web search, 3.9).
#[tokio::test]
async fn a_switch_of_model_sends_the_canonical_text_with_its_marks() {
    let setup = Setup::new();
    let models = vec![ModelInfo::new("claude-a"), ModelInfo::new(MODEL)];
    let model =
        Scripted::new(&setup.clock, WebSupport::Live, vec![rob_run_answer()]).with_models(models);
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, Arc::clone(&model)).await;

    for (on, prompt) in [("claude-a", "find out why"), (MODEL, "and now?")] {
        let cwd = h.cwd.clone();
        let mut params = h.prompt_params(&cwd, prompt);
        params.settings.model = Some(on.to_owned());
        let sent = h.handle.send_prompt(params, Origin::Shell).await.expect("prompt accepted");
        let end = h.wait_end(sent.turn_id).await;
        assert!(matches!(end, Event::TurnCompleted { .. }), "{end:?}");
    }
    let requests = model.requests();
    let answer = &requests[1].messages[1];
    assert_eq!(answer.provider_raw, None, "another model reads no item of the first");
    assert_eq!(
        answer.content,
        [
            ContentBlock::Text { text: "I'll check what changed.".to_owned() },
            ContentBlock::Text { text: rob_run_text() },
        ]
    );
}

/// A turn rebuilt from its events (no saved messages) ignores the web events: the
/// answer is in its `assistant_message_completed` events.
#[tokio::test]
async fn a_rebuilt_turn_ignores_the_web_events() {
    let setup = Setup::new();
    let model = Scripted::new(&setup.clock, WebSupport::Live, vec![rob_run_answer()]);
    let mut h = start_on(setup, Mode::Auto, WebSearch::Mode, model).await;
    let sent = h.prompt("find out why").await;
    h.wait_end(sent.turn_id).await;

    let envelopes = h.envelopes().await;
    let all: Vec<&EventEnvelope> = envelopes
        .iter()
        .filter(|envelope| envelope.event.turn_id() == Some(sent.turn_id))
        .collect();
    let without: Vec<&EventEnvelope> = all
        .iter()
        .copied()
        .filter(|envelope| {
            !matches!(
                envelope.event,
                Event::WebCallStarted { .. }
                    | Event::WebCallCompleted { .. }
                    | Event::TurnWebReport { .. }
            )
        })
        .collect();
    assert!(without.len() < all.len());
    let status = efr_store::conversations::TurnStatus::Completed;
    let rebuilt = crate::history::rebuild("find out why", status, &all);
    assert_eq!(rebuilt, crate::history::rebuild("find out why", status, &without));
    let texts: Vec<String> = rebuilt.iter().map(Message::text).collect();
    assert_eq!(
        texts,
        ["find out why".to_owned(), "I'll check what changed.".to_owned(), rob_run_text()]
    );
}
