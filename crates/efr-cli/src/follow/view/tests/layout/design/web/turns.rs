//! Whole turns of web search against the frames of TM-Web: a stream of the providers'
//! fixtures goes through the real provider, the real turn of a test daemon and the
//! protocol, and the turn's events go through a real view (and `efr history`). This
//! joins the four slices of the build: what the provider maps, the turn records and
//! the client draws must meet.
//!
//! Only the clock is the test's ([`Timing`]): the fake server sends a stream at once.
//! The daemon's `turn_started` names the provider `anthropic-api`; the view gets the
//! one of the board (`work/claude-opus-5-5 high`), a point that is not web search.

use std::path::Path;

use efr_config::{DiffGroups, WebSearch};
use efr_protocol::{
    ContextUse, ConversationHistoryResult, Draft, DraftPart, Event, EventEnvelope, Mode, Seq,
    TurnId,
};
use efr_test_support::design::{Depth, Frame};
use jiff::tz::TimeZone;
use serde_json::Value;

use super::super::Play;
use super::{play_of, rows, status_cells};
use crate::commands::history::{Head, Shown, transcript};
use crate::testing::bare;
use daemon::{Company, Ran, Timing, Wire, run};

use super::super::check;

/// Every cell by its colour and attributes.
fn all(_: usize, _: usize) -> bool {
    false
}

/// The timed events of `ran`, without the daemon's `turn_started` ([`play_of`] sent the
/// board's).
fn timed(ran: &Ran, timing: &Timing) -> Vec<(i64, Event)> {
    timing
        .of(ran)
        .into_iter()
        .filter(|(_, event)| !matches!(event, Event::TurnStarted { .. }))
        .collect()
}

/// Plays `events` on a view of `cols` columns in `depth` and gives the rows that the
/// scrollback keeps.
fn played(ran: &Ran, events: &[(i64, Event)], depth: Depth, cols: u16) -> String {
    let mut play = play_of(ran.turn, depth, cols);
    for (at, event) in events {
        play.event(*at, event.clone());
    }
    play.committed()
}

/// The daemon sends a draft of the model's context of turn `turn_id`: `percent` of its
/// limit, as the status row of the board shows it.
fn context(play: &mut Play, turn_id: TurnId, percent: u64) {
    let part = DraftPart::Context(ContextUse { tokens: percent, limit: 100, window: 100 });
    let draft = Draft { turn_id, after_seq: Seq::new(play.seq), draft: part };
    play.view.draft(&draft, play.size);
    play.frame();
}

/// The index of the first row of `shown` whose text starts with `start`.
fn row_of(shown: &str, start: &str) -> usize {
    shown
        .split_inclusive('\n')
        .position(|row| bare(row).trim_start().starts_with(start))
        .unwrap_or_else(|| panic!("no row starts with {start:?}:\n{}", bare(shown)))
}

/// The tools of each request that `ran`'s server got, as JSON text.
fn tools(ran: &Ran) -> Vec<String> {
    ran.bodies.iter().map(|body| body["tools"].to_string()).collect()
}

/// A Claude answer of plain text with its usage: `input` tokens in all, `cached` of
/// them read from the cache, and `output`.
fn claude_text(text: &str, input: u64, cached: u64, output: u64) -> String {
    let uncached = input - cached;
    let usage = serde_json::json!({
        "input_tokens": uncached,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": cached,
        "output_tokens": output,
    });
    let events: Vec<(&str, Value)> = vec![
        (
            "message_start",
            serde_json::json!({"type": "message_start", "message": {
                "id": "msg_01plain", "type": "message", "role": "assistant",
                "model": "claude-opus-5-5", "content": [], "stop_reason": null,
                "stop_sequence": null, "usage": usage}}),
        ),
        (
            "content_block_start",
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": text}}),
        ),
        ("content_block_stop", serde_json::json!({"type": "content_block_stop", "index": 0})),
        (
            "message_delta",
            serde_json::json!({"type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": usage}),
        ),
        ("message_stop", serde_json::json!({"type": "message_stop"})),
    ];
    events.iter().map(|(name, data)| format!("event: {name}\ndata: {data}\n\n")).collect()
}

