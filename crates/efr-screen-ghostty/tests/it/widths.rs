//! The widths that `efr` counts in Ghostty (`efr_render::WidthMethod::Grapheme`) against
//! libghostty-vt: when the two disagree, a redraw of `efr`'s live zone moves the cursor
//! up one row too many or too few. Ghostty turns mode 2027 (grapheme clustering) on by
//! default (`grapheme-width-method = unicode`), so the screen here turns it on as well.

use efr_render::{WidthMethod, text_width};
use efr_screen::{Cursor, Screen as _, ScreenSink, Seq, ShellMarkScanner, Size};
use efr_screen_ghostty::GhosttyScreen;
use pretty_assertions::assert_eq;

/// A sink for a screen whose replies and notices nobody reads.
struct Quiet;

impl ScreenSink for Quiet {
    fn pty_reply(&mut self, _: &[u8]) {}
    fn bell(&mut self) {}
    fn title_changed(&mut self, _: &str) {}
}

/// Text whose width depends on how it is counted: a ZWJ emoji, a flag, variation
/// selectors 16 and 15, a skin tone, a combining mark, and wide CJK.
const SAMPLES: &[&str] = &[
    "plain",
    "\u{65e5}\u{672c}",
    "a\u{301}",
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
    "\u{1f1fa}\u{1f1f8}",
    "\u{2764}\u{fe0f}",
    "\u{231a}\u{fe0e}",
    "\u{1f44d}\u{1f3fd}",
];

/// The cursor of a new screen `cols` wide after `text`, with mode 2027 on as Ghostty
/// has it by default, or off. The bytes go in as the actor feeds them: with the marks
/// of one scan.
fn cursor_after(cols: u16, clustering: bool, text: &str) -> Cursor {
    let mut screen =
        GhosttyScreen::new(Size { cols, rows: 10 }).expect("libghostty-vt creates a terminal");
    let mode: &[u8] = if clustering { b"\x1b[?2027h" } else { b"\x1b[?2027l" };
    let bytes = [mode, text.as_bytes()].concat();
    let marks = ShellMarkScanner::new().scan(&bytes, Seq::ZERO);
    screen.feed_marked(&bytes, Seq::ZERO, &marks, &mut Quiet);
    screen.snapshot(0).cursor
}

#[test]
fn the_width_of_each_cluster_is_the_one_ghostty_gives_it() {
    for sample in SAMPLES {
        let counted = text_width(sample, WidthMethod::Grapheme);
        assert_eq!(usize::from(cursor_after(80, true, sample).col), counted, "{sample:?}");
    }
}

/// The glyphs of efr's terminal look (`efr_render::Glyphs`) and the marks of the zsh
/// plugin take one column each in Ghostty, with and without mode 2027, as zle counts
/// them with wcwidth: `⌂` (home), `›` (the marker of sticky mode and of a steer), `·`,
/// `↪`, `⇄`, `▸`, `■`, `↳`, `↩`, `✓`, `✗`, `→`, `…`, `─`, the spinner and `•`. Each has
/// the Unicode East Asian width "neutral" or "ambiguous"; Ghostty has no setting that
/// draws an ambiguous character wide, and its terminal never reads the locale, so a CJK
/// locale does not move zle's cursor away from the terminal's. The robot of older
/// versions stays in the list: a line of history may still start with it, and it is
/// two columns wide.
#[test]
fn the_glyphs_of_the_look_take_one_column() {
    let glyphs = efr_render::Glyphs::UNICODE;
    let mut one: Vec<&str> = vec![
        "\u{2302}", "\u{203a}", "\u{b7}", "\u{21aa}", "\u{21c4}", "\u{25b8}", "\u{25a0}",
        "\u{21b3}", "\u{21a9}", "\u{2713}", "\u{2717}", "\u{2192}", "\u{2026}", "\u{2500}",
        "\u{2022}",
    ];
    for (name, glyph) in glyphs.all() {
        if glyph.chars().count() == 1 && !one.contains(&glyph) {
            one.push(glyph);
        }
        assert!(text_width(glyph, WidthMethod::Grapheme) >= 1, "{name} {glyph:?}");
    }
    one.extend(glyphs.spinner.iter().copied());
    for glyph in one {
        for clustering in [true, false] {
            let col = cursor_after(80, clustering, glyph).col;
            assert_eq!(col, 1, "{glyph:?}, mode 2027 on: {clustering}");
        }
        assert_eq!(text_width(glyph, WidthMethod::Grapheme), 1, "{glyph:?}");
    }
    for clustering in [true, false] {
        let col = cursor_after(80, clustering, "\u{1f916} ").col;
        assert_eq!(col, 3, "mode 2027 on: {clustering}");
    }
}

#[test]
fn a_line_of_clusters_wraps_onto_the_rows_that_efr_counts() {
    for sample in SAMPLES {
        let line = sample.repeat(7);
        let width = text_width(&line, WidthMethod::Grapheme);
        let rows = width.div_ceil(10).max(1);
        let row = cursor_after(10, true, &format!("{line}\r\n")).row;
        assert_eq!(usize::from(row), rows, "{sample:?} is {width} columns");
    }
}
