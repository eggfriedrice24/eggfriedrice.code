//! `efr history` of a turn that searched the web, against TM-Web 14 and 15: the same
//! rows as the turn, and with `--verbose` the results under a search and the finds
//! under an opened page.

use efr_config::DiffGroups;
use efr_protocol::{
    CommandId, ContextUse, ConversationHistoryResult, EffectiveSettings, Event, EventEnvelope,
    Mode, Origin, OverriddenSettings, Scope, TurnId, TurnSettings, Usage, WebAction, WebError,
    WebResult, WebSource,
};
use efr_test_support::design::{Depth, Frame, assert_rows};
use jiff::tz::TimeZone;
use pretty_assertions::assert_eq;

use super::super::{Head, Shown, transcript};
use crate::testing::{bare, design_options, envelope};

fn turn_id() -> TurnId {
    "019a7c02-3d00-7a10-8b20-000000000002".parse().unwrap()
}

fn call(n: u8) -> efr_protocol::CallId {
    format!("019a7c02-3d00-7a10-8b20-0000000004{n:02x}").parse().unwrap()
}

fn at(seconds: f64) -> jiff::Timestamp {
    let start: jiff::Timestamp = "2026-11-12T21:14:00Z".parse().unwrap();
    start + jiff::SignedDuration::from_secs_f64(seconds)
}

/// Step `n` of the web tool: a search of `query`, or an open or a find of `url`.
fn step(
    n: u8,
    action: WebAction,
    query: Option<&str>,
    url: Option<&str>,
    pattern: Option<&str>,
) -> Event {
    Event::WebCallStarted {
        turn_id: turn_id(),
        call_id: call(n),
        action,
        query: query.map(str::to_owned),
        url: url.map(str::to_owned),
        pattern: pattern.map(str::to_owned),
    }
}

fn ended(started: &Event, results: Option<Vec<WebResult>>, error: Option<WebError>) -> Event {
    let Event::WebCallStarted { call_id, action, query, url, pattern, .. } = started.clone() else {
        panic!("not a start: {started:?}");
    };
    Event::WebCallCompleted {
        turn_id: turn_id(),
        call_id,
        action,
        query,
        url,
        pattern,
        results,
        error,
    }
}

fn results(count: usize) -> Vec<WebResult> {
    (0..count).map(|n| WebResult { url: format!("https://example.com/{n}"), title: None }).collect()
}

/// A turn of `steps`, each a start at a second and its end at another, then the report
/// with `sources` and the end of the turn after 1m 48s.
fn page(steps: Vec<(f64, Event)>, sources: Vec<WebSource>) -> ConversationHistoryResult {
    let command_id: CommandId = "019a7c02-3d00-7a10-8b20-0000000000c1".parse().unwrap();
    let mut usage = Usage::new(30_000, 3_100);
    usage.cached_input_tokens = 26_700;
    let mut events = vec![
        (
            0.0,
            Event::PromptQueued {
                turn_id: turn_id(),
                command_id,
                text: "ROB RUN quits at start under Proton. find out why and fix it".to_owned(),
                origin: Origin::Shell,
                context: None,
                settings: TurnSettings::default(),
                steers: Vec::new(),
            },
        ),
        (
            0.0,
            Event::TurnStarted {
                turn_id: turn_id(),
                cwd: "/home/u".into(),
                scope: Scope::Machine,
                settings: Some(EffectiveSettings {
                    mode: Mode::Auto,
                    model: "claude-opus-5-5".to_owned(),
                    effort: Some("high".to_owned()),
                    provider: Some("anthropic-api.work".to_owned()),
                    overridden: OverriddenSettings::default(),
                    fallback: None,
                    default_effort: None,
                }),
            },
        ),
    ];
    events.extend(steps);
    let report = Event::TurnWebReport {
        turn_id: turn_id(),
        searches: 5,
        stored_copy: false,
        stored_copy_note: false,
        sources,
    };
    events.push((108.0, report));
    events.push((
        108.0,
        Event::TurnCompleted {
            turn_id: turn_id(),
            usage: Some(usage),
            changes: None,
            context: Some(ContextUse { tokens: 34_000, limit: 100_000, window: 272_000 }),
            stop: None,
            undo: None,
        },
    ));
    let events: Vec<EventEnvelope> = events
        .into_iter()
        .zip(1..)
        .map(|((seconds, event), seq)| EventEnvelope { at: at(seconds), ..envelope(seq, event) })
        .collect();
    ConversationHistoryResult { events, next_cursor: None }
}

