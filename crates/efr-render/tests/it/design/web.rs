//! TM-Web: the citation marks of web search in the text of an answer. efrd writes each
//! mark into the text as ` [[n]](<url>)` after the words that it backs, before the
//! punctuation; the renderer shows `[n]` in the `link` role (underlined also without
//! colour), never with its target after it, and keeps it with the word before it when
//! a row breaks.

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

/// Checks rows `range` of frame `number` in each of `depths` against the answer `text`.
fn check(number: u32, range: std::ops::RangeInclusive<usize>, text: &str, depths: &[Depth]) {
    let frame = Frame::load("TM-Web", number).unwrap();
    for depth in depths {
        let out = render(&format!("{text}\n"), &options(*depth, frame.cols));
        assert_rows(&frame, range.clone(), &out, *depth);
    }
}

/// TM-Web 10 and its `NO_COLOR` frame 11: `[n]` in the link role after the words, before
/// the punctuation, and `Deck [3].` on one row.
#[test]
fn the_marks_of_an_answer_are_the_web_board() {
    check(10, 0..=2, &format!("{CITED}{REST}"), &Depth::ALL);
}

/// TM-Web 12: the same answer at 60 columns. NOTE: the board breaks its row 1 one word
/// before the width (`... with Proton 10`, 53 columns, though ` and` fits), as the
/// browser of the board laid it out; efr's word wrap of `main` fills each row, a point
/// that is not web search (DECISIONS-web 4b). A line break after `Proton 10` in the
/// mock keeps the rows of the board; the marks, their roles and `Deck [3].` are what
/// this test checks.
#[test]
fn the_marks_at_sixty_columns_are_the_web_board() {
    let broken = CITED.replacen("Proton 10 and", "Proton 10\\\nand", 1);
    check(12, 0..=3, &broken, &Depth::ALL);
}

/// TM-Web 16 to 19: the key frames in 16 colours and without colour, at 100 and 60
/// columns. Frames 18 and 19 draw the first two rows of the answer only.
#[test]
fn the_marks_in_sixteen_colours_and_without_colour_are_the_web_board() {
    let answer = format!("{CITED}{REST}");
    check(16, 5..=7, &answer, &[Depth::Ansi16]);
    check(17, 5..=7, &answer, &[Depth::NoColor]);
    let start = "The developers allowed Linux in Easy Anti-Cheat with patch 1.4 \
[[1]](<https://areweanticheatyet.com/game/rob-run>). ProtonDB reports that it runs with Proton 10";
    check(18, 4..=5, start, &[Depth::Ansi16]);
    check(19, 4..=5, start, &[Depth::NoColor]);
}
