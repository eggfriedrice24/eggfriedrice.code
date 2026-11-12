//! The thought row against the frames of the design: TM-LongTurn 3 (a model that writes
//! titles) and 4 (a model that writes none), in the three depths.

use std::time::Duration;

use efr_test_support::design::{Depth, Frame, assert_rows};

use super::Phase;
use crate::testing::design_options;

/// The row of a phase with `title`, or with the reasoning `text` and no title, that
/// took `secs` seconds.
fn row(title: Option<&str>, text: &str, secs: u64, depth: Depth, cols: u16) -> String {
    let phase =
        Phase { title: title.map(str::to_owned), head: text.to_owned(), ..Phase::default() };
    phase.row(Duration::from_secs(secs), &design_options(depth, cols))
}

#[test]
fn a_title_is_bold_after_the_accent_mark_longturn_3() {
    let frame = Frame::load("LongTurn", 3).unwrap();
    let rows = [
        (2, "reading the store", 6),
        (7, "adding the sqlite store", 11),
        (32, "fixing the failing import", 4),
        (43, "checking the old data", 7),
        (57, "finishing up", 3),
    ];
    for depth in Depth::ALL {
        for (at, title, secs) in rows {
            let real = row(Some(title), "", secs, depth, frame.cols);
            assert_rows(&frame, at..=at, &real, depth);
        }
    }
}

#[test]
fn the_first_words_are_italic_and_cut_inside_a_word_longturn_4() {
    let frame = Frame::load("LongTurn", 4).unwrap();
    let rows = [
        (
            1,
            "The user wants sessions in SQLite. First I need to see how the JSON store reads them.",
            6,
        ),
        (
            6,
            "Plan: a SqliteStore behind the same trait, the JSON reader stays for old sessions.",
            11,
        ),
        (31, "The test panics at line 88: ended_at is None for sessions that never ended.", 4),
        (42, "Before touching real data I should run the importer as a dry run first.", 7),
        (56, "", 3),
    ];
    for depth in Depth::ALL {
        for (at, text, secs) in rows {
            let real = row(None, text, secs, depth, frame.cols);
            assert_rows(&frame, at..=at, &real, depth);
        }
    }
}
