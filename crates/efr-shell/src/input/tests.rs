use std::time::Duration;

use efr_protocol::{Cell, Cursor, InputRespond, InputWait, RowCells, ScreenSnapshot};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{InputWatch, Look, Probe, Quiet, check_answer, check_modes, look, visible_prompt};
use crate::ShellError;
use crate::modes::InputModes;

const START: Timestamp = Timestamp::constant(1_791_115_200, 0);

const QUIET: Quiet = Quiet { hidden: Duration::from_secs(1), visible: Duration::from_secs(3) };

fn at(millis: i64) -> Timestamp {
    START.checked_add(SignedDuration::from_millis(millis)).unwrap()
}

fn probe(modes: InputModes) -> Probe {
    Probe { running: true, last_output: Some(START), modes: Some(modes), answers: 0 }
}

const HIDDEN: InputModes = InputModes { echo: false, canonical: true };
const COOKED: InputModes = InputModes { echo: true, canonical: true };
const RAW: InputModes = InputModes { echo: false, canonical: false };

#[test]
fn echo_off_with_line_input_is_hidden_once_the_output_is_quiet() {
    assert_eq!(look(&probe(HIDDEN), at(999), QUIET), Look::Settled(InputWait::None));
    assert_eq!(look(&probe(HIDDEN), at(1000), QUIET), Look::Settled(InputWait::Hidden));
}

#[test]
fn a_cooked_terminal_reads_the_screen_only_after_the_visible_quiet() {
    assert_eq!(look(&probe(COOKED), at(2999), QUIET), Look::Settled(InputWait::None));
    assert_eq!(look(&probe(COOKED), at(3000), QUIET), Look::ReadScreen);
}

#[test]
fn a_raw_terminal_never_waits() {
    assert_eq!(look(&probe(RAW), at(60_000), QUIET), Look::Settled(InputWait::None));
    let echoing_raw = InputModes { echo: true, canonical: false };
    assert_eq!(look(&probe(echoing_raw), at(60_000), QUIET), Look::Settled(InputWait::None));
}

#[test]
fn nothing_waits_before_the_command_runs_or_without_modes() {
    let before = Probe { running: false, ..probe(HIDDEN) };
    assert_eq!(look(&before, at(60_000), QUIET), Look::Settled(InputWait::None));
    let unknown = Probe { modes: None, ..probe(HIDDEN) };
    assert_eq!(look(&unknown, at(60_000), QUIET), Look::Settled(InputWait::None));
}

#[test]
fn a_clock_that_went_back_is_not_quiet() {
    assert_eq!(look(&probe(HIDDEN), at(-5000), QUIET), Look::Settled(InputWait::None));
}

fn row(text: &str) -> RowCells {
    RowCells {
        cells: text.chars().map(|c| Cell { text: c.to_string(), ..Cell::default() }).collect(),
        wrapped: false,
    }
}

fn screen(rows: &[&str], cursor: (u16, u16), alternate_screen: bool) -> ScreenSnapshot {
    ScreenSnapshot {
        rows: rows.iter().map(|text| row(text)).collect(),
        cursor: Cursor { row: cursor.0, col: cursor.1, hidden: false },
        alternate_screen,
        ..ScreenSnapshot::default()
    }
}

#[test]
fn a_prompt_is_the_cursor_after_text_on_the_main_screen() {
    let asking = screen(&["Proceed? [Y/n] "], (0, 15), false);
    assert!(visible_prompt(&asking));
    let done = screen(&["done", ""], (1, 0), false);
    assert!(!visible_prompt(&done));
    let full_screen = screen(&["top - 12:00"], (0, 5), true);
    assert!(!visible_prompt(&full_screen));
}

#[test]
fn each_change_is_reported_once() {
    let mut watch = InputWatch::default();
    assert_eq!(watch.settle(InputWait::None, 0), None);
    assert_eq!(watch.settle(InputWait::Hidden, 0), Some(InputWait::Hidden));
    assert_eq!(watch.settle(InputWait::Hidden, 0), None);
    assert_eq!(watch.current(), InputWait::Hidden);
    assert_eq!(watch.settle(InputWait::Visible, 0), Some(InputWait::Visible));
    assert_eq!(watch.end(), Some(InputWait::None));
    assert_eq!(watch.end(), None);
}

#[test]
fn an_answer_makes_the_next_look_none_so_a_prompt_asked_again_is_new() {
    let mut watch = InputWatch::default();
    assert_eq!(watch.settle(InputWait::Hidden, 0), Some(InputWait::Hidden));
    // The look right after the answer still sees the old prompt.
    assert_eq!(watch.settle(InputWait::Hidden, 1), Some(InputWait::None));
    assert_eq!(watch.settle(InputWait::Hidden, 1), Some(InputWait::Hidden));
}

#[test]
fn an_answer_is_one_short_line() {
    check_answer("hunter2").unwrap();
    check_answer("").unwrap();
    check_answer("p\u{e4}ss w\u{f6}rd").unwrap();
    check_answer(&"x".repeat(InputRespond::MAX_TEXT_BYTES)).unwrap();
    for bad in ["a\rb", "a\nb", "\u{1b}[A", "tab\there", "nul\0", "del\u{7f}"] {
        let error = check_answer(bad).unwrap_err();
        assert!(matches!(error, ShellError::InvalidAnswer { .. }), "{error:?}");
        assert!(!error.to_string().contains(bad), "the reason never quotes the text");
    }
    assert!(matches!(
        check_answer(&"x".repeat(InputRespond::MAX_TEXT_BYTES + 1)),
        Err(ShellError::InvalidAnswer { .. })
    ));
}

#[test]
fn an_answer_needs_line_input_and_a_hidden_one_needs_echo_off() {
    check_modes(HIDDEN, true).unwrap();
    check_modes(HIDDEN, false).unwrap();
    check_modes(COOKED, false).unwrap();
    assert!(check_modes(COOKED, true).is_err());
    assert!(check_modes(RAW, true).is_err());
    assert!(check_modes(RAW, false).is_err());
}
