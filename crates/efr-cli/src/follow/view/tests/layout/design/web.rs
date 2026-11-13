//! Web search through a real view against the frames of TM-Web: the events of a turn
//! that searched, the screen that a terminal keeps, compared cell by cell in each depth
//! that the board draws or that `efr_test_support::design::expect` derives.

use std::path::PathBuf;

use efr_protocol::{
    ContextUse, EffectiveSettings, Event, Mode, OverriddenSettings, Scope, Usage, WebAction,
    WebError, WebResult, WebSource,
};
use efr_test_support::design::{Depth, Frame};

use super::super::super::super::{Look, TurnView};
use super::super::{Screen, id, message, shell};
use super::{Play, all, check, ended, rows, spun};
use crate::terminal::Size;
use crate::testing::{bare, design_options, turn};

/// The look of the frames: no motion, and the line at the end of a turn.
const LOOK: Look = Look {
    motion: false,
    summary: true,
    progress: false,
    diff_lines: 20,
    diff_lines_long: 8,
    bell: false,
};

/// A view of a turn on `work/claude-opus-5-5 high` in the home directory, with the
/// scope in the status row (no prompt corner), as the frames of TM-Web draw it.
fn play(depth: Depth, cols: u16) -> Play {
    let mut view = TurnView::new(turn(), design_options(depth, cols))
        .with_look(LOOK)
        .with_home(Some(PathBuf::from("/home/user")))
        .with_scope(Some("\u{2302} ~".to_owned()));
    view.start();
    let size = Size { cols, rows: 60 };
    let mut play = Play { view, screen: Screen::new(cols), size, seq: 10, now: 0 };
    let settings = EffectiveSettings {
        mode: Mode::Auto,
        model: "claude-opus-5-5".to_owned(),
        effort: Some("high".to_owned()),
        provider: Some("anthropic-api.work".to_owned()),
        overridden: OverriddenSettings::default(),
        fallback: None,
        default_effort: None,
    };
    let started = Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/user"),
        scope: Scope::Machine,
        settings: Some(settings),
    };
    play.event(0, started);
    play
}

/// Step `n` of the web tool starts: a search of `query`, or with `action` an open or a
/// find of the page `url`.
fn web(n: u8, action: WebAction, query: Option<&str>, url: Option<&str>) -> Event {
    Event::WebCallStarted {
        turn_id: turn(),
        call_id: id(n),
        action,
        query: query.map(str::to_owned),
        url: url.map(str::to_owned),
        pattern: (action == WebAction::Find).then(|| "Easy Anti-Cheat".to_owned()),
    }
}

fn search(n: u8, query: &str) -> Event {
    web(n, WebAction::Search, Some(query), None)
}

/// The end of `started` with `results` results when the provider counts them, or with
/// `error`.
fn web_end(started: &Event, results: Option<usize>, error: Option<WebError>) -> Event {
    let Event::WebCallStarted { call_id, action, query, url, pattern, .. } = started.clone() else {
        panic!("not the start of a web step: {started:?}");
    };
    let result = |n: usize| WebResult { url: format!("https://example.com/{n}"), title: None };
    Event::WebCallCompleted {
        turn_id: turn(),
        call_id,
        action,
        query,
        url,
        pattern,
        results: results.map(|count| (0..count).map(result).collect()),
        error,
    }
}

/// A step from `from` ms that went well `secs` later, with `results`.
fn searched(play: &mut Play, started: Event, from: i64, secs: i64, results: Option<usize>) {
    play.event(from, started.clone());
    play.event(from + secs * 1_000, web_end(&started, results, None));
}

fn sources(count: usize) -> Vec<WebSource> {
    let source =
        |url: &str, title: &str| WebSource { url: url.to_owned(), title: Some(title.to_owned()) };
    let all = vec![
        source(
            "https://areweanticheatyet.com/game/rob-run",
            "ROB RUN \u{b7} Are We Anti-Cheat Yet?",
        ),
        source("https://www.protondb.com/app/2384710", "ROB RUN \u{b7} ProtonDB"),
        source("https://www.steamdeck.com/verified/2384710", "Steam Deck Verified: ROB RUN"),
    ];
    all.into_iter().take(count).collect()
}

