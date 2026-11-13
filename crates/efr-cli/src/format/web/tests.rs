//! The rows of web search against the frames of TM-Web: the text, the role and the
//! attributes of each cell, in the depths that the board draws and those that
//! `efr_test_support::design::expect` derives from a truecolor frame.

use std::time::Duration;

use efr_protocol::{CallId, WebAction, WebError, WebResult, WebSource};
use efr_render::Glyphs;
use efr_test_support::design::{Depth, Frame, assert_rows};
use pretty_assertions::assert_eq;

use super::{
    End, Ended, Fit, Order, Step, add_count, count, finds_row, page, reason, results_rows, row,
    sources, stored_copy_note,
};
use crate::format::Tone;
use crate::format::ending::{done_line, rows};
use crate::testing::design_options;

const G: &Glyphs = &Glyphs::UNICODE;

fn id(n: u8) -> CallId {
    format!("019a9b1c-3d00-7a10-8b20-0000000003{n:02x}").parse().unwrap()
}

fn search(query: &str) -> Step {
    Step { action: WebAction::Search, query: Some(query.to_owned()), url: None, pattern: None }
}

fn open(url: &str) -> Step {
    Step { action: WebAction::Open, query: None, url: Some(url.to_owned()), pattern: None }
}

fn find(pattern: &str, url: &str) -> Step {
    Step {
        action: WebAction::Find,
        query: None,
        url: Some(url.to_owned()),
        pattern: Some(pattern.to_owned()),
    }
}

/// A step that went well after `secs`, with `results` when the provider counts them.
fn done(step: Step, results: Option<usize>, secs: f64) -> Ended {
    Ended {
        call_id: id(1),
        step,
        end: End::Done { results },
        took: Some(Duration::from_secs_f64(secs)),
    }
}

fn failed(step: Step, error: WebError, secs: f64) -> Ended {
    Ended {
        call_id: id(1),
        step,
        end: End::Failed(error),
        took: Some(Duration::from_secs_f64(secs)),
    }
}

/// The rows of `ended`, as the turn writes them, at the depth and width of a frame.
fn written(ended: &[Ended], depth: Depth, cols: u16) -> String {
    let options = design_options(depth, cols);
    ended.iter().map(|ended| row(ended, Fit::Cut, &options)).collect()
}

/// Checks rows `range` of frame `number` against `ended`, in each of `depths`.
fn check(number: u32, range: std::ops::RangeInclusive<usize>, ended: &[Ended], depths: &[Depth]) {
    let frame = Frame::load("TM-Web", number).unwrap();
    for depth in depths {
        assert_rows(&frame, range.clone(), &written(ended, *depth, frame.cols), *depth);
    }
}

/// The searches of TM-Web 1 and 2: Claude with its count, OpenAI without one, no
/// results, and a long query.
fn one_search() -> Vec<Ended> {
    vec![
        done(search("\"ROB RUN\" game Linux Proton Easy Anti-Cheat"), Some(10), 6.0),
        done(search("\"ROB RUN\" areweanticheatyet"), None, 5.0),
        done(search("\"ROB RUN\" EAC kernel 6.17"), Some(0), 3.0),
        done(
            search(
                "\"ROB RUN\" EasyAntiCheat \"platform not allowed\" Proton 10 Arch Linux kernel 6.17",
            ),
            Some(10),
            6.0,
        ),
    ]
}

/// TM-Web 1 and 2, rows 1 to 4 (rows 5 and 6 are call rows of `main`): `web` and the
/// query muted, the count and the time after two spaces; a long query cut after the
/// last whole word that fits, at 100 and at 60 columns.
#[test]
fn the_row_of_a_search_is_the_web_board() {
    check(1, 0..=3, &one_search(), &Depth::ALL);
    check(2, 0..=3, &one_search(), &Depth::ALL);
}

/// TM-Web 3: five searches, one row each, never folded.
#[test]
fn five_searches_are_five_rows_of_the_web_board() {
    let five = [
        done(search("\"ROB RUN\" Proton Easy Anti-Cheat"), Some(10), 5.0),
        done(search("\"ROB RUN\" areweanticheatyet"), Some(8), 4.0),
        done(search("ROB RUN protondb"), Some(10), 6.0),
        done(search("\"ROB RUN\" Steam Deck verified"), Some(9), 5.0),
        done(search("Proton EasyAntiCheat Runtime install 2026"), Some(10), 7.0),
    ];
    check(3, 0..=4, &five, &Depth::ALL);
}

