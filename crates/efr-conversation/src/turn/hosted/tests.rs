//! The web state of a turn without a turn around it: the events of the steps, the
//! count, the numbers of the sources and the marks.

use std::sync::atomic::AtomicBool;

use efr_protocol::{
    CallId, Event, MAX_WEB_RESULTS, MAX_WEB_SOURCES, MAX_WEB_TEXT_CHARS, MAX_WEB_TITLE_CHARS,
    MAX_WEB_URL_CHARS, TurnId, WebAction, WebError, WebSource,
};
use efr_provider::{HostedAction, HostedError, HostedOutcome, WebResult};
use efr_stdx::id::uuid_v7;
use efr_test_support::{TestClock, TestRng};
use pretty_assertions::assert_eq;

use super::{WebState, mark};
use crate::web::{WebPlan, WhyOff};

fn turn() -> TurnId {
    TurnId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(7)))
}

fn id(n: u128) -> CallId {
    CallId::from_uuid(uuid_v7(&TestClock::new(), &TestRng::new(100 + n as u64)))
}

fn search(query: Option<&str>) -> HostedAction {
    HostedAction::Search { query: query.map(str::to_owned) }
}

fn done(results: Option<usize>) -> HostedOutcome {
    HostedOutcome::Done {
        results: results.map(|n| {
            (0..n).map(|i| WebResult::new(format!("https://example.com/{i}"), None)).collect()
        }),
    }
}

fn failed(error: HostedError) -> HostedOutcome {
    HostedOutcome::Failed { error }
}

#[test]
fn a_step_records_its_start_and_its_end_with_the_query_of_the_end() {
    let mut web = WebState::new(WebPlan::Live);
    let started = web.start(turn(), "ws_1", &search(None), id(1));
    assert_eq!(
        started,
        Some(Event::WebCallStarted {
            turn_id: turn(),
            call_id: id(1),
            action: WebAction::Search,
            query: None,
            url: None,
            pattern: None,
        })
    );
    assert!(web.pending(), "the step started in this answer and did not end");
    let events =
        web.end(turn(), "ws_1", &search(Some("\"ROB RUN\" Proton")), &done(Some(2)), || {
            unreachable!("the start came")
        });
    let [Event::WebCallCompleted { call_id, action, query, results, error, .. }] =
        events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert_eq!((*call_id, *action), (id(1), WebAction::Search));
    assert_eq!(query.as_deref(), Some("\"ROB RUN\" Proton"));
    assert_eq!(results.as_ref().map(Vec::len), Some(2));
    assert_eq!(*error, None);
    assert_eq!(web.searches, 1);
    assert!(!web.pending());
}

/// A start can say the query and its end not (Anthropic's result block of a search
/// that a client tool call moved to the next answer): the completion keeps the query.
#[test]
fn the_end_keeps_the_subject_of_the_start_when_it_lacks_one() {
    let mut web = WebState::new(WebPlan::Live);
    web.start(turn(), "srvtoolu_1", &search(Some("rust 2026")), id(1));
    web.begin_answer();
    assert!(!web.pending(), "the step started in an earlier answer");
    let events = web.end(turn(), "srvtoolu_1", &search(None), &done(Some(0)), || id(9));
    let [Event::WebCallCompleted { query, results, .. }] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!(query.as_deref(), Some("rust 2026"));
    assert_eq!(results.as_deref(), Some(&[][..]), "0 results is said");
}

#[test]
fn an_end_without_a_start_opens_and_closes_the_step() {
    let mut web = WebState::new(WebPlan::Live);
    let page = HostedAction::Open { url: Some("https://example.com/a".to_owned()) };
    let events = web.end(turn(), "srvtoolu_9", &page, &done(None), || id(4));
    let kinds: Vec<&str> = events.iter().map(Event::kind).collect();
    assert_eq!(kinds, ["web_call_started", "web_call_completed"]);
    assert!(events.iter().all(|event| matches!(
        event,
        Event::WebCallStarted { call_id, action: WebAction::Open, .. }
            | Event::WebCallCompleted { call_id, action: WebAction::Open, .. }
            if *call_id == id(4)
    )));
    assert_eq!(web.searches, 0, "an open is no search");
}