fn report(searches: u32, stored_copy: bool, note: bool, sources: Vec<WebSource>) -> Event {
    Event::TurnWebReport { turn_id: turn(), searches, stored_copy, stored_copy_note: note, sources }
}

/// The end of the turn: `usage` and the context at `percent`.
fn completed(usage: Usage, percent: u64) -> Event {
    Event::TurnCompleted {
        turn_id: turn(),
        usage: Some(usage),
        changes: None,
        context: Some(ContextUse { tokens: percent * 1_000, limit: 100_000, window: 272_000 }),
        stop: None,
        undo: None,
    }
}

fn usage(input: u64, cached: u64, output: u64) -> Usage {
    Usage { cached_input_tokens: cached, ..Usage::new(input, output) }
}

/// The answer of TM-Web 10 with the marks as efrd writes them into its text.
const ANSWER: &str = "The developers allowed Linux in Easy Anti-Cheat with patch 1.4 \
[[1]](<https://areweanticheatyet.com/game/rob-run>). ProtonDB reports that it runs with \
Proton 10 and the Proton EasyAntiCheat Runtime installed [[2]](<https://www.protondb.com/app/2384710>), \
and it is Verified on Steam Deck [[3]](<https://www.steamdeck.com/verified/2384710>). Your setup \
forces Proton 9 for it, and the runtime is not installed.\n";

/// TM-Web 3: five searches, one row each in the order of the model, never folded.
#[test]
fn five_searches_are_five_rows_web_3() {
    let frame = Frame::load("TM-Web", 3).unwrap();
    for depth in Depth::ALL {
        let mut play = play(depth, frame.cols);
        searched(&mut play, search(1, "\"ROB RUN\" Proton Easy Anti-Cheat"), 1_000, 5, Some(10));
        searched(&mut play, search(2, "\"ROB RUN\" areweanticheatyet"), 7_000, 4, Some(8));
        searched(&mut play, search(3, "ROB RUN protondb"), 12_000, 6, Some(10));
        searched(&mut play, search(4, "\"ROB RUN\" Steam Deck verified"), 19_000, 5, Some(9));
        searched(
            &mut play,
            search(5, "Proton EasyAntiCheat Runtime install 2026"),
            25_000,
            7,
            Some(10),
        );
        let shown = play.committed();
        check(&frame, .., &rows(&shown, 0..5), depth, all);
    }
}

/// The status row of a frame, with its spinner as the board draws it. NOTE: without
/// motion the state shows no band; the board draws it over `arc`, so those cells
/// compare by their text (the status row's own tests check the band).
fn status_cells(status_row: usize) -> impl Fn(usize, usize) -> bool {
    move |row, column| row == status_row && (2..11).contains(&column)
}