/// TM-Web 5: OpenAI's search, open, find. An open names its page in the link role; a
/// find has no row. NOTE: the board keeps an empty row where the find stands, to show
/// that it has none; the rows around it are checked apart.
#[test]
fn a_search_an_open_and_a_find_are_the_web_board() {
    let first = [
        done(search("\"ROB RUN\" areweanticheatyet"), None, 5.0),
        done(open("https://areweanticheatyet.com/game/rob-run"), None, 2.0),
        done(find("Easy Anti-Cheat", "https://areweanticheatyet.com/game/rob-run"), None, 1.0),
    ];
    check(5, 0..=1, &first, &Depth::ALL);
    let second = [
        done(open("https://www.protondb.com/app/2384710"), None, 3.0),
        done(search("Proton EasyAntiCheat Runtime install"), None, 4.0),
    ];
    check(5, 3..=4, &second, &Depth::ALL);
}

/// TM-Web 8, rows 1 to 3 (row 5 is the failed command of `main`): `!` and the reason
/// in `warning`, the rest muted, no tail. A limit and a missing tool fail at once, so
/// their rows show no time.
#[test]
fn a_failed_search_is_the_web_board() {
    let rows = [
        failed(search("\"ROB RUN\" 1.4 patch notes Linux"), WebError::TooManyRequests, 1.0),
        failed(search("\"ROB RUN\" kernel anti-cheat"), WebError::LimitReached, 0.0),
        failed(search("\"ROB RUN\" Proton"), WebError::Unavailable, 0.0),
    ];
    check(8, 0..=2, &rows, &Depth::ALL);
}

/// The key rows of TM-Web 16 to 19 in 16 colours and without colour: `!` bold without
/// colour, the page underlined, at 100 and at 60 columns.
#[test]
fn the_key_rows_in_sixteen_colours_and_without_colour_are_the_web_board() {
    let at_100 = [
        done(search("\"ROB RUN\" Proton Easy Anti-Cheat"), Some(10), 5.0),
        done(search("\"ROB RUN\" areweanticheatyet"), Some(8), 4.0),
        failed(search("\"ROB RUN\" 1.4 patch notes Linux"), WebError::TooManyRequests, 1.0),
        done(open("https://areweanticheatyet.com/game/rob-run"), None, 2.0),
    ];
    check(16, 0..=3, &at_100, &[Depth::Ansi16]);
    check(17, 0..=3, &at_100, &[Depth::NoColor]);
    // NOTE: frames 18 and 19 cut a query to `Proton EasyAntiCheat Runtime…`. With the
    // query of frame 3 (`... Runtime install 2026`), `install…` fits the 37 columns that
    // a row of 60 leaves (DECISIONS-web 3, the note of frame 2: "37 at 60"), so the
    // board's row is the cut of a longer query; this test gives it one.
    let at_60 = [
        done(search("\"ROB RUN\" Proton Easy Anti-Cheat"), Some(10), 5.0),
        done(search("Proton EasyAntiCheat Runtime installation 2026"), Some(10), 7.0),
        failed(search("\"ROB RUN\" 1.4 patch notes Linux"), WebError::TooManyRequests, 1.0),
    ];
    check(18, 0..=2, &at_60, &[Depth::Ansi16]);
    check(19, 0..=2, &at_60, &[Depth::NoColor]);
}

/// The sources of TM-Web 10.
fn three_sources() -> Vec<WebSource> {
    let source =
        |url: &str, title: &str| WebSource { url: url.to_owned(), title: Some(title.to_owned()) };
    vec![
        source(
            "https://areweanticheatyet.com/game/rob-run",
            "ROB RUN \u{b7} Are We Anti-Cheat Yet?",
        ),
        source("https://www.protondb.com/app/2384710", "ROB RUN \u{b7} ProtonDB"),
        source("https://www.steamdeck.com/verified/2384710", "Steam Deck Verified: ROB RUN"),
    ]
}

