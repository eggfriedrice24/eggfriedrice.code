use efr_render::{WidthMethod, display_rows, last_rows};
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, prop_oneof, proptest};
use proptest::strategy::{Just, Strategy};

use super::{Pen, RowStart, Spot, layout, relayout};

const CP: WidthMethod = WidthMethod::CodePoint;
const GR: WidthMethod = WidthMethod::Grapheme;

/// The text of a line from row `row` on, with what is open there first: what
/// `efr_render::last_rows` gives for the rows from there.
fn from_row(line: &str, start: &RowStart) -> String {
    let mut out = String::new();
    start.pen.push_to(&mut out);
    out.push_str(&line[start.at..]);
    out
}

#[test]
fn a_line_that_fits_is_one_row_with_its_end() {
    let line = layout("abc", 10, CP);
    assert_eq!(line.rows(), 1);
    assert_eq!(line.end, 3);
    assert_eq!(layout("", 10, CP).end, 0);
}

#[test]
fn a_line_as_wide_as_the_screen_is_one_full_row() {
    let line = layout(&"x".repeat(10), 10, CP);
    assert_eq!((line.rows(), line.end), (1, 10));
    let line = layout(&"x".repeat(11), 10, CP);
    assert_eq!((line.rows(), line.end), (2, 1));
    assert_eq!(line.starts[1].at, 10);
}

#[test]
fn a_wide_character_that_does_not_fit_starts_the_next_row() {
    // Three columns: `ab` and a wide character do not fit in one row.
    let line = layout("ab\u{65e5}", 3, CP);
    assert_eq!(line.rows(), 2);
    assert_eq!(line.starts[1].at, 2);
    assert_eq!(line.end, 2);
}

#[test]
fn a_row_start_keeps_the_colours_and_the_link_that_are_open() {
    let line = "\x1b[1maaaa\x1b[0m\x1b[33m\x1b]8;;https://e.io\x1b\\bbbb\x1b]8;;\x1b\\\x1b[0m";
    let laid = layout(line, 3, CP);
    assert_eq!(laid.rows(), 3);
    assert_eq!(from_row(line, &laid.starts[1]), last_rows(line, 3, 2, CP));
    assert_eq!(from_row(line, &laid.starts[2]), last_rows(line, 3, 1, CP));
}

#[test]
fn the_spot_of_a_longer_line_is_its_old_last_cell() {
    let old = layout("hello", 10, CP);
    let (new, spot) = relayout("hello world", &old, 5, 10, CP);
    assert_eq!(new, layout("hello world", 10, CP));
    // The last old cell is written again: a next character could have joined it.
    assert_eq!(spot, Spot { row: 0, col: 4, at: 4, pen: Pen::default() });
}

#[test]
fn the_spot_of_a_wide_character_that_wraps_is_at_the_end_of_the_row_before() {
    let old = layout("ab", 3, CP);
    let (_, spot) = relayout("ab\u{65e5}", &old, 2, 3, CP);
    assert_eq!((spot.row, spot.col), (0, 1));
    let old = layout("abc", 3, CP);
    let (new, spot) = relayout("abc\u{65e5}", &old, 3, 3, CP);
    assert_eq!(new.rows(), 2);
    assert_eq!((spot.row, spot.col, spot.at), (0, 2, 2));
}

#[test]
fn a_change_past_the_last_cell_writes_the_last_cell_again() {
    // In the old line a mark of no width could have stood after the escape sequence
    // and joined `b`.
    let old = layout("ab\x1b[0m", 10, CP);
    let (_, spot) = relayout("ab\x1b[1m", &old, 4, 10, CP);
    assert_eq!((spot.row, spot.col, spot.at), (0, 1, 1));
    assert_eq!(spot.pen, Pen::default());
}

#[test]
fn a_mark_after_an_escape_sequence_that_the_line_loses_is_written_over() {
    // The accent joined `o` across the end of the bold span.
    let old = layout("\x1b[1mbo\x1b[0m\u{301} x", 40, CP);
    let (_, spot) = relayout("\x1b[1mbo\x1b[0m x", &old, 10, 40, CP);
    assert_eq!((spot.row, spot.col, spot.at), (0, 1, 5));
    let mut bold = String::new();
    spot.pen.push_to(&mut bold);
    assert_eq!(bold, "\x1b[1m");
}

#[test]
fn a_cluster_that_grows_is_written_again_whole() {
    // A man, then a zero width joiner and a woman: one cluster in Ghostty.
    let old = layout("a\u{1f468}", 10, GR);
    let (new, spot) = relayout("a\u{1f468}\u{200d}\u{1f469}", &old, 5, 10, GR);
    assert_eq!(new, layout("a\u{1f468}\u{200d}\u{1f469}", 10, GR));
    assert_eq!((spot.col, spot.at), (1, 1));
}

/// A piece of a painted line: words, wide characters, clusters and styles.
fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{1,9}".prop_map(|word| word),
        Just(" ".to_owned()),
        Just("\u{65e5}\u{672c}".to_owned()),
        Just("e\u{301}".to_owned()),
        Just("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}".to_owned()),
        Just("\u{1f1fa}\u{1f1f8}".to_owned()),
        Just("\u{2764}\u{fe0f}".to_owned()),
        Just("\x1b[1m".to_owned()),
        Just("\x1b[0m".to_owned()),
        Just("\x1b[38;2;1;2;3m".to_owned()),
        Just("\x1b]8;;https://e.io\x1b\\".to_owned()),
        Just("\x1b]8;;\x1b\\".to_owned()),
    ]
}

fn method() -> impl Strategy<Value = WidthMethod> {
    prop_oneof![Just(CP), Just(GR)]
}

proptest! {
    /// The rows and their starts agree with what `efr_render` counts and cuts.
    #[test]
    fn rows_and_starts_agree_with_the_renderer(
        pieces in prop::collection::vec(piece(), 0..40),
        width in 1_usize..30,
        method in method(),
    ) {
        let line = pieces.concat();
        let laid = layout(&line, width, method);
        assert_eq!(laid.rows(), display_rows(&line, width, method), "{line:?}");
        for (row, start) in laid.starts.iter().enumerate() {
            let rows = laid.rows() - row;
            assert_eq!(from_row(&line, start), last_rows(&line, width, rows, method), "{line:?}");
        }
    }

    /// A layout made again from an older line equals one made from nothing, and the
    /// spot stands where the walk of the whole line puts that cell.
    #[test]
    fn a_layout_made_again_equals_a_new_one(
        common in prop::collection::vec(piece(), 0..30),
        old_end in prop::collection::vec(piece(), 0..6),
        new_end in prop::collection::vec(piece(), 0..6),
        width in 1_usize..30,
        method in method(),
    ) {
        let old = format!("{}{}", common.concat(), old_end.concat());
        let new = format!("{}{}", common.concat(), new_end.concat());
        let same = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        let (laid, spot) = relayout(&new, &layout(&old, width, method), same, width, method);
        assert_eq!(laid, layout(&new, width, method), "{old:?} -> {new:?}");
        // What a write from the spot puts on the screen is the rest of the line.
        let start = RowStart { at: spot.at, pen: spot.pen.clone() };
        let rest = from_row(&new, &start);
        let before = layout(&new[..spot.at], width, method);
        assert!(spot.row + 1 >= before.rows(), "{old:?} -> {new:?}");
        assert!(rest.ends_with(&new[spot.at..]));
    }
}
