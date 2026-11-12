//! The question card against the frames of the design (TM-Questions, TM-Live 20,
//! TM-Legend 23-25, TM-Sequence 9, TM-Depths 2, 6, 10), in each depth that a board shows:
//! the text, the role and the attributes of each cell. The empty live row above a card
//! is the view's (`follow/view/tests/always.rs`); these tests compare the card's own
//! rows.

use efr_test_support::design::{Depth, Frame, assert_rows, cells};

use super::super::{ALLOW_KEYS, Card, Footer, Row};
use crate::format::Tone;
use crate::testing::design_options;

/// The rule that the cards of the boards offer for `a`.
const RULE: &str = "network index.crates.io for cargo";

/// Checks rows `range` of frame `number` of `board` against `card` with `footer`, in
/// every depth (the board's own frame of a depth, else the one that the table derives).
fn check(board: &str, number: u32, range: std::ops::Range<usize>, card: &Card, footer: &Footer) {
    let frame = Frame::load(board, number).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        assert_rows(&frame, range.clone(), &card.render(Some(footer), &options), depth);
    }
}

/// The card of TM-Questions 1: a command outside the sandbox. NOTE: the board cuts the
/// command after `build` at 100 columns, where it fits; the test gives the command as the
/// two lines that the board's rows show, joined by the shell's own `\`, whose mark is the
/// muted one of a cut (the board's caption: "the line wraps as in a call, with \").
fn outside() -> Card {
    Card {
        title: "allow outside the sandbox".to_owned(),
        rows: vec![
            Row::Command("cd ~/p/app && find target/debug/build \\".to_owned()),
            Row::Command("    -path '*ghostty*' -type f | awk '{print $1}'".to_owned()),
            Row::text("runs with your full rights: files, secrets, network", Tone::Attention),
            Row::labelled("programs", "cd, find, awk"),
            Row::labelled("the model says", "\"find where the ghostty build put libghostty-vt\""),
        ],
    }
}

#[test]
fn a_command_outside_the_sandbox_is_questions_1_9_10() {
    // 1 is truecolor, 9 the 16 colours and 10 `NO_COLOR` of the same card.
    check("Questions", 1, 2..9, &outside(), &Footer::Keys(ALLOW_KEYS));
}

/// TM-Questions 11 with the command as the model wrote it, on one line: the card cuts it
/// itself, after the pipe, so `awk` starts a row.
// NOTE: the board's caption says 60 columns, but its rows are cut as a screen of 44
// columns cuts them (each row fits in 60, and the risk row is cut at 38); the test draws
// the card at 44 columns.
#[test]
fn a_long_command_is_cut_after_its_pipe_questions_11() {
    let mut rows = super::super::command_rows(
        "cd ~/p/app && find target/debug/build -path '*ghostty*' -type f | awk '{print $1}'",
    );
    rows.push(Row::text("runs with your full rights: files, secrets, network", Tone::Attention));
    rows.push(Row::labelled("programs", "cd, find, awk"));
    let card = Card { title: "allow outside the sandbox".to_owned(), rows };
    let frame = Frame::load("Questions", 11).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, 44);
        let real = card.render(Some(&Footer::Keys(ALLOW_KEYS)), &options);
        assert_rows(&frame, 2..10, &real, depth);
    }
}

#[test]
fn at_a_narrow_screen_a_text_row_goes_on_two_columns_further_in_questions_11() {
    // NOTE: the board draws its "60 columns" frame with rows that fit in 60 columns cut
    // as a screen of 44 columns cuts them; the test draws the card at 44 columns, where
    // the text row goes on two columns further in, as the board's note says. The board
    // leaves out the row of what the model says.
    let card = Card {
        title: "allow outside the sandbox".to_owned(),
        rows: vec![
            Row::Command("cd ~/p/app && find target/debug/build \\".to_owned()),
            Row::Command("    -path '*ghostty*' -type f | \\".to_owned()),
            Row::Command("    awk '{print $1}'".to_owned()),
            Row::text("runs with your full rights: files, secrets, network", Tone::Attention),
            Row::labelled("programs", "cd, find, awk"),
        ],
    };
    let frame = Frame::load("Questions", 11).unwrap();
    let options = design_options(Depth::TrueColor, 44);
    let real = card.render(Some(&Footer::Keys(ALLOW_KEYS)), &options);
    assert_rows(&frame, 2..10, &real, Depth::TrueColor);
}