/// TM-Web 10 to 12, 16 to 19: the list of sources, `[n]` muted, the URL whole in the
/// link role, the title muted; at 60 columns a title that does not fit goes, and the
/// URL stays whole.
#[test]
fn the_list_of_sources_is_the_web_board() {
    let all = three_sources();
    let list = |depth: Depth, cols: u16, sources_: &[WebSource]| {
        sources(sources_, &design_options(depth, cols))
    };
    for depth in Depth::ALL {
        let frame = Frame::load("TM-Web", 10).unwrap();
        assert_rows(&frame, 4..=6, &list(depth, 100, &all), depth);
        let frame = Frame::load("TM-Web", 12).unwrap();
        assert_rows(&frame, 5..=7, &list(depth, 60, &all), depth);
    }
    for (number, depth, cols, range) in [
        (16, Depth::Ansi16, 100, 9..=10),
        (17, Depth::NoColor, 100, 9..=10),
        (18, Depth::Ansi16, 60, 7..=8),
        (19, Depth::NoColor, 60, 7..=8),
    ] {
        let frame = Frame::load("TM-Web", number).unwrap();
        assert_rows(&frame, range, &list(depth, cols, &all[..2]), depth);
    }
}

/// A title is cut while [`super::MIN_TITLE`] columns are left, and a URL is never cut,
/// also when it is wider than the row. Off a terminal the row is whole.
#[test]
fn a_title_is_cut_then_dropped_and_a_url_never_cut() {
    let long = WebSource {
        url: "https://example.com/a/very/long/path/that/goes/on".to_owned(),
        title: Some("A title that is quite long indeed".to_owned()),
    };
    let plain = |cols: u16| {
        crate::testing::bare(&sources(
            std::slice::from_ref(&long),
            &design_options(Depth::NoColor, cols),
        ))
    };
    assert_eq!(
        plain(100),
        "[1] https://example.com/a/very/long/path/that/goes/on  A title that is quite long indeed\n"
    );
    assert_eq!(
        plain(75),
        "[1] https://example.com/a/very/long/path/that/goes/on  A title that is qui\u{2026}\n"
    );
    assert_eq!(plain(70), "[1] https://example.com/a/very/long/path/that/goes/on\n");
    assert_eq!(plain(20), "[1] https://example.com/a/very/long/path/that/goes/on\n");
    let piped = efr_render::RenderOptions::new(20).with_terminal(false);
    assert_eq!(
        sources(&[long], &piped),
        "[1] https://example.com/a/very/long/path/that/goes/on  A title that is quite long indeed\n"
    );
}

/// TM-Web 13, rows 1 to 4 (row 0 is the line without searches, the foundation's
/// example test): the count last on the cost line, `stored copy` after it, and the
/// note under the line of the first such turn.
#[test]
fn the_end_lines_with_searches_are_the_web_board() {
    let usage = |input: u64, cached: u64, output: u64| {
        let mut usage = efr_protocol::Usage::new(input, output);
        usage.cached_input_tokens = cached;
        usage
    };
    let ctx = |percent: u64| efr_protocol::ContextUse {
        tokens: percent * 1_000,
        limit: 100_000,
        window: 272_000,
    };
    let line = |secs: u64, percent: u64, usage: efr_protocol::Usage, searches, stored| {
        let mut line =
            done_line(Some(Duration::from_secs(secs)), Some(&usage), Some(&ctx(percent)), G);
        add_count(&mut line, &count(searches, stored).unwrap(), G);
        line
    };
    let lines = [
        line(108, 34, usage(30_000, 26_700, 3_100), 5, false),
        line(12, 9, usage(1_500, 0, 640), 1, false),
        line(31, 22, usage(1_500, 0, 2_400), 5, true),
        vec![(stored_copy_note(G), Tone::Dim)],
    ];
    let frame = Frame::load("TM-Web", 13).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let out: String = lines.iter().map(|line| rows(line, 0, &options)).collect();
        assert_rows(&frame, 1..=4, &out, depth);
    }
}

#[test]
fn the_count_says_nothing_without_a_search() {
    assert_eq!(count(0, true), None);
    assert_eq!(count(1, false).as_deref(), Some("1 search"));
    assert_eq!(count(2, true).as_deref(), Some("2 searches, stored copy"));
    // A line without a cost gets the count after two spaces.
    let mut line = done_line(None, None, None, G);
    add_count(&mut line, "1 search", G);
    assert_eq!(crate::format::ending::plain(&line), "\u{2713} done  1 search");
}