/// An OpenAI answer of plain text.
fn openai_text(text: &str) -> String {
    let item = serde_json::json!({"id": "msg_plain", "type": "message", "status": "completed",
        "role": "assistant", "content": [{"type": "output_text", "text": text, "annotations": []}]});
    let events: Vec<(&str, Value)> = vec![
        (
            "response.created",
            serde_json::json!({"type": "response.created", "sequence_number": 0,
                "response": {"id": "resp_plain", "object": "response", "status": "in_progress",
                "model": "gpt-5.5", "output": []}}),
        ),
        (
            "response.output_item.done",
            serde_json::json!({"type": "response.output_item.done", "sequence_number": 1,
                "output_index": 0, "item": item}),
        ),
        (
            "response.completed",
            serde_json::json!({"type": "response.completed", "sequence_number": 2,
                "response": {"id": "resp_plain", "object": "response", "status": "completed",
                "model": "gpt-5.5", "output": [item], "usage": {"input_tokens": 1200,
                "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 96,
                "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 1296}}}),
        ),
    ];
    events.iter().map(|(name, data)| format!("event: {name}\ndata: {data}\n\n")).collect()
}

/// The times of `web_search_board.sse` as TM-Web 3, 4 and 10 draw them: two searches one
/// after the other, then three at once, of which the fourth ends with the third; the
/// answer, and the end after 1m 48s.
fn board_timing() -> Timing {
    Timing {
        steps: vec![
            (1_000, 5_000),
            (6_000, 4_000),
            (11_000, 6_000),
            (12_000, 5_000),
            (12_000, 7_000),
        ],
        answer: 100_000,
        end: 108_000,
    }
}

/// TM-Web 3, 4, 10 and 13 (row 1) from one Claude turn: the fixture stream of five
/// searches, the last three at once with their results in another order, then an answer
/// with three citations. The status row is the only live row while they run; the rows
/// commit in the order of the starts; the marks stand before the punctuation, the
/// sources follow the answer, and the end line counts the 5 searches.
#[tokio::test]
async fn a_claude_turn_of_five_searches_is_the_board_web_3_4_10_13() {
    let ran = run(Wire::fixtures(Company::Anthropic, 1_000_000, &["web_search_board.sse"])).await;
    let events = timed(&ran, &board_timing());
    let fifth = events
        .iter()
        .rposition(|(_, event)| matches!(event, Event::WebCallStarted { .. }))
        .unwrap();
    let third_ended = events
        .iter()
        .position(|(_, event)| {
            matches!(event, Event::WebCallCompleted { query: Some(query), .. }
                if query == "ROB RUN protondb")
        })
        .unwrap();
    let (four, three, ten, thirteen) = (
        Frame::load("TM-Web", 4).unwrap(),
        Frame::load("TM-Web", 3).unwrap(),
        Frame::load("TM-Web", 10).unwrap(),
        Frame::load("TM-Web", 13).unwrap(),
    );
    for depth in Depth::ALL {
        let mut play = play_of(ran.turn, depth, 100);
        context(&mut play, ran.turn, 22);
        for (at, event) in &events[..=fifth] {
            play.event(*at, event.clone());
        }
        play.now = 13_000;
        play.frame();
        // NOTE: row 0 of TM-Web 4 is a shell row of `main`, which this turn has not.
        let shown = super::super::spun(&play.screen.raw(), "\u{283c}");
        check(&four, 1..=3, &rows(&shown, 0..3), depth, status_cells(2));
        for (at, event) in &events[fifth + 1..=third_ended] {
            play.event(*at, event.clone());
        }
        let shown = super::super::spun(&play.screen.raw(), "\u{283c}");
        check(&four, 5..=9, &rows(&shown, 0..5), depth, status_cells(4));
        for (at, event) in &events[third_ended + 1..] {
            play.event(*at, event.clone());
        }
        let shown = play.committed();
        check(&four, 11..=15, &rows(&shown, 0..5), depth, all);
        check(&three, 0..=4, &rows(&shown, 0..5), depth, all);
        // One blank row stands between the rows and the answer (TM-Web 16).
        assert_eq!(bare(&rows(&shown, 5..6)), "\n");
        check(&ten, 0..=8, &rows(&shown, 6..15), depth, all);
        check(&thirteen, 1..=1, &rows(&shown, 14..15), depth, all);
    }
    // TM-Web 12 and 18, 19 at 60 columns: the titles drop before a URL is cut, and the
    // count stays last on the end line. NOTE: the prose of TM-Web 12 leaves out the last
    // sentence of the answer, and row 1 of TM-Web 18 and 19 cuts a query that fits
    // (`install…` fits in the 37 cells of DECISIONS-web 3); those rows are not checked.
    // The end line of TM-Web 12 leaves out `3.1k out · cache 89%`, which fit in 60
    // columns; the parts of the cost are `main`'s (TM-Endings, DECISIONS-web 4b), so
    // only the count is checked there.
    let twelve = Frame::load("TM-Web", 12).unwrap();
    for depth in Depth::ALL {
        let shown = played(&ran, &events, depth, 60);
        let sources = row_of(&shown, "[1]");
        check(&twelve, 4..=8, &rows(&shown, sources - 1..sources + 4), depth, all);
        let done = bare(&rows(&shown, sources + 4..sources + 5));
        assert!(
            done.starts_with("\u{2713} done  1m 48s") && done.ends_with(" \u{b7} 5 searches\n")
        );
    }
    for (number, depth) in [(18, Depth::Ansi16), (19, Depth::NoColor)] {
        let frame = Frame::load("TM-Web", number).unwrap();
        let shown = played(&ran, &events, depth, 60);
        check(&frame, 0..=0, &rows(&shown, 0..1), depth, all);
        let sources = row_of(&shown, "[1]");
        check(&frame, 6..=8, &rows(&shown, sources - 1..sources + 2), depth, all);
    }
    // The request carried the live web of a Claude model.
    assert!(tools(&ran)[0].contains("\"type\":\"web_search_20250305\""), "{:?}", tools(&ran));
}

/// TM-Web 14 and 15 from the same Claude turn, read back as `efr history` reads it: the
/// rows of the searches, the end line with the count, and under `--verbose` the first
/// results of a search and the sources of the turn among them. NOTE: rows 3 to 5, 11 and
/// 12 of TM-Web 14 are the prompt, the model line, a fold of reads, a failed search and
/// an edit that this turn has not; this checks the web rows and the end line.
#[tokio::test]
async fn efr_history_of_the_claude_turn_is_the_board_web_14_15() {
    let ran = run(Wire::fixtures(Company::Anthropic, 1_000_000, &["web_search_board.sse"])).await;
    let start: jiff::Timestamp = "2026-11-12T21:14:00Z".parse().unwrap();
    let events: Vec<EventEnvelope> = board_timing()
        .of(&ran)
        .into_iter()
        .zip(1..)
        .map(|((millis, event), seq)| EventEnvelope {
            seq: Seq::new(seq),
            conversation_id: None,
            at: start + jiff::SignedDuration::from_millis(millis),
            event,
        })
        .collect();
    let page = ConversationHistoryResult { events, next_cursor: None };
    let head = Head {
        title: Some("ROB RUN quits under Proton".to_owned()),
        place: "\u{2302} home".to_owned(),
        turns: 2,
    };
    let (fourteen, fifteen) =
        (Frame::load("TM-Web", 14).unwrap(), Frame::load("TM-Web", 15).unwrap());
    for depth in Depth::ALL {
        for verbose in [false, true] {
            let options = crate::testing::design_options(depth, 100);
            let shown = Shown {
                options: &options,
                verbose,
                home: Some(Path::new("/home/user")),
                groups: &DiffGroups::default(),
                zone: &TimeZone::UTC,
                head: Some(&head),
            };
            let id = "019a7c02-3d00-7a10-8b20-000000000001".parse().unwrap();
            let text = transcript(id, &page, &shown);
            let first = row_of(&text, "\u{2713} web");
            if verbose {
                // The second search with its first three results and `… 5 more · cited: 1`.
                let second = row_of(&text, "\u{2713} web \"ROB RUN\" areweanticheatyet");
                check(&fifteen, 0..=4, &rows(&text, second..second + 5), depth, all);
            } else {
                check(&fourteen, 6..=10, &rows(&text, first..first + 5), depth, all);
                let done = row_of(&text, "\u{2713} done");
                check(&fourteen, 13..=13, &rows(&text, done..done + 1), depth, all);
            }
        }
    }
}

/// TM-Web 1 and 2, the rows of a search at 100 and 60 columns, from three turns: a
/// Claude turn of a search that fits and one whose query is cut (rows 0 and 3), a
/// Claude turn of a search with no results (row 2) and an OpenAI turn, whose searches
/// have no count (row 1). NOTE: rows 4 and 5 are a shell row and a read of `main`.
#[tokio::test]
async fn the_rows_of_a_search_of_each_provider_are_the_board_web_1_2() {
    let long =
        run(Wire::fixtures(Company::Anthropic, 1_000_000, &["web_search_long_query.sse"])).await;
    let empty = run(Wire::fixtures(Company::Anthropic, 1_000_000, &["web_search_empty.sse"])).await;
    let openai = run(Wire::fixtures(Company::OpenAi, 400_000, &["web_search.sse"])).await;
    let long_events = timed(
        &long,
        &Timing { steps: vec![(1_000, 6_000), (8_000, 6_000)], answer: 15_000, end: 16_000 },
    );
    let empty_events =
        timed(&empty, &Timing { steps: vec![(1_000, 3_000)], answer: 5_000, end: 6_000 });
    let openai_events = timed(
        &openai,
        &Timing {
            steps: vec![(1_000, 5_000), (6_000, 2_000), (8_000, 1_000)],
            answer: 10_000,
            end: 11_000,
        },
    );
    for (number, cols) in [(1, 100), (2, 60)] {
        let frame = Frame::load("TM-Web", number).unwrap();
        for depth in Depth::ALL {
            let shown = played(&long, &long_events, depth, cols);
            check(&frame, 0..=0, &rows(&shown, 0..1), depth, all);
            check(&frame, 3..=3, &rows(&shown, 1..2), depth, all);
            let shown = played(&empty, &empty_events, depth, cols);
            check(&frame, 2..=2, &rows(&shown, 0..1), depth, all);
            let shown = played(&openai, &openai_events, depth, cols);
            check(&frame, 1..=1, &rows(&shown, 0..1), depth, all);
        }
    }
}

/// TM-Web 5 from OpenAI turns: a search, an open, a find, an open and a search. Each
/// open is its own row with its page; the find has no row. The fixture of the slice
/// (`web_search.sse`: a search, an open, a find) gives rows 0 and 1, the board's stream
/// rows 0, 1, 3 and 4. NOTE: the board keeps an empty row where the find stands.
#[tokio::test]
async fn the_steps_of_an_openai_turn_are_the_board_web_5() {
    let frame = Frame::load("TM-Web", 5).unwrap();
    let slice = run(Wire::fixtures(Company::OpenAi, 400_000, &["web_search.sse"])).await;
    let board = run(Wire::fixtures(Company::OpenAi, 400_000, &["web_search_steps.sse"])).await;
    let slice_events = timed(
        &slice,
        &Timing {
            steps: vec![(1_000, 5_000), (6_000, 2_000), (8_000, 1_000)],
            answer: 10_000,
            end: 11_000,
        },
    );
    let board_events = timed(
        &board,
        &Timing {
            steps: vec![
                (1_000, 5_000),
                (6_000, 2_000),
                (8_000, 1_000),
                (9_000, 3_000),
                (12_000, 4_000),
            ],
            answer: 17_000,
            end: 18_000,
        },
    );
    // Auto mode: the live web, so the end line names no stored copy.
    assert!(tools(&board)[0].contains(r#"{"external_web_access":true,"type":"web_search"}"#));
    for depth in Depth::ALL {
        let shown = played(&slice, &slice_events, depth, frame.cols);
        check(&frame, 0..=1, &rows(&shown, 0..2), depth, all);
        assert!(!bare(&rows(&shown, 2..3)).starts_with("\u{2713} web"), "{}", bare(&shown));
        let shown = played(&board, &board_events, depth, frame.cols);
        check(&frame, 0..=1, &rows(&shown, 0..2), depth, all);
        check(&frame, 3..=4, &rows(&shown, 2..4), depth, all);
        assert!(!bare(&shown).contains("find"), "a find has no row: {}", bare(&shown));
        assert!(!bare(&shown).contains("stored copy"), "{}", bare(&shown));
    }
}

/// TM-Web 8 from the Claude fixture of three failed searches: one row each, `!` and the
/// reason in the warning role, no tail. NOTE: rows 1 and 2 of the board show no time;
/// these two fail at once, under the 0.1 s from which a row shows its time. Rows 4 and 5
/// are a failed command of `main`.
#[tokio::test]
async fn failed_searches_of_a_claude_turn_are_the_board_web_8() {
    let frame = Frame::load("TM-Web", 8).unwrap();
    let ran = run(Wire::fixtures(Company::Anthropic, 1_000_000, &["web_search_error.sse"])).await;
    let events = timed(
        &ran,
        &Timing {
            steps: vec![(1_000, 1_000), (2_000, 50), (2_100, 50)],
            answer: 3_000,
            end: 4_000,
        },
    );
    for depth in Depth::ALL {
        let shown = played(&ran, &events, depth, frame.cols);
        let first = row_of(&shown, "! web");
        check(&frame, 0..=2, &rows(&shown, first..first + 3), depth, all);
        if let Some(number) = match depth {
            Depth::Ansi16 => Some(18),
            Depth::NoColor => Some(19),
            _ => None,
        } {
            let narrow = Frame::load("TM-Web", number).unwrap();
            let shown = played(&ran, &events, depth, 60);
            let first = row_of(&shown, "! web");
            check(&narrow, 2..=2, &rows(&shown, first..first + 1), depth, all);
        }
        let done = bare(&shown);
        let done = done.lines().find(|row| row.starts_with("\u{2713} done")).unwrap();
        assert!(!done.contains("search"), "no search ran: {done}");
    }
}

/// TM-Web 13, rows 0, 2, 3 and 4 (row 1 is the board turn above; the fixture leaves out
/// the old line of the board): a Claude turn with no
/// search has no count, a Claude turn with one search says `1 search`, and an OpenAI turn
/// in cautious mode searched the stored copy: the request asked for it, the end line says
/// `stored copy` and the first such turn gets the note under it.
#[tokio::test]
async fn the_end_line_counts_the_searches_of_each_provider_web_13() {
    let frame = Frame::load("TM-Web", 13).unwrap();
    let plain = run(Wire {
        streams: vec![claude_text("Patch 1.4 allowed Linux.", 325_700, 296_400, 1_100)],
        ..Wire::fixtures(Company::Anthropic, 1_000_000, &[])
    })
    .await;
    let one = run(Wire::fixtures(Company::Anthropic, 30_000, &["web_search_short.sse"])).await;
    let stored =
        run(Wire::fixtures(Company::OpenAi, 23_000, &["web_search_stored.sse"])
            .mode(Mode::Cautious))
        .await;
    assert!(tools(&plain)[0].contains("web_search_20250305"), "the tool went, unused");
    assert!(tools(&stored)[0].contains(r#"{"external_web_access":false,"type":"web_search"}"#));
    let plain_events = timed(&plain, &Timing { steps: Vec::new(), answer: 0, end: 42_000 });
    let one_events =
        timed(&one, &Timing { steps: vec![(1_000, 5_000)], answer: 8_000, end: 12_000 });
    let five = (0..5).map(|n| (1_000 + n * 5_000, 4_000)).collect();
    let stored_events = timed(&stored, &Timing { steps: five, answer: 27_000, end: 31_000 });
    for depth in Depth::ALL {
        let shown = played(&plain, &plain_events, depth, frame.cols);
        let done = row_of(&shown, "\u{2713} done");
        check(&frame, 0..=0, &rows(&shown, done..done + 1), depth, all);
        let shown = played(&one, &one_events, depth, frame.cols);
        let done = row_of(&shown, "\u{2713} done");
        check(&frame, 2..=2, &rows(&shown, done..done + 1), depth, all);
        let shown = played(&stored, &stored_events, depth, frame.cols);
        let done = row_of(&shown, "\u{2713} done");
        check(&frame, 3..=4, &rows(&shown, done..done + 2), depth, all);
    }
}

/// The gate on both providers: in manual mode, and with `web.search = "off"` in any
/// mode, the model gets no web tool, and no request names one. TM-Web 9: in manual mode
/// efr adds nothing of its own; the model's sentence says why.
#[tokio::test]
async fn the_model_gets_no_web_tool_in_manual_mode_or_with_the_setting_off_web_9() {
    let answer = "Web search is off in manual mode.";
    for company in [Company::Anthropic, Company::OpenAi] {
        let stream = match company {
            Company::Anthropic => claude_text(answer, 1_200, 0, 96),
            Company::OpenAi => openai_text(answer),
        };
        let wire = Wire { streams: vec![stream], ..Wire::fixtures(company, 400_000, &[]) };
        for (mode, web) in [
            (Mode::Manual, WebSearch::Mode),
            (Mode::Manual, WebSearch::Live),
            (Mode::Cautious, WebSearch::Off),
            (Mode::Auto, WebSearch::Off),
            (Mode::Full, WebSearch::Off),
        ] {
            let ran = run(wire.clone().mode(mode).web(web)).await;
            assert_eq!(ran.bodies.len(), 1, "{company:?} {mode:?} {web:?}");
            let body = ran.bodies[0].to_string();
            assert!(!tools(&ran)[0].contains("web_"), "{company:?} {mode:?} {web:?}: {body}");
            assert!(!body.contains("web_search\""), "{company:?} {mode:?} {web:?}: {body}");
            let kinds: Vec<&str> =
                ran.events.iter().map(|envelope| envelope.event.kind()).collect();
            assert!(!kinds.iter().any(|kind| kind.contains("web")), "{kinds:?}");
            let events = timed(&ran, &Timing { steps: Vec::new(), answer: 0, end: 3_000 });
            let shown = bare(&played(&ran, &events, Depth::NoColor, 100));
            assert!(shown.contains(answer), "{shown}");
            let own = shown.replace(answer, "");
            assert!(!own.contains("web") && !own.contains("search"), "{shown}");
        }
    }
}

mod daemon;