/// TM-Web 4: three searches at once. The status row is the only live row, right under
/// the rows; it counts the time from the first of the three, names the newest and `+2`.
/// Each row commits when it ends, in the order of the starts: the fourth search ends
/// before the third and waits for it.
#[test]
fn three_searches_at_once_commit_in_the_order_of_their_starts_web_4() {
    let frame = Frame::load("TM-Web", 4).unwrap();
    for depth in Depth::ALL {
        let mut play = play(depth, frame.cols);
        play.context(22);
        play.event(10, shell(1, "rg -n \"EasyAntiCheat|EOS_\" ~/.steam/steam/logs/compat_log.txt"));
        play.event(110, ended(1, Some(0), ""));
        searched(&mut play, search(2, "\"ROB RUN\" Proton Easy Anti-Cheat"), 1_000, 5, Some(10));
        searched(&mut play, search(3, "\"ROB RUN\" areweanticheatyet"), 6_000, 4, Some(8));
        let third = search(4, "ROB RUN protondb");
        let fourth = search(5, "\"ROB RUN\" Steam Deck verified");
        let fifth = search(6, "Proton EasyAntiCheat Runtime install 2026");
        play.event(11_000, third.clone());
        play.event(12_000, fourth.clone());
        play.event(12_000, fifth.clone());
        play.now = 13_000;
        play.frame();
        let shown = spun(&play.screen.raw(), "\u{283c}");
        check(&frame, 0..=3, &rows(&shown, 0..4), depth, status_cells(3));
        // The fourth ends first and waits for the third.
        play.event(17_000, web_end(&fourth, Some(9), None));
        assert!(!bare(&play.screen.raw()).contains("Steam Deck verified  9"));
        play.event(17_000, web_end(&third, Some(10), None));
        let shown = spun(&play.screen.raw(), "\u{283c}");
        check(&frame, 5..=9, &rows(&shown, 1..6), depth, status_cells(4));
        play.event(19_000, web_end(&fifth, Some(10), None));
        let shown = play.committed();
        check(&frame, 11..=15, &rows(&shown, 1..6), depth, all);
    }
}

/// TM-Web 5: a search, an open and a find on OpenAI. The open names its page in the
/// link role; the find has no row. NOTE: the board keeps an empty row where the find
/// stands; the rows around it are checked apart.
#[test]
fn a_search_an_open_and_a_find_web_5() {
    let frame = Frame::load("TM-Web", 5).unwrap();
    let page = "https://areweanticheatyet.com/game/rob-run";
    for depth in Depth::ALL {
        let mut play = play(depth, frame.cols);
        searched(&mut play, search(1, "\"ROB RUN\" areweanticheatyet"), 1_000, 5, None);
        searched(&mut play, web(2, WebAction::Open, None, Some(page)), 6_000, 2, None);
        searched(&mut play, web(3, WebAction::Find, None, Some(page)), 8_000, 1, None);
        let protondb = "https://www.protondb.com/app/2384710";
        searched(&mut play, web(4, WebAction::Open, None, Some(protondb)), 9_000, 3, None);
        searched(&mut play, search(5, "Proton EasyAntiCheat Runtime install"), 12_000, 4, None);
        let shown = play.committed();
        check(&frame, 0..=1, &rows(&shown, 0..2), depth, all);
        check(&frame, 3..=4, &rows(&shown, 2..4), depth, all);
    }
}

/// A turn of TM-Web 16: three searches (one failed), an open, the answer with its marks,
/// `count` sources after it and the end line with the count of the searches that ran.
fn cited_turn(depth: Depth, cols: u16, count: usize) -> String {
    let mut play = play(depth, cols);
    searched(&mut play, search(1, "\"ROB RUN\" Proton Easy Anti-Cheat"), 1_000, 5, Some(10));
    searched(&mut play, search(2, "\"ROB RUN\" areweanticheatyet"), 6_000, 4, Some(8));
    let limited = search(3, "\"ROB RUN\" 1.4 patch notes Linux");
    play.event(10_000, limited.clone());
    play.event(11_000, web_end(&limited, None, Some(WebError::TooManyRequests)));
    let page = "https://areweanticheatyet.com/game/rob-run";
    searched(&mut play, web(4, WebAction::Open, None, Some(page)), 12_000, 2, None);
    play.event(100_000, message(1, ANSWER));
    play.event(108_000, report(5, false, false, sources(count)));
    play.event(108_000, completed(usage(30_000, 26_700, 3_100), 34));
    play.committed()
}

