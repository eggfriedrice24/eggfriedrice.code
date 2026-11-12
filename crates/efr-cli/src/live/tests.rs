use efr_render::{RenderOptions, Renderer};
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, prop_oneof, proptest};
use proptest::strategy::{Just, Strategy};

use efr_render::{WidthMethod, display_width};

use super::{Cursor, LiveZone, Tail, rows_of};
use crate::terminal::Size;
use crate::testing::screen::Screen;
use crate::testing::{Grid, readable};

const BEGIN: &str = "\x1b[?2026h";
const END: &str = "\x1b[?2026l";

/// Widths by code point, as most terminals count.
const CP: WidthMethod = WidthMethod::CodePoint;

fn size(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

#[test]
fn width_skips_colours_and_hyperlinks() {
    assert_eq!(display_width("plain", CP), 5);
    assert_eq!(display_width("\x1b[1;35mTitle\x1b[0m", CP), 5);
    assert_eq!(display_width("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\", CP), 4);
    assert_eq!(display_width("\x1b]8;;https://example.com\x07link\x1b]8;;\x07", CP), 4);
}

#[test]
fn wide_characters_take_two_columns() {
    assert_eq!(display_width("日本", CP), 4);
    assert_eq!(display_width("a\u{301}", CP), 1);
}

#[test]
fn a_line_that_fits_takes_one_row() {
    assert_eq!(rows_of("", 10, CP), 0);
    assert_eq!(rows_of("abc\n", 10, CP), 1);
    assert_eq!(rows_of("\n", 10, CP), 1);
    assert_eq!(rows_of("0123456789\n", 10, CP), 1);
}

#[test]
fn a_long_line_wraps_onto_more_rows() {
    assert_eq!(rows_of("01234567890\n", 10, CP), 2);
    assert_eq!(rows_of(&format!("{}\n", "x".repeat(25)), 10, CP), 3);
    assert_eq!(rows_of(&format!("{}\n{}\n", "x".repeat(25), "y".repeat(3)), 10, CP), 4);
    assert_eq!(rows_of("日本語日本\n", 4, CP), 3);
}

#[test]
fn colours_do_not_count_towards_wrapping() {
    let line = format!("\x1b[2m{}\x1b[0m\n", "x".repeat(10));
    assert_eq!(rows_of(&line, 10, CP), 1);
}

#[test]
fn the_first_redraw_erases_nothing() {
    let mut zone = LiveZone::default();
    let out = zone.redraw("done\n", "typing\n", size(40, 20));
    assert_eq!(out, format!("{BEGIN}done\ntyping\n{END}"));
}

#[test]
fn a_redraw_moves_up_over_every_wrapped_row_of_the_old_live_zone() {
    let mut zone = LiveZone::default();
    let wide = format!("{}\n", "x".repeat(25));
    zone.redraw("", &format!("{wide}short\n"), size(10, 20));
    let out = zone.redraw("committed\n", "next\n", size(10, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[4A\\e[Jcommitted\nnext\n\\e[?2026l");
}

#[test]
fn nothing_is_written_when_nothing_changes() {
    let mut zone = LiveZone::default();
    zone.redraw("", "same\n", size(40, 20));
    assert_eq!(zone.redraw("", "same\n", size(40, 20)), "");
}

#[test]
fn clearing_the_live_zone_erases_it_and_shows_nothing() {
    let mut zone = LiveZone::default();
    zone.redraw("", "one\ntwo\n", size(40, 20));
    let out = zone.redraw("", "", size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[2A\\e[J\\e[?2026l");
    assert_eq!(zone.redraw("", "", size(40, 20)), "");
}

#[test]
fn after_a_resize_the_old_live_zone_is_measured_at_the_new_width() {
    let mut zone = LiveZone::default();
    let line = format!("{}\n", "x".repeat(30));
    zone.redraw("", &line, size(40, 20));
    // The terminal reflowed the 30 columns onto two rows of 20.
    let out = zone.redraw("", &line, size(20, 20));
    assert_eq!(readable(&out), format!("\\e[?2026h\\r\\e[2A\\e[J{line}\\e[?2026l"));
    // And back: one row at 40 again.
    let out = zone.redraw("", "", size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[1A\\e[J\\e[?2026l");
}

#[test]
fn a_live_zone_taller_than_the_screen_shows_its_last_lines() {
    let mut zone = LiveZone::default();
    let live: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    let out = zone.redraw("", &live, size(40, 5));
    assert_eq!(out, format!("{BEGIN}line 7\nline 8\nline 9\nline 10\n{END}"));
    // Only the four rows that were shown are erased.
    let out = zone.redraw("", "", size(40, 5));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[4A\\e[J\\e[?2026l");
}

#[test]
fn clipping_counts_wrapped_rows() {
    let mut zone = LiveZone::default();
    let live = format!("first\n{}\nlast\n", "x".repeat(25));
    // Four rows fit: the wrapped line takes three, and `last` one.
    let out = zone.redraw("", &live, size(10, 5));
    assert_eq!(out, format!("{BEGIN}{}\nlast\n{END}", "x".repeat(25)));
}

/// A line of `rows` rows of ten letters each: `a` ten times, then `b`, and so on.
fn rows_of_letters(rows: u8) -> String {
    (0..rows).flat_map(|row| std::iter::repeat_n(char::from(b'a' + row), 10)).collect()
}

#[test]
fn a_single_line_taller_than_the_screen_shows_its_last_rows() {
    let mut zone = LiveZone::default();
    let line = rows_of_letters(10);
    let out = zone.redraw("ok\n", &format!("{line}\n"), size(10, 5));
    // Four rows fit: the last four of the line's ten.
    assert_eq!(out, format!("{BEGIN}ok\n{}\n{END}", &line[60..]));
    let mut screen = Screen::new(10, 5, CP);
    screen.write(&out);
    assert_eq!(screen.lines(), ["ok", "gggggggggg", "hhhhhhhhhh", "iiiiiiiiii", "jjjjjjjjjj"]);
    // The line grows by a row: the top row of the live zone goes, the rows below it move
    // up, and only the line from its last old cell is written.
    let longer = format!("{line}kkk\n");
    let out = zone.redraw("", &longer, size(10, 5));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[4A\\e[1M\\r\\e[2B\\e[9C\\e[Jjkkk\n\\e[?2026l");
    screen.write(&out);
    assert_eq!(screen.lines(), ["ok", "hhhhhhhhhh", "iiiiiiiiii", "jjjjjjjjjj", "kkk"]);
    let out = zone.redraw("", "", size(10, 5));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[4A\\e[J\\e[?2026l");
}

#[test]
fn a_tall_line_shows_its_last_rows_above_the_status_row_and_the_tail() {
    let mut zone = LiveZone::default();
    let mut grid = Grid::new(10);
    let body = format!("{}\n", rows_of_letters(8));
    grid.write(&zone.draw("", &body, "status\n", &input_row(), size(10, 6)));
    // Five rows: two of the line, the status row and the two rows of the tail.
    assert_eq!(grid.lines(), ["gggggggggg", "hhhhhhhhhh", "status", "> first", "  typ"]);
    assert_eq!(grid.cursor(), (4, 5));
    // A tick writes only what changed in the status row.
    let out = zone.draw("", &body, "status 2\n", &input_row(), size(10, 6));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[2A\\e[5Cs 2\\r\\e[2B\\e[5C\\e[?2026l");
    grid.write(&out);
    assert_eq!(grid.lines(), ["gggggggggg", "hhhhhhhhhh", "status 2", "> first", "  typ"]);
}

#[test]
fn the_rows_cut_from_a_line_open_its_colours_and_its_hyperlink_again() {
    let mut zone = LiveZone::default();
    let line = format!(
        "\x1b[1m{}\x1b[0m\x1b[33m\x1b]8;;https://e.io\x1b\\{}\x1b]8;;\x1b\\\x1b[0m",
        "a".repeat(15),
        "b".repeat(15),
    );
    let out = zone.redraw("", &format!("{line}\n"), size(10, 2));
    // One row fits: the last, which starts in the link.
    assert_eq!(
        readable(&out),
        "\\e[?2026h\\e[33m\\e]8;;https://e.io\\e\\bbbbbbbbbb\\e]8;;\\e\\\\e[0m\n\\e[?2026l"
    );
    // Two rows fit: the last two, which start in bold.
    let out = zone.redraw("", &format!("{line}\n"), size(10, 3));
    assert_eq!(
        readable(&out),
        "\\e[?2026h\\r\\e[1A\\e[J\\e[1maaaaa\\e[0m\\e[33m\\e]8;;https://e.io\\e\\bbbbbbbbbbbbbbb\\e]8;;\\e\\\\e[0m\n\\e[?2026l"
    );
}

#[test]
fn a_wide_character_that_does_not_fit_at_the_end_of_a_row_starts_the_next() {
    // Three columns: each wide character takes a row of its own, as on the terminal.
    assert_eq!(rows_of("\u{65e5}\u{672c}\u{8a9e}\n", 3, CP), 3);
    let mut zone = LiveZone::default();
    let out = zone.redraw("", "\u{65e5}\u{672c}\u{8a9e}\n", size(3, 3));
    assert_eq!(out, format!("{BEGIN}\u{672c}\u{8a9e}\n{END}"));
}

#[test]
fn a_tall_line_of_clusters_is_cut_where_the_terminal_starts_a_row() {
    // By grapheme cluster a family takes two columns: five to a row of ten.
    let line = FAMILY.repeat(12);
    let mut zone = LiveZone::new(GR);
    let out = zone.redraw("", &format!("{line}\n"), size(10, 2));
    assert_eq!(out, format!("{BEGIN}{}\n{END}", FAMILY.repeat(2)));
    let out = zone.redraw("", &format!("{line}\n"), size(10, 3));
    assert_eq!(readable(&out), format!("\\e[?2026h\\r\\e[1A\\e[J{}\n\\e[?2026l", FAMILY.repeat(7)));
}

#[test]
fn an_unknown_size_renders_at_80_columns_without_clipping() {
    let mut zone = LiveZone::default();
    let live = format!("{}\n", "x".repeat(81));
    zone.redraw("", &live, Size::default());
    let out = zone.redraw("", "", Size::default());
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[2A\\e[J\\e[?2026l");
}

/// A ZWJ emoji: three wide code points, one wide cluster.
const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
/// A flag: two regional indicators, one wide cluster.
const FLAG: &str = "\u{1f1fa}\u{1f1f8}";
/// A narrow heart made wide by variation selector 16.
const HEART: &str = "\u{2764}\u{fe0f}";

/// Widths by grapheme cluster, as Ghostty counts.
const GR: WidthMethod = WidthMethod::Grapheme;

#[test]
fn zwj_emoji_flags_and_vs16_take_the_rows_that_the_terminal_gives_them() {
    let families = format!("{}\n", FAMILY.repeat(4));
    assert_eq!(rows_of(&families, 10, CP), 3);
    assert_eq!(rows_of(&families, 10, GR), 1);
    let flags = format!("{}\n", FLAG.repeat(6));
    assert_eq!(rows_of(&flags, 10, CP), 2);
    assert_eq!(rows_of(&flags, 10, GR), 2);
    let hearts = format!("{}\n", HEART.repeat(8));
    assert_eq!(rows_of(&hearts, 10, CP), 1);
    assert_eq!(rows_of(&hearts, 10, GR), 2);
}

#[test]
fn a_redraw_moves_up_over_the_rows_that_the_terminal_counted() {
    let live = format!("{}\n{}\n", FAMILY.repeat(4), HEART.repeat(8));
    for (method, up) in [(CP, 4), (GR, 3)] {
        let mut zone = LiveZone::new(method);
        zone.redraw("", &live, size(10, 20));
        let out = zone.redraw("", "next\n", size(10, 20));
        assert_eq!(readable(&out), format!("\\e[?2026h\\r\\e[{up}A\\e[Jnext\n\\e[?2026l"));
    }
}

/// An input row of two lines with the cursor after `typ` on its second line.
fn input_row() -> Tail {
    Tail { text: "> first\n  typ\n".to_owned(), cursor: Some(Cursor { line: 1, column: 5 }) }
}

#[test]
fn the_cursor_waits_in_the_tail_and_the_next_redraw_starts_from_there() {
    let mut zone = LiveZone::default();
    let out = zone.draw("", "body\n", "status\n", &input_row(), size(40, 20));
    // Up one row from below the live zone, to column 5 of the tail's second line.
    assert_eq!(readable(&out), "\\e[?2026hbody\nstatus\n> first\n  typ\n\\e[1A\\r\\e[5C\\e[?2026l");
    let mut grid = Grid::new(40);
    grid.write(&out);
    assert_eq!(grid.cursor(), (3, 5));
    // The cursor is three rows below the top of the live zone, not four.
    let out = zone.draw("done\n", "body\n", "status\n", &Tail::default(), size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[3A\\e[Jdone\nbody\nstatus\n\\e[?2026l");
    grid.write(&out);
    assert_eq!(grid.lines(), ["done", "body", "status"]);
    assert_eq!(grid.cursor(), (3, 0));
}

#[test]
fn a_tick_rewrites_the_status_row_above_the_tail_and_puts_the_cursor_back() {
    let mut zone = LiveZone::default();
    let mut grid = Grid::new(40);
    grid.write(&zone.draw("", "body\n", "status 1\n", &input_row(), size(40, 20)));
    let out = zone.draw("", "body\n", "status 2\n", &input_row(), size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[2A\\e[6C 2\\r\\e[2B\\e[5C\\e[?2026l");
    grid.write(&out);
    assert_eq!(grid.lines(), ["body", "status 2", "> first", "  typ"]);
    assert_eq!(grid.cursor(), (3, 5));
}

#[test]
fn text_that_streams_at_the_end_of_a_line_writes_only_its_new_characters() {
    let mut zone = LiveZone::default();
    let mut screen = Screen::new(40, 20, CP);
    screen.write(&zone.draw("", "Hello\n", "status\n", &input_row(), size(40, 20)));
    let out = zone.draw("", "Hello world\n", "status\n", &input_row(), size(40, 20));
    // From the last old cell, which a next character could join, then the new ones.
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[3A\\e[4Co world\\r\\e[3B\\e[5C\\e[?2026l");
    screen.write(&out);
    assert_eq!(screen.lines(), ["Hello world", "status", "> first", "  typ"]);
    assert_eq!(screen.cursor(), (3, 5));
}

#[test]
fn a_line_that_wraps_onto_a_new_row_writes_from_its_last_old_cell_to_the_end() {
    let mut zone = LiveZone::default();
    let mut screen = Screen::new(10, 20, CP);
    screen.write(&zone.draw("", "abcdefgh\n", "status\n", &Tail::default(), size(10, 20)));
    let out = zone.draw("", "abcdefgh ijk\n", "status\n", &Tail::default(), size(10, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[2A\\e[7C\\e[Jh ijk\nstatus\n\\e[?2026l");
    screen.write(&out);
    assert_eq!(screen.lines(), ["abcdefgh i", "jk", "status"]);
    assert_eq!(screen.cursor(), (3, 0));
}

#[test]
fn a_blank_row_with_a_background_that_commits_over_a_blank_row_gets_its_fill() {
    // A blank line of a code block commits where a blank row of the live zone stood.
    // No cell of the row changes, only the erase that fills it with the background.
    let fill = "\x1b[48;2;20;24;32m\x1b[K\x1b[0m\n";
    let mut zone = LiveZone::default();
    let first = zone.draw("", "\n", "status\n", &Tail::default(), size(40, 20));
    assert!(first.contains("status"), "{}", readable(&first));
    let out = zone.draw(fill, "", "status\n", &Tail::default(), size(40, 20));
    assert!(out.contains(fill), "the fill is written: {}", readable(&out));
    // The same when the row is the last of the live zone and nothing follows it.
    let mut zone = LiveZone::default();
    zone.draw("", "\n", "", &Tail::default(), size(40, 20));
    let out = zone.draw(fill, "", "", &Tail::default(), size(40, 20));
    assert!(out.contains(fill.trim_end()), "the fill is written: {}", readable(&out));
}

#[test]
fn a_block_that_commits_as_it_stands_writes_nothing() {
    let mut zone = LiveZone::default();
    let mut screen = Screen::new(40, 20, CP);
    screen.write(&zone.draw("", "para\n", "status\n", &Tail::default(), size(40, 20)));
    // The paragraph stays where it stands, and the live zone starts below it.
    assert_eq!(zone.draw("para\n", "", "status\n", &Tail::default(), size(40, 20)), "");
    let out = zone.draw("", "", "", &Tail::default(), size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[1A\\e[J\\e[?2026l");
    screen.write(&out);
    assert_eq!(screen.lines(), ["para"]);
}

#[test]
fn a_cursor_that_moves_redraws_the_live_zone() {
    let mut zone = LiveZone::default();
    let mut grid = Grid::new(40);
    grid.write(&zone.draw("", "", "status\n", &input_row(), size(40, 20)));
    let mut moved = input_row();
    moved.cursor = Some(Cursor { line: 0, column: 2 });
    let out = zone.draw("", "", "status\n", &moved, size(40, 20));
    assert!(!out.is_empty());
    grid.write(&out);
    assert_eq!(grid.lines(), ["status", "> first", "  typ"]);
    assert_eq!(grid.cursor(), (1, 2));
    assert_eq!(zone.draw("", "", "status\n", &moved, size(40, 20)), "", "nothing new");
}

#[test]
fn a_tail_without_a_status_row_takes_the_cursor_too() {
    let mut zone = LiveZone::default();
    let mut grid = Grid::new(40);
    let tail = Tail { text: "> x\n".to_owned(), cursor: Some(Cursor { line: 0, column: 3 }) };
    grid.write(&zone.draw("", "", "", &tail, size(40, 20)));
    assert_eq!(grid.cursor(), (0, 3));
    // The note takes the place of the tail, in the top row of the live zone.
    let out = zone.draw("note\n", "", "", &Tail::default(), size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\rnote\n\\e[?2026l");
    grid.write(&out);
    assert_eq!(grid.lines(), ["note"]);
}

#[test]
fn a_tail_that_does_not_fit_gets_no_cursor() {
    let mut zone = LiveZone::default();
    let out = zone.draw("", "", "status\n", &input_row(), size(40, 2));
    assert!(!out.contains("\\e[5C") && !out.contains("\x1b[5C"), "{}", readable(&out));
}

proptest! {
    /// The CLI's count of rows agrees with the renderer's for every live zone it
    /// produces, wrapped lines included, so moving up by either lands on the same row.
    #[test]
    fn rows_agree_with_the_renderer(
        words in prop::collection::vec("[a-z]{1,12}|`[a-z]{1,6}`|\\*\\*[a-z]{1,6}\\*\\*", 1..60),
        breaks in prop::collection::vec(0_usize..60, 0..6),
        width in 8_u16..60,
    ) {
        let mut text = String::new();
        for (index, word) in words.iter().enumerate() {
            text.push_str(word);
            text.push(if breaks.contains(&index) { '\n' } else { ' ' });
        }
        let mut renderer = Renderer::new(RenderOptions::new(width));
        let update = renderer.push(&text);
        assert_eq!(rows_of(update.live(), width, CP), update.live_rows(), "{:?}", update.live());
    }
}

/// A piece of a reply's markdown: words, spaces, line breaks, styles, a list item, a
/// heading, a link, wide characters, clusters and a word wider than a narrow screen.
fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => "[a-z]{1,8}".prop_map(|word| word),
        8 => Just(" ".to_owned()),
        2 => Just("\n".to_owned()),
        2 => Just("\n\n".to_owned()),
        1 => Just("**bold words**".to_owned()),
        1 => Just("`code`".to_owned()),
        1 => Just("\n- item ".to_owned()),
        1 => Just("\n\n## head ".to_owned()),
        1 => Just("[a link](https://e.io/x)".to_owned()),
        1 => Just("\u{65e5}\u{672c}\u{8a9e}".to_owned()),
        1 => Just(FAMILY.to_owned()),
        1 => Just(FLAG.to_owned()),
        1 => Just(HEART.to_owned()),
        1 => Just("e\u{301}".to_owned()),
        1 => Just("x".repeat(70)),
    ]
}

/// The status row of frame `frame` in variant `variant`: none, a short row whose spinner
/// and time move, or a row wider than a narrow screen.
fn status_row(variant: u8, frame: usize) -> String {
    const SPINNER: [char; 4] = ['\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}'];
    match variant % 4 {
        0 => String::new(),
        3 => format!("\x1b[2m{} {}\x1b[0m\n", SPINNER[frame % 4], "detail ".repeat(9)),
        _ => format!(
            "\x1b[36m{}\x1b[0m \x1b[2mthinking\x1b[0m  {}s\n",
            SPINNER[frame % 4],
            frame / 3
        ),
    }
}

/// The tail in variant `variant`: none, or the input row with the cursor somewhere.
fn tail_rows(variant: u8, frame: usize) -> Tail {
    match variant % 3 {
        0 => Tail::default(),
        1 => input_row(),
        _ => Tail {
            text: format!("> {}\n", "w".repeat(frame % 7)),
            cursor: Some(Cursor { line: 0, column: 2 + frame % 7 }),
        },
    }
}

/// Plays `text` in the pieces that `cuts` makes, through a live zone that writes only
/// what changed and one that writes all each frame, onto two screens, and checks after
/// each frame that both screens show the same. Each frame also gets a frame of its own
/// for a tick of the status row.
fn replay(text: &str, cuts: &[(usize, u8, u8)], size: Size, method: WidthMethod) {
    let options = RenderOptions::new(size.cols).with_width_method(method);
    let mut renderer = Renderer::new(options);
    let mut changes = LiveZone::new(method);
    let mut whole = LiveZone::whole(method);
    let mut left = Screen::new(size.cols, size.rows, method);
    let mut right = Screen::new(size.cols, size.rows, method);
    for screen in [&mut left, &mut right] {
        screen.write("$ efr a question\nabove the live zone\n");
    }
    let mut at = 0;
    let mut frames = Vec::new();
    let mut frame = 0;
    let mut check = |committed: &str,
                     body: &str,
                     status: &str,
                     tail: &Tail,
                     frames: &mut Vec<String>| {
        let one = changes.draw(committed, body, status, tail, size);
        let two = whole.draw(committed, body, status, tail, size);
        left.write(&one);
        right.write(&two);
        frames.push(readable(&one));
        assert!(
            left.shot() == right.shot(),
            "frames:\n{}\nthe frames that write only what changed:\n{}\nthe frames that write all:\n{}",
            frames.join("\n"),
            left.text(),
            right.text(),
        );
    };
    for (cut, status, tail) in cuts {
        let mut end = (*cut).clamp(at, text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let update = renderer.push(&text[at..end]);
        at = end;
        let (committed, body) = (update.committed().to_owned(), update.live().to_owned());
        let tail = tail_rows(*tail, frame);
        check(&committed, &body, &status_row(*status, frame), &tail, &mut frames);
        frame += 1;
        check("", &body, &status_row(*status, frame), &tail, &mut frames);
        frame += 1;
    }
    let mut rest = renderer.push(&text[at..]).committed().to_owned();
    rest.push_str(&renderer.finish());
    check(&rest, "", "", &Tail::default(), &mut frames);
}

proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(400))]

    /// After each frame the screen shows what a frame that erases the whole live zone and
    /// writes it again shows: the same cells with the same colours and links, the same
    /// scrollback and the cursor in the same place.
    #[test]
    fn a_frame_shows_what_a_full_redraw_shows(
        pieces in prop::collection::vec(piece(), 1..80),
        cuts in prop::collection::vec((0_usize..1200, 0_u8..8, 0_u8..6), 1..40),
        cols in 8_u16..48,
        rows in 4_u16..14,
        grapheme in proptest::bool::ANY,
    ) {
        let text = pieces.concat();
        let mut cuts = cuts;
        cuts.sort_by_key(|(cut, _, _)| *cut);
        let method = if grapheme { GR } else { CP };
        replay(&text, &cuts, size(cols, rows), method);
    }
}

#[test]
fn a_long_paragraph_that_streams_shows_what_a_full_redraw_shows() {
    let text: String = (0..400).map(|n| format!("word{n} ")).collect();
    let cuts: Vec<(usize, u8, u8)> =
        (0..text.len()).step_by(7).map(|cut| (cut, (cut % 3) as u8, (cut % 5) as u8)).collect();
    for (cols, rows) in [(40, 10), (80, 24), (23, 7)] {
        replay(&text, &cuts, size(cols, rows), CP);
        replay(&text, &cuts, size(cols, rows), GR);
    }
}

/// Draws `frames` in turn, each the committed text and the body of the live zone, with
/// a status row, through a live zone that writes only what changed and one that writes
/// all each frame, onto two screens, and checks after each frame that both screens
/// show the same.
fn play(frames: &[(String, String)], size: Size, method: WidthMethod) {
    let mut changes = LiveZone::new(method);
    let mut whole = LiveZone::whole(method);
    let mut left = Screen::new(size.cols, size.rows, method);
    let mut right = Screen::new(size.cols, size.rows, method);
    let mut written = Vec::new();
    for (index, (committed, body)) in frames.iter().enumerate() {
        let status = status_row((index % 4) as u8, index);
        let one = changes.draw(committed, body, &status, &Tail::default(), size);
        let two = whole.draw(committed, body, &status, &Tail::default(), size);
        left.write(&one);
        right.write(&two);
        written.push(readable(&one));
        assert!(
            left.shot() == right.shot(),
            "frames:\n{}\nthe frames that write only what changed:\n{}\nthe frames that write all:\n{}",
            written.join("\n"),
            left.text(),
            right.text(),
        );
    }
}

/// The frames of `lines` one after another as bodies of the live zone, none committed.
fn bodies(lines: &[&str]) -> Vec<(String, String)> {
    lines.iter().map(|line| (String::new(), format!("{line}\n"))).collect()
}

#[test]
fn a_mark_after_a_style_that_a_line_loses_goes_from_the_screen() {
    for method in [CP, GR] {
        play(&bodies(&["\x1b[1mbo\x1b[0m\u{301} x", "\x1b[1mbo\x1b[0m x"]), size(40, 10), method);
        play(
            &bodies(&["see \x1b[1mcafe\x1b[0m\u{301}s", "see \x1b[1mcafe\x1b[0m!"]),
            size(40, 10),
            method,
        );
        play(&bodies(&["a\x1b[0m\u{200d}\u{1f469}", "a\x1b[0m"]), size(40, 10), method);
    }
}

/// A piece of a painted line: words, wide characters, clusters, marks of no width
/// alone, styles and links.
fn painted() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[a-z]{1,6}".prop_map(|word| word),
        4 => Just(" ".to_owned()),
        1 => Just("\u{65e5}\u{672c}".to_owned()),
        1 => Just("e\u{301}".to_owned()),
        1 => Just("\u{301}".to_owned()),
        1 => Just("\u{200d}".to_owned()),
        1 => Just("\u{fe0f}".to_owned()),
        1 => Just(FAMILY.to_owned()),
        1 => Just(FLAG.to_owned()),
        1 => Just(HEART.to_owned()),
        2 => Just("\x1b[1m".to_owned()),
        2 => Just("\x1b[0m".to_owned()),
        1 => Just("\x1b[38;2;1;2;3m".to_owned()),
        1 => Just("\x1b]8;;https://e.io\x1b\\".to_owned()),
        1 => Just("\x1b]8;;\x1b\\".to_owned()),
        1 => Just("x".repeat(31)),
    ]
}

/// One change of the lines of the live zone from one frame to the next. A line is a
/// list of pieces, so a cut never splits an escape sequence.
#[derive(Debug, Clone)]
enum Edit {
    /// Adds the piece at the end of the last line.
    Grow(String),
    /// Cuts this many pieces from the end of a line.
    Cut(usize, usize),
    /// Puts these pieces in place of a line.
    Replace(usize, Vec<String>),
    /// Drops a line.
    Drop(usize),
    /// Adds a line.
    Add(Vec<String>),
    /// Commits the first line: it leaves the live zone and goes above it.
    Commit,
}

fn edit() -> impl Strategy<Value = Edit> {
    let line = || prop::collection::vec(painted(), 0..6);
    prop_oneof![
        4 => painted().prop_map(Edit::Grow),
        2 => (0_usize..8, 1_usize..4).prop_map(|(line, count)| Edit::Cut(line, count)),
        2 => (0_usize..8, line()).prop_map(|(at, pieces)| Edit::Replace(at, pieces)),
        1 => (0_usize..8).prop_map(Edit::Drop),
        2 => line().prop_map(Edit::Add),
        1 => Just(Edit::Commit),
    ]
}

/// A line of the live zone as the renderer ends one: no style and no link stay open.
fn closed(pieces: &[String]) -> String {
    format!("{}\x1b[0m\x1b]8;;\x1b\\\n", pieces.concat())
}

/// The frames that `edits` make, one for each edit.
fn edited(edits: &[Edit]) -> Vec<(String, String)> {
    let mut lines: Vec<Vec<String>> = vec![Vec::new()];
    let mut frames = Vec::new();
    for edit in edits {
        let mut committed = String::new();
        let count = lines.len().max(1);
        match edit {
            Edit::Grow(piece) => match lines.last_mut() {
                Some(last) => last.push(piece.clone()),
                None => lines.push(vec![piece.clone()]),
            },
            Edit::Cut(at, pieces) => {
                if let Some(line) = lines.get_mut(at % count) {
                    line.truncate(line.len().saturating_sub(*pieces));
                }
            }
            Edit::Replace(at, pieces) => {
                if let Some(line) = lines.get_mut(at % count) {
                    line.clone_from(pieces);
                }
            }
            Edit::Drop(at) => {
                if !lines.is_empty() {
                    lines.remove(at % count);
                }
            }
            Edit::Add(pieces) => lines.push(pieces.clone()),
            Edit::Commit => {
                if !lines.is_empty() {
                    committed = closed(&lines.remove(0));
                }
            }
        }
        let body: String = lines.iter().map(|line| closed(line)).collect();
        frames.push((committed, body));
    }
    frames
}

proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(600))]

    /// As [`a_frame_shows_what_a_full_redraw_shows`], but the lines also get shorter,
    /// change in the middle, go away and commit, on screens from 2 columns and 1 row.
    #[test]
    fn a_frame_that_cuts_or_replaces_lines_shows_what_a_full_redraw_shows(
        edits in prop::collection::vec(edit(), 1..30),
        cols in 2_u16..30,
        rows in 1_u16..8,
        grapheme in proptest::bool::ANY,
    ) {
        let method = if grapheme { GR } else { CP };
        play(&edited(&edits), size(cols, rows), method);
    }
}
