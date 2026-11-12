//! The input row, the rows of unread steers and queued prompts, and a steer in the
//! scrollback against the frames of the design (TM-Live 15, 17-19, 29, TM-Legend 27,
//! 28, TM-Sequence 1, 11, 12), in the three depths. The cursor of a frame is a cell in
//! reverse video; the terminal draws it, so the row has a blank there.

use efr_protocol::Seq;
use efr_test_support::design::{Depth, Frame, assert_rows};

use super::{Input, typed, user_message};
use crate::testing::design_options;

/// Checks rows `range` of frame `number` of `board` against what `draw` gives at each
/// depth.
fn check(
    board: &str,
    number: u32,
    range: std::ops::Range<usize>,
    draw: impl Fn(&efr_render::RenderOptions, u16) -> String,
) {
    let frame = Frame::load(board, number).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        assert_rows(&frame, range.clone(), &draw(&options, frame.cols), depth);
    }
}

#[test]
fn an_empty_input_row_shows_the_hint_after_the_cursor_live_15_29() {
    check("Live", 15, 1..2, |options, cols| Input::default().row(options, cols).text);
    check("Live", 29, 0..1, |options, cols| Input::default().row(options, cols).text);
}

#[test]
fn ctrl_b_leads_the_hint_while_a_shell_call_can_move_live_16() {
    // The rows above it are the running call's (`call.rs`).
    check("Live", 16, 2..3, |options, cols| {
        Input { background: true, ..Input::default() }.row(options, cols).text
    });
}

#[test]
fn typed_text_follows_the_mark_live_17_18_sequence_1() {
    check("Live", 17, 1..2, |options, cols| {
        typed("use TurnState, not a new enum").row(options, cols).text
    });
    let text = "keep the JSON reader, but:\n- behind legacy-store, off by default\n- with a test for a missing ended_at";
    check("Live", 18, 1..4, |options, cols| typed(text).row(options, cols).text);
    check("Sequence", 1, 2..3, |options, cols| typed("also add a test").row(options, cols).text);
}

/// The steers and prompts of TM-Live 19.
fn waiting() -> Input {
    let mut input = Input::default();
    input.steered(Seq::new(20), "use TurnState, not a new enum".to_owned());
    input.queued(super::turn(1), "then write the changelog entry".to_owned(), false);
    input.queued(super::turn(2), "bump efrd to 0.0.6".to_owned(), true);
    input
}

#[test]
fn unread_steers_and_queued_prompts_stand_above_the_status_row_live_19() {
    check("Live", 19, 0..3, |options, cols| waiting().pending(options, cols));
    check("Live", 19, 4..5, |options, cols| waiting().row(options, cols).text);
}

#[test]
fn an_unread_steer_is_one_muted_row_legend_28_sequence_11() {
    let steer = |text: &str| {
        let mut input = Input::default();
        input.steered(Seq::new(20), text.to_owned());
        input
    };
    check("Legend", 28, 0..1, |options, cols| steer("use TurnState").pending(options, cols));
    check("Sequence", 11, 4..5, |options, cols| {
        steer("use TurnState, not a new enum").pending(options, cols)
    });
}

#[test]
fn a_read_steer_is_the_accent_mark_and_bold_text_legend_27_sequence_12() {
    check("Legend", 27, 0..1, |options, _| user_message("use TurnState", options));
    check("Sequence", 12, 5..6, |options, _| {
        user_message("use TurnState, not a new enum", options)
    });
}

#[test]
fn the_input_row_keeps_its_hierarchy_in_every_depth_depths_3_7_11() {
    // Depths 3 is truecolor, 7 the 16 colours and 11 `NO_COLOR` of the same zone; the
    // rows above the input row are the running call's.
    check("Depths", 3, 5..6, |options, cols| Input::default().row(options, cols).text);
}