/// TM-Web 15: under a search its first three results and how many more, with the
/// sources of the turn among them; under an open its finds. The rows stand 6 columns
/// in; `efr history` puts 2 of them before each row.
#[test]
fn the_verbose_rows_are_the_web_board() {
    let result =
        |title: &str, url: &str| WebResult { url: url.to_owned(), title: Some(title.to_owned()) };
    let mut results = vec![
        result(
            "ROB RUN \u{b7} Are We Anti-Cheat Yet?",
            "https://areweanticheatyet.com/game/rob-run",
        ),
        result("Easy Anti-Cheat games on Linux", "https://areweanticheatyet.com/"),
        result("ROB RUN 1.4 patch notes", "https://store.steampowered.com/news/app/2384710"),
    ];
    results.extend((0..5).map(|n| result("other", &format!("https://example.com/{n}"))));
    let frame = Frame::load("TM-Web", 15).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let under = results_rows(&results, &three_sources(), "      ", &options);
        assert_rows(&frame, 1..=4, &under, depth);
        let patterns = ["Easy Anti-Cheat".to_owned(), "Proton".to_owned()];
        assert_rows(&frame, 6..=6, &finds_row(&patterns, "      ", &options), depth);
    }
}

#[test]
fn the_rows_wait_for_the_steps_that_started_before_them() {
    let mut order = Order::default();
    let ended = |n: u8, step: Step| Ended {
        call_id: id(n),
        step,
        end: End::Done { results: None },
        took: None,
    };
    order.start(id(1), &search("a"));
    order.start(id(2), &search("b"));
    order.start(id(3), &find("x", "https://a.org/"));
    order.start(id(4), &search("c"));
    // The second and the find end first: nothing is written before the first.
    assert_eq!(order.end(ended(2, search("b"))), vec![]);
    assert_eq!(order.end(ended(3, find("x", "https://a.org/"))), vec![]);
    let ids = |rows: Vec<Ended>| rows.into_iter().map(|row| row.call_id).collect::<Vec<_>>();
    assert_eq!(ids(order.end(ended(1, search("a")))), vec![id(1), id(2)]);
    // An end whose start was not seen counts as a step that started now.
    assert_eq!(order.end(ended(5, search("d"))), vec![]);
    assert_eq!(ids(order.rest()), vec![id(5)]);
}

/// OpenAI starts every step without its action, so a find starts as a search with no
/// query and is a find only at its end. The rows after it must not wait for it.
#[test]
fn a_step_that_ends_as_a_find_holds_back_no_row() {
    let mut order = Order::default();
    let ended = |n: u8, step: Step| Ended {
        call_id: id(n),
        step,
        end: End::Done { results: None },
        took: None,
    };
    let unknown = Step { action: WebAction::Search, query: None, url: None, pattern: None };
    let ids = |rows: Vec<Ended>| rows.into_iter().map(|row| row.call_id).collect::<Vec<_>>();
    order.start(id(1), &unknown);
    order.start(id(2), &unknown);
    assert_eq!(order.end(ended(2, search("b"))), vec![]);
    assert_eq!(ids(order.end(ended(1, find("x", "https://a.org/")))), vec![id(2)]);
    order.start(id(3), &unknown);
    assert_eq!(ids(order.end(ended(3, search("c")))), vec![id(3)]);
    assert_eq!(ids(order.rest()), Vec::<CallId>::new());
}

#[test]
fn every_failure_has_its_words() {
    let all: Vec<(&str, &str)> = WebError::ALL
        .iter()
        .map(|error| (reason(WebAction::Search, *error), reason(WebAction::Open, *error)))
        .collect();
    assert_eq!(
        all,
        [
            ("too many requests", "too many requests"),
            ("search limit of this turn reached", "page limit of this turn reached"),
            ("web search not available", "page not available"),
            ("query not accepted", "address not accepted"),
            ("page not allowed", "page not allowed"),
            ("page not reachable", "page not reachable"),
            ("page type not supported", "page type not supported"),
            ("did not finish", "did not finish"),
            ("failed", "failed"),
        ]
    );
}