/// TM-Web 10 and 11 (`NO_COLOR`), 16 and 17: the rows of the steps, the answer with its
/// marks, one blank row, the sources, one blank row, and `· 5 searches` last on the end
/// line. A failed search and the open do not count.
#[test]
fn the_answer_its_sources_and_the_count_web_10_16_17() {
    let frame = Frame::load("TM-Web", 10).unwrap();
    for depth in Depth::ALL {
        let shown = cited_turn(depth, 100, 3);
        check(&frame, 0..=8, &rows(&shown, 5..14), depth, all);
    }
    for (number, depth) in [(16, Depth::Ansi16), (17, Depth::NoColor)] {
        let shown = cited_turn(depth, 100, 2);
        check(&Frame::load("TM-Web", number).unwrap(), 0..=12, &rows(&shown, 0..13), depth, all);
    }
}

/// TM-Web 13, rows 3 and 4: on OpenAI in cautious the end line says `stored copy`, and
/// the first such turn of a conversation gets the note under it; a later one does not.
#[test]
fn the_stored_copy_and_its_note_web_13() {
    let frame = Frame::load("TM-Web", 13).unwrap();
    for depth in Depth::ALL {
        for note in [true, false] {
            let mut play = play(depth, frame.cols);
            searched(&mut play, search(1, "rust 1.95 release notes"), 1_000, 5, None);
            play.event(20_000, message(1, "Done.\n"));
            play.event(31_000, report(5, true, note, Vec::new()));
            play.event(31_000, completed(usage(1_500, 0, 2_400), 22));
            let shown = play.committed();
            let end = shown.split_inclusive('\n').count();
            if note {
                check(&frame, 3..=4, &rows(&shown, end - 2..end), depth, all);
            } else {
                check(&frame, 3..=3, &rows(&shown, end - 1..end), depth, all);
            }
        }
    }
}

/// TM-Web 9: in manual mode efr adds nothing of its own; the model's sentence says why.
/// Rows 1 to 4 (row 0 and 5 are the user's prompt line, not efr's output). NOTE: in the
/// board the code span `,mode cautious` has no backticks; without colour `main` keeps
/// them (TM-Markdown 2), a point that is not web search, so this checks the depths with
/// colour.
#[test]
fn a_manual_turn_writes_nothing_about_web_search_web_9() {
    let frame = Frame::load("TM-Web", 9).unwrap();
    let answer = "Web search is off in manual mode, so I cannot check what changed lately. \
`,mode cautious` turns it on for this terminal. What I knew at my cutoff: the game shipped Easy \
Anti-Cheat without Linux support.\n";
    for depth in [Depth::TrueColor, Depth::Ansi16] {
        let mut play = play(depth, frame.cols);
        play.event(2_000, message(0, answer));
        play.event(3_000, completed(usage(1_200, 0, 96), 4));
        let shown = play.committed();
        check(&frame, 1..=4, &shown, depth, all);
        assert!(!bare(&shown).contains("web "), "{shown}");
    }
}

/// Without a terminal the rows go to stderr, plain and in the order of the starts, the
/// answer alone to stdout, and the list of sources to stderr after it.
#[test]
fn through_a_pipe_the_rows_go_to_stderr_in_order() {
    let options = efr_render::RenderOptions::new(80).with_terminal(false);
    let mut view = TurnView::new(turn(), options).with_look(LOOK);
    let size = Size { cols: 80, rows: 24 };
    let (mut out, mut err) = (String::new(), String::new());
    let first = search(1, "a b");
    let second = search(2, "c d");
    let events = [
        first.clone(),
        second.clone(),
        web_end(&second, Some(2), None),
        web_end(&first, Some(1), None),
        message(0, "It is [[1]](<https://a.org/>).\n"),
        report(2, false, false, sources(1)),
        completed(usage(100, 0, 10), 1),
    ];
    for event in events {
        let step = view.event(&event, size, false);
        out.push_str(&step.out);
        err.push_str(&step.err);
    }
    assert_eq!(out, "It is [[1]](<https://a.org/>).\n");
    assert_eq!(
        err,
        "\u{2713} web a b  1 result\n\u{2713} web c d  2 results\n\n[1] https://areweanticheatyet.com/game/rob-run  ROB RUN \u{b7} Are We Anti-Cheat Yet?\n"
    );
}
