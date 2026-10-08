use efr_render::{RenderOptions, Renderer};
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, proptest};

use efr_render::{WidthMethod, display_width};

use super::{Cursor, LiveZone, Measured, Tail, rows_of};
use crate::terminal::Size;
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
    let out = zone.redraw("done\n", "typing\n", None, size(40, 20));
    assert_eq!(out, format!("{BEGIN}done\ntyping\n{END}"));
}

#[test]
fn a_redraw_moves_up_over_every_wrapped_row_of_the_old_live_zone() {
    let mut zone = LiveZone::default();
    let wide = format!("{}\n", "x".repeat(25));
    zone.redraw("", &format!("{wide}short\n"), None, size(10, 20));
    let out = zone.redraw("committed\n", "next\n", None, size(10, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[4A\\e[Jcommitted\nnext\n\\e[?2026l");
}

#[test]
fn nothing_is_written_when_nothing_changes() {
    let mut zone = LiveZone::default();
    zone.redraw("", "same\n", None, size(40, 20));
    assert_eq!(zone.redraw("", "same\n", None, size(40, 20)), "");
}

#[test]
fn clearing_the_live_zone_erases_it_and_shows_nothing() {
    let mut zone = LiveZone::default();
    zone.redraw("", "one\ntwo\n", None, size(40, 20));
    let out = zone.redraw("", "", None, size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[2A\\e[J\\e[?2026l");
    assert_eq!(zone.redraw("", "", None, size(40, 20)), "");
}

#[test]
fn after_a_resize_the_old_live_zone_is_measured_at_the_new_width() {
    let mut zone = LiveZone::default();
    let line = format!("{}\n", "x".repeat(30));
    zone.redraw("", &line, None, size(40, 20));
    // The terminal reflowed the 30 columns onto two rows of 20.
    let out = zone.redraw("", &line, None, size(20, 20));
    assert_eq!(readable(&out), format!("\\e[?2026h\\r\\e[2A\\e[J{line}\\e[?2026l"));
    // And back: one row at 40 again.
    let out = zone.redraw("", "", None, size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[1A\\e[J\\e[?2026l");
}

#[test]
fn the_renderers_count_is_used_at_its_own_width() {
    let mut zone = LiveZone::default();
    // A deliberately wrong count shows that it is trusted, not recounted.
    zone.redraw("", "a\n", Some(Measured { rows: 3, width: 40 }), size(40, 20));
    let out = zone.redraw("", "", None, size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[3A\\e[J\\e[?2026l");
}

#[test]
fn the_renderers_count_at_another_width_is_recounted() {
    let mut zone = LiveZone::default();
    zone.redraw("", "a\n", Some(Measured { rows: 3, width: 80 }), size(40, 20));
    let out = zone.redraw("", "", None, size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[1A\\e[J\\e[?2026l");
}

#[test]
fn a_live_zone_taller_than_the_screen_shows_its_last_lines() {
    let mut zone = LiveZone::default();
    let live: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    let out = zone.redraw("", &live, Some(Measured { rows: 10, width: 40 }), size(40, 5));
    assert_eq!(out, format!("{BEGIN}line 7\nline 8\nline 9\nline 10\n{END}"));
    // Only the four rows that were shown are erased.
    let out = zone.redraw("", "", None, size(40, 5));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[4A\\e[J\\e[?2026l");
}

#[test]
fn clipping_counts_wrapped_rows() {
    let mut zone = LiveZone::default();
    let live = format!("first\n{}\nlast\n", "x".repeat(25));
    // Four rows fit: the wrapped line takes three, and `last` one.
    let out = zone.redraw("", &live, None, size(10, 5));
    assert_eq!(out, format!("{BEGIN}{}\nlast\n{END}", "x".repeat(25)));
}

#[test]
fn a_single_line_taller_than_the_screen_is_not_shown() {
    let mut zone = LiveZone::default();
    let out = zone.redraw("ok\n", &format!("{}\n", "x".repeat(100)), None, size(10, 5));
    assert_eq!(out, format!("{BEGIN}ok\n{END}"));
}

#[test]
fn an_unknown_size_renders_at_80_columns_without_clipping() {
    let mut zone = LiveZone::default();
    let live = format!("{}\n", "x".repeat(81));
    zone.redraw("", &live, None, Size::default());
    let out = zone.redraw("", "", None, Size::default());
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
        zone.redraw("", &live, None, size(10, 20));
        let out = zone.redraw("", "next\n", None, size(10, 20));
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
    let out = zone.draw("", "body\n", None, "status\n", &input_row(), size(40, 20));
    // Up one row from below the live zone, to column 5 of the tail's second line.
    assert_eq!(readable(&out), "\\e[?2026hbody\nstatus\n> first\n  typ\n\\e[1A\\r\\e[5C\\e[?2026l");
    let mut grid = Grid::new(40);
    grid.write(&out);
    assert_eq!(grid.cursor(), (3, 5));
    // The cursor is three rows below the top of the live zone, not four.
    let out = zone.draw("done\n", "body\n", None, "status\n", &Tail::default(), size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[3A\\e[Jdone\nbody\nstatus\n\\e[?2026l");
    grid.write(&out);
    assert_eq!(grid.lines(), ["done", "body", "status"]);
    assert_eq!(grid.cursor(), (3, 0));
}

#[test]
fn a_tick_rewrites_the_status_row_above_the_tail_and_puts_the_cursor_back() {
    let mut zone = LiveZone::default();
    let mut grid = Grid::new(40);
    grid.write(&zone.draw("", "body\n", None, "status 1\n", &input_row(), size(40, 20)));
    let out = zone.draw("", "body\n", None, "status 2\n", &input_row(), size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[2A\\e[2Kstatus 2\n\\e[1B\\r\\e[5C\\e[?2026l");
    grid.write(&out);
    assert_eq!(grid.lines(), ["body", "status 2", "> first", "  typ"]);
    assert_eq!(grid.cursor(), (3, 5));
}

#[test]
fn a_cursor_that_moves_redraws_the_live_zone() {
    let mut zone = LiveZone::default();
    let mut grid = Grid::new(40);
    grid.write(&zone.draw("", "", None, "status\n", &input_row(), size(40, 20)));
    let mut moved = input_row();
    moved.cursor = Some(Cursor { line: 0, column: 2 });
    let out = zone.draw("", "", None, "status\n", &moved, size(40, 20));
    assert!(!out.is_empty());
    grid.write(&out);
    assert_eq!(grid.lines(), ["status", "> first", "  typ"]);
    assert_eq!(grid.cursor(), (1, 2));
    assert_eq!(zone.draw("", "", None, "status\n", &moved, size(40, 20)), "", "nothing new");
}

#[test]
fn a_tail_without_a_status_row_takes_the_cursor_too() {
    let mut zone = LiveZone::default();
    let mut grid = Grid::new(40);
    let tail = Tail { text: "> x\n".to_owned(), cursor: Some(Cursor { line: 0, column: 3 }) };
    grid.write(&zone.draw("", "", None, "", &tail, size(40, 20)));
    assert_eq!(grid.cursor(), (0, 3));
    // A redraw from the top row of the live zone moves up no row.
    let out = zone.draw("note\n", "", None, "", &Tail::default(), size(40, 20));
    assert_eq!(readable(&out), "\\e[?2026h\\r\\e[Jnote\n\\e[?2026l");
    grid.write(&out);
    assert_eq!(grid.lines(), ["note"]);
}

#[test]
fn a_tail_that_does_not_fit_gets_no_cursor() {
    let mut zone = LiveZone::default();
    let out = zone.draw("", "", None, "status\n", &input_row(), size(40, 2));
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
