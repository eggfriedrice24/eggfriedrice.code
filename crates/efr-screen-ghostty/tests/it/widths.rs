//! The widths that `efr` counts in Ghostty (`efr_render::WidthMethod::Grapheme`) against
//! libghostty-vt: when the two disagree, a redraw of `efr`'s live zone moves the cursor
//! up one row too many or too few. Ghostty turns mode 2027 (grapheme clustering) on by
//! default (`grapheme-width-method = unicode`), so the screen here turns it on as well.

use efr_render::{WidthMethod, text_width};
use efr_screen::{Screen as _, ScreenSink, Size};
use efr_screen_ghostty::{GhosttyConfig, GhosttyScreen};
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

/// A screen `cols` wide that clusters graphemes, as Ghostty does by default.
fn ghostty(cols: u16) -> GhosttyScreen {
    let mut screen = GhosttyScreen::new(Size { cols, rows: 10 }, &GhosttyConfig::default())
        .expect("libghostty-vt creates a terminal");
    screen.feed(b"\x1b[?2027h", &mut Quiet);
    screen
}

#[test]
fn the_width_of_each_cluster_is_the_one_ghostty_gives_it() {
    for sample in SAMPLES {
        let mut screen = ghostty(80);
        screen.feed(sample.as_bytes(), &mut Quiet);
        let counted = text_width(sample, WidthMethod::Grapheme);
        assert_eq!(usize::from(screen.cursor().col), counted, "{sample:?}");
    }
}

#[test]
fn a_line_of_clusters_wraps_onto_the_rows_that_efr_counts() {
    for sample in SAMPLES {
        let line = sample.repeat(7);
        let mut screen = ghostty(10);
        screen.feed(line.as_bytes(), &mut Quiet);
        screen.feed(b"\r\n", &mut Quiet);
        let width = text_width(&line, WidthMethod::Grapheme);
        let rows = width.div_ceil(10).max(1);
        assert_eq!(usize::from(screen.cursor().row), rows, "{sample:?} is {width} columns");
    }
}
