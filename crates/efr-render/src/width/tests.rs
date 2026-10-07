use pretty_assertions::assert_eq;

use super::{WidthMethod, display_width, text_width};

const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
const FLAG: &str = "\u{1f1fa}\u{1f1f8}";
const HEART_EMOJI: &str = "\u{2764}\u{fe0f}";
const WATCH_TEXT: &str = "\u{231a}\u{fe0e}";
const THUMB_TONE: &str = "\u{1f44d}\u{1f3fd}";

fn both(text: &str) -> (usize, usize) {
    (text_width(text, WidthMethod::CodePoint), text_width(text, WidthMethod::Grapheme))
}

#[test]
fn simple_text_has_the_same_width_both_ways() {
    assert_eq!(both("plain"), (5, 5));
    assert_eq!(both("\u{65e5}\u{672c}"), (4, 4));
    assert_eq!(both("a\u{301}"), (1, 1));
    assert_eq!(both(""), (0, 0));
    assert_eq!(both("tab\there"), (7, 7));
}

#[test]
fn a_zwj_emoji_is_one_wide_cluster() {
    assert_eq!(both(FAMILY), (6, 2));
}

#[test]
fn a_flag_is_one_wide_cluster() {
    assert_eq!(both(FLAG), (2, 2));
    assert_eq!(both(&format!("{FLAG}{FLAG}")), (4, 4));
    assert_eq!(text_width("\u{1f1fa}", WidthMethod::Grapheme), 2);
}

#[test]
fn variation_selectors_change_the_width_of_a_cluster() {
    assert_eq!(both(HEART_EMOJI), (1, 2));
    assert_eq!(both(WATCH_TEXT), (2, 1));
    assert_eq!(both(THUMB_TONE), (4, 2));
}

#[test]
fn display_width_skips_escape_sequences() {
    for method in [WidthMethod::CodePoint, WidthMethod::Grapheme] {
        assert_eq!(display_width("\x1b[1;31mred\x1b[0m", method), 3);
        assert_eq!(display_width("\x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\", method), 4);
        assert_eq!(display_width("\x1b]8;;https://x\x07link", method), 4);
        assert_eq!(display_width("wide \u{4e16}\u{754c}", method), 9);
    }
}

#[test]
fn a_cluster_continues_across_a_colour_change() {
    let painted = "\x1b[1m\u{1f468}\x1b[0m\u{200d}\u{1f469}";
    assert_eq!(display_width(painted, WidthMethod::CodePoint), 4);
    assert_eq!(display_width(painted, WidthMethod::Grapheme), 2);
}

#[test]
fn code_point_is_the_default() {
    assert_eq!(WidthMethod::default(), WidthMethod::CodePoint);
}
