//! TM-Web: the citation marks of web search in the text of an answer. efrd writes each
//! mark into the text as ` [[n]](<url>)` after the words that it backs, before the
//! punctuation; the renderer shows `[n]` in the `link` role (underlined also without
//! colour), never with its target after it.
//!
//! NOTE: TM-Web breaks the rows of the answer at words, at the width of the frame.
//! TM-Narrow wins: efr writes a paragraph as one line and the terminal wraps it. A line
//! break of the mock at each row of the board keeps the rows of the board; the marks
//! and their roles are what these tests check.

use efr_render::render;
use efr_test_support::design::{Depth, Frame, assert_rows};

use super::options;

/// The first sentences of the answer of TM-Web 10, with the marks as efrd writes them.
const CITED: &str = "The developers allowed Linux in Easy Anti-Cheat with patch 1.4 \
[[1]](<https://areweanticheatyet.com/game/rob-run>). ProtonDB reports that it runs with \
Proton 10 and the Proton EasyAntiCheat Runtime installed [[2]](<https://www.protondb.com/app/2384710>), \
and it is Verified on Steam Deck [[3]](<https://www.steamdeck.com/verified/2384710>).";

/// The last sentence of the answer of TM-Web 10.
const REST: &str = " Your setup forces Proton 9 for it, and the runtime is not installed.";

/// The words that start a row of the answer at 100 columns (TM-Web 10 and 16).
const ROWS_AT_100: &[&str] = &["with Proton 10", "Deck [[3]]"];

/// The words that start a row of the answer at 60 columns (TM-Web 12 and 18).
const ROWS_AT_60: &[&str] = &["1.4 [[1]]", "and the Proton", "is Verified"];

/// `text` with a line break in place of the space before each of `starts`.
fn broken(text: &str, starts: &[&str]) -> String {
    starts.iter().fold(text.to_owned(), |text, start| {
        text.replacen(&format!(" {start}"), &format!("\n{start}"), 1)
    })
}

/// Checks rows `range` of frame `number` in each of `depths` against the answer `text`.
fn check(number: u32, range: std::ops::RangeInclusive<usize>, text: &str, depths: &[Depth]) {
    let frame = Frame::load("TM-Web", number).unwrap();
    for depth in depths {
        let out = render(&format!("{text}\n"), &options(*depth, frame.cols));
        assert_rows(&frame, range.clone(), &out, *depth);
    }
}

/// TM-Web 10 and its `NO_COLOR` frame 11: `[n]` in the link role after the words,
/// before the punctuation.
#[test]
fn the_marks_of_an_answer_are_the_web_board() {
    check(10, 0..=2, &broken(&format!("{CITED}{REST}"), ROWS_AT_100), &Depth::ALL);
}

/// TM-Web 12: the same answer at 60 columns.
#[test]
fn the_marks_at_sixty_columns_are_the_web_board() {
    check(12, 0..=3, &broken(CITED, ROWS_AT_60), &Depth::ALL);
}

/// TM-Web 16 to 19: the key frames in 16 colours and without colour, at 100 and 60
/// columns. Frames 18 and 19 draw the first two rows of the answer only.
#[test]
fn the_marks_in_sixteen_colours_and_without_colour_are_the_web_board() {
    let answer = broken(&format!("{CITED}{REST}"), ROWS_AT_100);
    check(16, 5..=7, &answer, &[Depth::Ansi16]);
    check(17, 5..=7, &answer, &[Depth::NoColor]);
    let start = broken(
        "The developers allowed Linux in Easy Anti-Cheat with patch 1.4 \
[[1]](<https://areweanticheatyet.com/game/rob-run>). ProtonDB reports that it runs with Proton 10",
        ROWS_AT_60,
    );
    check(18, 4..=5, &start, &[Depth::Ansi16]);
    check(19, 4..=5, &start, &[Depth::NoColor]);
}

/// TM-Narrow: the answer is one line, whatever the width: efr breaks no row of it.
#[test]
fn the_answer_is_one_line_at_any_width() {
    let answer = format!("{CITED}{REST}\n");
    for cols in [60, 100] {
        let out = render(&answer, &options(Depth::TrueColor, cols));
        assert_eq!(out.matches('\n').count(), 1, "{out:?}");
    }
}