#[test]
fn a_step_without_a_subject_is_named_by_its_word() {
    let options = design_options(Depth::NoColor, 100);
    let bare = |ended: &Ended| crate::testing::bare(&row(ended, Fit::Cut, &options));
    let none = Step { action: WebAction::Search, query: None, url: None, pattern: None };
    assert_eq!(bare(&done(none, None, 2.0)), "\u{2713} web search  2s\n");
    let page_less = Step { action: WebAction::Open, query: None, url: None, pattern: None };
    assert_eq!(
        bare(&failed(page_less, WebError::NotFinished, 0.0)),
        "! web open  did not finish\n"
    );
    let unknown = Step { action: WebAction::Unknown, query: None, url: None, pattern: None };
    assert_eq!(bare(&done(unknown, Some(3), 0.05)), "\u{2713} web\n");
    // A find has no row.
    assert_eq!(bare(&done(find("x", "https://a.org/"), None, 1.0)), "");
}

#[test]
fn a_page_drops_its_scheme_and_www_and_a_long_one_is_cut_in_the_middle() {
    assert_eq!(page("https://www.protondb.com/app/2384710"), "protondb.com/app/2384710");
    assert_eq!(page("http://areweanticheatyet.com/"), "areweanticheatyet.com");
    assert_eq!(page("HTTPS://a.org/x?y=1"), "a.org/x?y=1");
    let options = design_options(Depth::NoColor, 50);
    let step = open("https://store.steampowered.com/app/2384710/ROB_RUN/");
    let written = crate::testing::bare(&row(&done(step, None, 2.0), Fit::Cut, &options));
    assert_eq!(written, "\u{2713} web open store.steampowered.com/\u{2026}/ROB_RUN/  2s\n");
    // An OSC 8 link to the whole URL where the terminal has links.
    let linked = design_options(Depth::TrueColor, 100).with_hyperlinks(true);
    let painted = row(&done(open("https://a.org/b c"), None, 2.0), Fit::Cut, &linked);
    assert!(painted.contains("\x1b]8;;https://a.org/b%20c\x1b\\"), "{painted:?}");
}

#[test]
fn the_ascii_glyphs_stand_in() {
    let options = design_options(Depth::NoColor, 40).with_glyphs(&Glyphs::ASCII);
    let long = done(search("one two three four five six seven eight"), Some(3), 2.0);
    let written = crate::testing::bare(&row(&long, Fit::Cut, &options));
    assert_eq!(written, "+ web one two three...  3 results - 2s\n");
}

/// `efr history` keeps the whole query: on a terminal it goes on in the next rows, 6
/// columns in, as a command does; off a terminal it is one row.
#[test]
fn the_whole_query_wraps_in_history() {
    let options = design_options(Depth::NoColor, 40);
    let long = done(search("one two three four five six seven eight nine"), Some(3), 2.0);
    let written = crate::testing::bare(&row(&long, Fit::Wrap, &options));
    assert_eq!(
        written,
        "\u{2713} web one two three four five six seven\n      eight nine  3 results \u{b7} 2s\n"
    );
    let piped = efr_render::RenderOptions::new(40).with_terminal(false);
    assert_eq!(
        row(&long, Fit::Cut, &piped),
        "\u{2713} web one two three four five six seven eight nine  3 results \u{b7} 2s\n"
    );
}

/// DECISIONS-web 3: the count, the time and the reason are never cut, also in
/// `efr history`, where only the query wraps. They stay on the last row of the query,
/// or go to a row of their own at the indent.
#[test]
fn history_never_breaks_the_count_the_time_or_the_reason() {
    let options = design_options(Depth::NoColor, 60);
    let bare = |ended: &Ended| crate::testing::bare(&row(ended, Fit::Wrap, &options));
    let long = done(search("Proton EasyAntiCheat Runtime install 2026 Linux"), Some(10), 7.0);
    assert_eq!(
        bare(&long),
        "\u{2713} web Proton EasyAntiCheat Runtime install 2026 Linux\n      10 results \u{b7} 7s\n"
    );
    let refused =
        failed(search("\"ROB RUN\" 1.4 patch notes Linux Proton"), WebError::TooManyRequests, 1.0);
    assert_eq!(
        bare(&refused),
        "! web \"ROB RUN\" 1.4 patch notes Linux Proton\n      too many requests \u{b7} 1s\n"
    );
    let fits = done(search("Proton EasyAntiCheat Runtime"), Some(10), 7.0);
    assert_eq!(bare(&fits), "\u{2713} web Proton EasyAntiCheat Runtime  10 results \u{b7} 7s\n");
}