/// TM-Web 13 and 14: only searches that ran count; a failed search, an open and a find
/// do not.
#[test]
fn only_searches_that_ran_count() {
    let mut web = WebState::new(WebPlan::Live);
    let steps = [
        (search(Some("a")), done(Some(10))),
        (search(Some("b")), failed(HostedError::TooManyRequests)),
        (HostedAction::Open { url: Some("https://example.com".to_owned()) }, done(None)),
        (HostedAction::Find { pattern: "x".to_owned(), url: None }, done(None)),
        (search(None), done(None)),
    ];
    for (n, (action, outcome)) in steps.iter().enumerate() {
        let provider_id = format!("ws_{n}");
        web.start(turn(), &provider_id, action, id(n as u128));
        let events = web.end(turn(), &provider_id, action, outcome, || unreachable!());
        if matches!(outcome, HostedOutcome::Failed { .. }) {
            assert!(
                matches!(
                    events.as_slice(),
                    [Event::WebCallCompleted { error: Some(WebError::TooManyRequests), .. }]
                ),
                "{events:?}"
            );
        }
    }
    assert_eq!(web.searches, 2);
}

#[test]
fn every_hosted_error_has_its_wire_word() {
    let pairs = [
        (HostedError::TooManyRequests, WebError::TooManyRequests),
        (HostedError::LimitReached, WebError::LimitReached),
        (HostedError::Unavailable, WebError::Unavailable),
        (HostedError::Invalid, WebError::Invalid),
        (HostedError::NotAllowed, WebError::NotAllowed),
        (HostedError::NotAccessible, WebError::NotAccessible),
        (HostedError::Unsupported, WebError::Unsupported),
        (HostedError::NotFinished, WebError::NotFinished),
        (HostedError::Other, WebError::Unknown),
    ];
    for (error, wire) in pairs {
        assert_eq!(super::web_error(error), wire);
    }
}

#[test]
fn long_strings_and_long_result_lists_are_cut() {
    let mut web = WebState::new(WebPlan::Live);
    let query = "q".repeat(MAX_WEB_TEXT_CHARS + 5);
    let results = (0..MAX_WEB_RESULTS + 3)
        .map(|i| {
            WebResult::new(
                format!("https://example.com/{i}/{}", "u".repeat(MAX_WEB_URL_CHARS)),
                Some("t".repeat(MAX_WEB_TITLE_CHARS + 1)),
            )
        })
        .collect();
    let outcome = HostedOutcome::Done { results: Some(results) };
    let events = web.end(turn(), "ws_1", &search(Some(&query)), &outcome, || id(1));
    let Some(Event::WebCallCompleted { query: Some(query), results: Some(results), .. }) =
        events.last()
    else {
        panic!("{events:?}");
    };
    assert_eq!(query.chars().count(), MAX_WEB_TEXT_CHARS);
    assert_eq!(results.len(), MAX_WEB_RESULTS);
    assert!(results.iter().all(|result| result.url.chars().count() == MAX_WEB_URL_CHARS));
    assert!(results.iter().all(|result| {
        result.title.as_ref().is_some_and(|title| title.chars().count() == MAX_WEB_TITLE_CHARS)
    }));
}

/// TM-Web 10: one number per page for the whole turn, in the order of the first
/// citation; a known page keeps its number; two citations of one run with the same
/// number give one mark.
#[test]
fn citations_number_each_page_once_in_the_order_of_the_first() {
    let mut web = WebState::new(WebPlan::Live);
    let a = "https://areweanticheatyet.com/game/rob-run";
    let b = "https://www.protondb.com/app/2384710";
    assert_eq!(web.cite(a, Some("ROB RUN · Are We Anti-Cheat Yet?"), true), Some(mark(1, a)));
    assert_eq!(web.cite(a, None, true), None, "the run has the mark of 1");
    web.text_grew();
    assert_eq!(web.cite(b, Some("ROB RUN · ProtonDB"), true), Some(mark(2, b)));
    assert_eq!(web.cite(a, None, true), Some(mark(1, a)), "two numbers in one run");
    web.text_grew();
    assert_eq!(web.cite(a, None, true), Some(mark(1, a)), "the same page later in the text");
    assert_eq!(web.cite("https://example.com/c", None, false), None, "unplaced: no mark");
    let Some(Event::TurnWebReport { sources, searches, .. }) =
        web.report(turn(), &AtomicBool::new(false))
    else {
        panic!("a turn that cited a page has a report");
    };
    assert_eq!(searches, 0);
    assert_eq!(
        sources,
        [
            WebSource {
                url: a.to_owned(),
                title: Some("ROB RUN · Are We Anti-Cheat Yet?".to_owned())
            },
            WebSource { url: b.to_owned(), title: Some("ROB RUN · ProtonDB".to_owned()) },
            WebSource { url: "https://example.com/c".to_owned(), title: None },
        ]
    );
}