#[test]
fn a_write_shows_its_diff_at_the_cards_indent_questions_2() {
    let diff = "--- ~/.config/hypr/hyprland.conf\n+++ ~/.config/hypr/hyprland.conf\n\
                @@ -41 +41 @@ input\n-kb_options = caps:escape\n\
                +kb_options = caps:escape,compose:ralt\n";
    let card = Card {
        title: "allow this write".to_owned(),
        rows: vec![Row::Diff(diff.to_owned()), Row::labelled("why", "write outside the project")],
    };
    let footer = Footer::Always(RULE.to_owned());
    let frame = Frame::load("Questions", 2).unwrap();
    let options = design_options(Depth::TrueColor, frame.cols);
    let real = card.render(Some(&footer), &options);
    let rows: Vec<&str> = real.split_inclusive('\n').collect();
    // The title, the row of the file, the hunk header, then after the diff, why and the
    // keys: cell by cell.
    assert_rows(&frame, 2..5, &rows[..3].concat(), Depth::TrueColor);
    assert_rows(&frame, 7..9, &rows[5..].concat(), Depth::TrueColor);
    // NOTE: the colours inside a diff row are the render slice's (`diff_lines`); here the
    // text, the sign and the background of each row.
    let lines = cells(&rows[3..5].concat());
    for (at, line) in lines.iter().enumerate() {
        let want = &frame.rows[5 + at];
        assert_eq!(line.text().trim_end(), want.text.trim_end(), "row {at}");
        assert_eq!(line.fill, want.fill, "the background of row {at}");
    }
}

#[test]
fn a_network_question_and_its_confirm_are_questions_3_4() {
    let card = Card {
        title: "allow network".to_owned(),
        rows: vec![
            Row::Command("cargo update -p rusqlite".to_owned()),
            Row::Text(vec![
                ("host".to_owned(), Tone::Dim),
                ("  ".to_owned(), Tone::Plain),
                ("index.crates.io:443 ".to_owned(), Tone::Text),
                ("\u{b7} not on the allow list".to_owned(), Tone::Dim),
            ]),
        ],
    };
    check("Questions", 3, 2..6, &card, &Footer::Always(RULE.to_owned()));
    check("Questions", 4, 2..7, &card, &Footer::Confirm(RULE.to_owned()));
}

#[test]
fn full_mode_still_asks_with_its_reason_and_expiry_in_warning_questions_5() {
    let card = Card {
        title: "allow this command".to_owned(),
        rows: vec![
            super::super::guard_row(
                "full mode still asks for this: delete of ~ (your home directory)",
            ),
            Row::Command("rm -rf ~/tmp-build ~/".to_owned()),
            super::super::expires_row(120),
        ],
    };
    check("Questions", 5, 2..7, &card, &Footer::Keys(ALLOW_KEYS));
}

#[test]
fn another_client_answers_questions_6() {
    // NOTE: the board ends the row with ` · the phone has it too`; no event tells efr
    // that a phone got the question, so the row says only what efr knows. The test
    // compares the rows before it.
    let card = Card {
        title: "allow this command".to_owned(),
        rows: vec![
            Row::Command("git push -u origin fix/steer-race".to_owned()),
            Row::labelled("why", "network (github.com)"),
        ],
    };
    let frame = Frame::load("Questions", 6).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let real = card.render(Some(&Footer::Waiting), &options);
        let rows: Vec<&str> = real.split_inclusive('\n').collect();
        assert_rows(&frame, 2..5, &rows[..3].concat(), depth);
        assert!(rows[3].contains("waiting for another client to answer"), "{real}");
    }
}

/// The card of TM-Live 20 and TM-Sequence 9, without and with what the model says.
fn push(model_says: bool) -> Card {
    let mut rows = vec![
        Row::Command("git push -u origin fix/steer-race".to_owned()),
        Row::labelled("why", "network (github.com)"),
    ];
    if model_says {
        rows.push(Row::labelled("the model says", "\"push so CI runs on the fix\""));
    }
    Card { title: "allow this command".to_owned(), rows }
}

#[test]
fn a_question_replaces_the_live_zone_live_20_sequence_9_depths_2_6_10() {
    let always = Footer::Always(RULE.to_owned());
    check("Live", 20, 0..4, &push(false), &always);
    check("Sequence", 9, 4..9, &push(true), &always);
    // Depths 2 (truecolor), 6 (16 colours) and 10 (`NO_COLOR`) draw the same card.
    check("Depths", 2, 4..8, &push(false), &always);
}

#[test]
fn the_badge_the_label_row_and_the_keys_are_the_legend_23_24_25() {
    let frame = Frame::load("Legend", 23).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let card = Card { title: "allow this command".to_owned(), rows: Vec::new() };
        assert_rows(&frame, .., &card.render(None, &options), depth);
        let why =
            Card { title: String::new(), rows: vec![Row::labelled("why", "network (github.com)")] };
        let rows = why.render(None, &options);
        let row = rows.split_inclusive('\n').nth(1).unwrap();
        assert_rows(&Frame::load("Legend", 24).unwrap(), .., row, depth);
        let keys = super::super::footer_row(&Footer::Keys(ALLOW_KEYS), &options);
        assert_rows(&Frame::load("Legend", 25).unwrap(), .., &keys, depth);
    }
}