/// A query that is empty or only spaces names nothing: the row and the status row say
/// `web search` as for no query.
#[test]
fn a_blank_query_is_named_by_its_word() {
    let options = design_options(Depth::NoColor, 100);
    for query in ["", "   "] {
        let ended = done(search(query), Some(10), 2.0);
        assert_eq!(
            crate::testing::bare(&row(&ended, Fit::Cut, &options)),
            "\u{2713} web search  10 results \u{b7} 2s\n",
            "{query:?}"
        );
        assert_eq!(search(query).detail(), None, "{query:?}");
    }
}

/// The cut of a page keeps the end of its path (the note of TM-Web 5:
/// `store.steampowered.com/…/ROB_RUN/`); a query string goes first, as `?…`.
#[test]
fn the_cut_of_a_page_drops_its_query_string_before_its_path() {
    let url = "https://store.steampowered.com/app/2384710/ROB_RUN/?snr=1_4_4__tab-Specials&curator_clanid=1234567&utm_source=x";
    let at = |cols| {
        let options = design_options(Depth::NoColor, cols);
        crate::testing::bare(&row(&done(open(url), None, 2.0), Fit::Cut, &options))
    };
    assert_eq!(
        at(100),
        "\u{2713} web open store.steampowered.com/app/2384710/ROB_RUN/?\u{2026}  2s\n"
    );
    assert_eq!(at(50), "\u{2713} web open store.steampowered.com/\u{2026}/ROB_RUN/?\u{2026}  2s\n");
}

/// `efr history` keeps the whole page and goes on at efr's indent, so the terminal
/// never wraps it at column 0; each piece links the whole URL.
#[test]
fn a_long_page_goes_on_at_the_indent_in_history() {
    let url = "https://store.steampowered.com/app/2384710/ROB_RUN/?snr=1_4_4__tab-Specials&curator_clanid=1234567";
    let options = design_options(Depth::NoColor, 40);
    let written = crate::testing::bare(&row(&done(open(url), None, 2.0), Fit::Wrap, &options));
    assert_eq!(
        written,
        "\u{2713} web open store.steampowered.com/app/23\n      84710/ROB_RUN/?snr=1_4_4__tab-Spec\n      ials&curator_clanid=1234567  2s\n"
    );
    for line in written.lines() {
        assert!(efr_render::text_width(line, efr_render::WidthMethod::Grapheme) <= 40, "{line}");
    }
    let linked = design_options(Depth::TrueColor, 40).with_hyperlinks(true);
    let painted = row(&done(open(url), None, 2.0), Fit::Wrap, &linked);
    assert_eq!(
        painted.matches("\x1b]8;;https://store.steampowered.com/").count(),
        3,
        "{painted:?}"
    );
}

/// DECISIONS-web 3: the query shows as the model sent it. The joiners of an emoji
/// sequence and of scripts such as Persian stay; a bidi override and other format
/// characters still show as `U+FFFD`.
#[test]
fn a_query_keeps_its_joiners_and_loses_its_bidi_controls() {
    let options = design_options(Depth::NoColor, 100);
    let family = "\u{1f469}\u{200d}\u{1f469}\u{200d}\u{1f467} trip";
    let persian = "\u{0645}\u{06cc}\u{200c}\u{062e}\u{0648}\u{0627}\u{0647}\u{0645}";
    for query in [family, persian] {
        let written =
            crate::testing::bare(&row(&done(search(query), None, 2.0), Fit::Cut, &options));
        assert_eq!(written, format!("\u{2713} web {query}  2s\n"));
        assert_eq!(search(query).detail().as_deref(), Some(query));
    }
    let turned =
        crate::testing::bare(&row(&done(search("a\u{202e}b"), None, 2.0), Fit::Cut, &options));
    assert_eq!(turned, "\u{2713} web a\u{fffd}b  2s\n");
}