/// The transcript of `page` at the depth and width of a frame.
fn shown(page: &ConversationHistoryResult, depth: Depth, verbose: bool) -> Vec<String> {
    let options = design_options(depth, 100);
    let head = Head {
        title: Some("ROB RUN quits under Proton".to_owned()),
        place: "\u{2302} home".to_owned(),
        turns: 2,
    };
    let shown = Shown {
        options: &options,
        verbose,
        home: None,
        groups: &DiffGroups::default(),
        zone: &TimeZone::UTC,
        head: Some(&head),
    };
    let id = "019a7c02-3d00-7a10-8b20-000000000001".parse().unwrap();
    transcript(id, page, &shown).lines().map(|row| format!("{row}\n")).collect()
}

/// A search from `from` that ran `secs` and found `count` results.
fn searched(n: u8, query: &str, from: f64, secs: f64, count: usize) -> Vec<(f64, Event)> {
    let started = step(n, WebAction::Search, Some(query), None, None);
    let end = ended(&started, Some(results(count)), None);
    vec![(from, started), (from + secs, end)]
}

/// TM-Web 14: the rows of the searches as the turn wrote them, the failed one with its
/// reason, and the count on the end line. NOTE: rows 3 to 5 and 12 of the frame are the
/// prompt, the model line, a fold of reads and an edit of `main` (`efr history` shows
/// each read as a row of its own there, DECISIONS-web 4b); this checks the web rows and
/// the end line.
#[test]
fn the_rows_of_a_turn_that_searched_are_the_web_board() {
    let mut steps = Vec::new();
    steps.extend(searched(1, "\"ROB RUN\" Proton Easy Anti-Cheat", 1.0, 5.0, 10));
    steps.extend(searched(2, "\"ROB RUN\" areweanticheatyet", 7.0, 4.0, 8));
    steps.extend(searched(3, "ROB RUN protondb", 12.0, 6.0, 10));
    steps.extend(searched(4, "\"ROB RUN\" Steam Deck verified", 19.0, 5.0, 9));
    steps.extend(searched(5, "Proton EasyAntiCheat Runtime install 2026", 25.0, 7.0, 10));
    let limited = step(6, WebAction::Search, Some("\"ROB RUN\" 1.4 patch notes Linux"), None, None);
    steps.push((33.0, limited.clone()));
    steps.push((34.0, ended(&limited, None, Some(WebError::TooManyRequests))));
    let page = page(steps, Vec::new());
    let frame = Frame::load("TM-Web", 14).unwrap();
    for depth in Depth::ALL {
        let rows = shown(&page, depth, false);
        assert_rows(&frame, 6..=11, &rows[4..10].concat(), depth);
        assert_rows(&frame, 13..=13, &rows[10], depth);
    }
}

/// TM-Web 15: `--verbose` lists the first three results under a search, then how many
/// more and the sources of the turn among them; under an opened page, the finds in it.
#[test]
fn verbose_shows_the_results_and_the_finds_as_the_web_board() {
    let result =
        |title: &str, url: &str| WebResult { url: url.to_owned(), title: Some(title.to_owned()) };
    let rob_run = "https://areweanticheatyet.com/game/rob-run";
    let mut found = vec![
        result("ROB RUN \u{b7} Are We Anti-Cheat Yet?", rob_run),
        result("Easy Anti-Cheat games on Linux", "https://areweanticheatyet.com/"),
        result("ROB RUN 1.4 patch notes", "https://store.steampowered.com/news/app/2384710"),
    ];
    found.extend(results(5));
    let search = step(1, WebAction::Search, Some("\"ROB RUN\" areweanticheatyet"), None, None);
    let open = step(2, WebAction::Open, None, Some(rob_run), None);
    let find = |n: u8, pattern: &str| step(n, WebAction::Find, None, Some(rob_run), Some(pattern));
    let (first, second) = (find(3, "Easy Anti-Cheat"), find(4, "Proton"));
    let steps = vec![
        (1.0, search.clone()),
        (5.0, ended(&search, Some(found), None)),
        (6.0, open.clone()),
        (8.0, ended(&open, None, None)),
        (8.0, first.clone()),
        (9.0, ended(&first, None, None)),
        (9.0, second.clone()),
        (10.0, ended(&second, None, None)),
    ];
    let sources = vec![WebSource { url: rob_run.to_owned(), title: None }];
    let page = page(steps, sources);
    let frame = Frame::load("TM-Web", 15).unwrap();
    for depth in Depth::ALL {
        let rows = shown(&page, depth, true);
        assert_rows(&frame, 0..=6, &rows[4..11].concat(), depth);
    }
    // Without `--verbose` the finds and the results stay out, and the list of sources
    // follows the rows after one blank row.
    let rows = shown(&page, Depth::NoColor, false);
    let text: Vec<String> = rows.iter().map(|row| bare(row)).collect();
    assert_eq!(
        text[4..9],
        [
            "  \u{2713} web \"ROB RUN\" areweanticheatyet  8 results \u{b7} 4s\n",
            "  \u{2713} web open areweanticheatyet.com/game/rob-run  2s\n",
            "\n",
            "  [1] https://areweanticheatyet.com/game/rob-run\n",
            "\n",
        ]
    );
}