#[test]
fn at_most_the_cap_of_sources_get_a_number() {
    let mut web = WebState::new(WebPlan::Live);
    for i in 0..MAX_WEB_SOURCES {
        web.text_grew();
        assert!(web.cite(&format!("https://example.com/{i}"), None, true).is_some());
    }
    web.text_grew();
    assert_eq!(web.cite("https://example.com/more", None, true), None);
    let Some(Event::TurnWebReport { sources, .. }) = web.report(turn(), &AtomicBool::new(false))
    else {
        panic!("a report");
    };
    assert_eq!(sources.len(), MAX_WEB_SOURCES);
}

/// The mark is a Markdown link whose destination is always valid: in angle brackets,
/// with `<`, `>`, white space and control characters percent-encoded.
#[test]
fn the_mark_encodes_what_would_break_its_destination() {
    assert_eq!(mark(1, "https://example.com/a"), " [[1]](<https://example.com/a>)");
    assert_eq!(
        mark(12, "https://example.com/a b<c>\u{7}%20"),
        " [[12]](<https://example.com/a%20b%3Cc%3E%07%20>)"
    );
    assert_eq!(mark(2, "https://example.com/\u{a0}x"), " [[2]](<https://example.com/%C2%A0x>)");
    // `\>` would read as an escaped `>`: the link would not close and show no `[1]`.
    assert_eq!(mark(1, "https://a.example/b\\"), " [[1]](<https://a.example/b%5C>)");
}

/// TM-Web 13: the stored copy is said with the count, and its note only in the first
/// report of the conversation that has it.
#[test]
fn the_stored_copy_and_its_note_once() {
    let noted = AtomicBool::new(false);
    let ran = |plan| {
        let mut web = WebState::new(plan);
        web.end(turn(), "ws_1", &search(Some("a")), &done(None), || id(1));
        web
    };
    let first = ran(WebPlan::Stored).report(turn(), &noted);
    assert!(
        matches!(
            first,
            Some(Event::TurnWebReport {
                searches: 1,
                stored_copy: true,
                stored_copy_note: true,
                ..
            })
        ),
        "{first:?}"
    );
    let second = ran(WebPlan::Stored).report(turn(), &noted);
    assert!(
        matches!(
            second,
            Some(Event::TurnWebReport { stored_copy: true, stored_copy_note: false, .. })
        ),
        "{second:?}"
    );
    let mut earlier = ran(WebPlan::Stored);
    earlier.noted_before();
    let after_restart = earlier.report(turn(), &AtomicBool::new(false));
    assert!(
        matches!(after_restart, Some(Event::TurnWebReport { stored_copy_note: false, .. })),
        "an earlier turn of the log had the note: {after_restart:?}"
    );
    let live = ran(WebPlan::Live).report(turn(), &AtomicBool::new(false));
    assert!(
        matches!(
            live,
            Some(Event::TurnWebReport { stored_copy: false, stored_copy_note: false, .. })
        ),
        "{live:?}"
    );
}

#[test]
fn no_report_without_a_search_that_ran_or_a_source() {
    let mut web = WebState::new(WebPlan::Stored);
    web.end(turn(), "ws_1", &search(Some("a")), &failed(HostedError::Unavailable), || id(1));
    assert_eq!(web.report(turn(), &AtomicBool::new(false)), None);
    let off = WebState::new(WebPlan::Off(WhyOff::Manual));
    assert_eq!(off.report(turn(), &AtomicBool::new(false)), None);
}

#[test]
fn steps_that_never_ended_end_as_not_finished() {
    let mut web = WebState::new(WebPlan::Live);
    web.start(turn(), "srvtoolu_1", &search(Some("a")), id(1));
    web.start(turn(), "srvtoolu_2", &search(Some("b")), id(2));
    assert_eq!(web.start(turn(), "srvtoolu_1", &search(Some("a")), id(3)), None, "one start");
    let events = web.unfinished(turn());
    let ends: Vec<(CallId, Option<WebError>)> = events
        .iter()
        .map(|event| match event {
            Event::WebCallCompleted { call_id, error, .. } => (*call_id, *error),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        ends,
        [(id(1), Some(WebError::NotFinished)), (id(2), Some(WebError::NotFinished))],
        "in the order of their starts"
    );
    assert_eq!(web.searches, 0);
    assert!(web.unfinished(turn()).is_empty());
}
