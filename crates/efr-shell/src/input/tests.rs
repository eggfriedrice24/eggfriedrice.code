use std::time::Duration;

use efr_protocol::{Cell, Cursor, InputRespond, InputWait, RowCells, ScreenSnapshot};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{
    InputWatch, Look, Offer, Probe, Quiet, Waiting, check_answer, check_job, check_kind,
    check_modes, look, question_prompt, secret_prompt, visible_prompt,
};
use crate::modes::{InputModes, Job};
use crate::{RunMode, ShellError};

const START: Timestamp = Timestamp::constant(1_791_115_200, 0);

const QUIET: Quiet = Quiet {
    hidden: Duration::from_secs(1),
    visible: Duration::from_secs(3),
    question: Duration::from_millis(500),
};

fn at(millis: i64) -> Timestamp {
    START.checked_add(SignedDuration::from_millis(millis)).unwrap()
}

/// The process groups of two jobs, one after the other.
const JOB: u32 = 4242;
const OTHER_JOB: u32 = 4343;

fn probe(modes: InputModes) -> Probe {
    Probe {
        running: true,
        last_output: Some(START),
        job: Some(Job { group: JOB, modes }),
        answers: 0,
    }
}

const HIDDEN: InputModes = InputModes { echo: false, canonical: true };
const COOKED: InputModes = InputModes { echo: true, canonical: true };
const RAW: InputModes = InputModes { echo: false, canonical: false };
const ECHOING_RAW: InputModes = InputModes { echo: true, canonical: false };

#[test]
fn echo_off_with_line_input_is_hidden_once_the_output_is_quiet() {
    assert_eq!(look(&probe(HIDDEN), at(999), QUIET, Offer::All), Look::Settled(InputWait::None));
    assert_eq!(look(&probe(HIDDEN), at(1000), QUIET, Offer::All), Look::Settled(InputWait::Hidden));
}

const ANY_PROMPT: Look = Look::ReadScreen { questions_only: false };
const QUESTIONS: Look = Look::ReadScreen { questions_only: true };

#[test]
fn a_cooked_terminal_reads_the_screen_for_a_question_first_and_any_prompt_later() {
    assert_eq!(look(&probe(COOKED), at(499), QUIET, Offer::All), Look::Settled(InputWait::None));
    assert_eq!(look(&probe(COOKED), at(500), QUIET, Offer::All), QUESTIONS);
    assert_eq!(look(&probe(COOKED), at(2999), QUIET, Offer::All), QUESTIONS);
    assert_eq!(look(&probe(COOKED), at(3000), QUIET, Offer::All), ANY_PROMPT);
}

#[test]
fn a_question_quiet_longer_than_the_visible_one_changes_nothing() {
    let quiet = Quiet { question: Duration::from_secs(10), ..QUIET };
    assert_eq!(look(&probe(COOKED), at(2999), quiet, Offer::All), Look::Settled(InputWait::None));
    assert_eq!(look(&probe(COOKED), at(3000), quiet, Offer::All), ANY_PROMPT);
}

#[test]
fn a_raw_terminal_reads_the_screen_after_the_visible_quiet_too() {
    // A relay such as sudo's own terminal leaves the hidden shell's terminal raw while
    // the program behind it asks a question.
    for modes in [RAW, ECHOING_RAW] {
        assert_eq!(look(&probe(modes), at(499), QUIET, Offer::All), Look::Settled(InputWait::None));
        assert_eq!(look(&probe(modes), at(500), QUIET, Offer::All), QUESTIONS);
        assert_eq!(look(&probe(modes), at(3000), QUIET, Offer::All), ANY_PROMPT);
    }
}

#[test]
fn a_run_that_leaves_a_prompt_for_command_lines_reports_hidden_waits_only() {
    assert_eq!(
        look(&probe(HIDDEN), at(1000), QUIET, Offer::Hidden),
        Look::Settled(InputWait::Hidden)
    );
    for modes in [COOKED, RAW, ECHOING_RAW] {
        assert_eq!(
            look(&probe(modes), at(60_000), QUIET, Offer::Hidden),
            Look::Settled(InputWait::None)
        );
    }
}

#[test]
fn what_a_run_offers_follows_its_mode_and_its_command_line() {
    assert_eq!(Offer::of(RunMode::Auto, "sudo pacman -Syu"), Offer::All);
    assert_eq!(Offer::of(RunMode::Auto, "sudo -i"), Offer::Hidden);
    assert_eq!(Offer::of(RunMode::Auto, "script -q -c sh /dev/null"), Offer::Hidden);
    assert_eq!(Offer::of(RunMode::Auto, "python3"), Offer::Hidden);
    assert_eq!(Offer::of(RunMode::Sentinel, "pacman -Syu"), Offer::Hidden);
}

#[test]
fn nothing_waits_before_the_command_runs_or_without_modes() {
    let before = Probe { running: false, ..probe(HIDDEN) };
    assert_eq!(look(&before, at(60_000), QUIET, Offer::All), Look::Settled(InputWait::None));
    let unknown = Probe { job: None, ..probe(HIDDEN) };
    assert_eq!(look(&unknown, at(60_000), QUIET, Offer::All), Look::Settled(InputWait::None));
}

#[test]
fn a_clock_that_went_back_is_not_quiet() {
    assert_eq!(look(&probe(HIDDEN), at(-5000), QUIET, Offer::All), Look::Settled(InputWait::None));
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
fn a_row_that_asks_yes_or_no_or_ends_in_a_question_mark_or_a_colon_is_a_question() {
    for (text, col) in [
        ("Proceed with installation? [Y/n] ", 33),
        (":: Proceed with installation? [Y/n]", 36),
        ("Remove the file [y/N] ", 22),
        ("Continue (yes/no) ", 18),
        ("Are you sure you want to continue connecting (yes/no/[fingerprint])? ", 70),
        ("Overwrite? [YES/NO] ", 20),
        ("Delete it (Y/n)", 15),
        ("name? ", 6),
        ("Enter a name: ", 14),
        ("[sudo] password for u: ", 23),
    ] {
        let asking = screen(&["earlier output", text], (1, col), false);
        assert!(question_prompt(&asking), "{text:?} with the cursor at {col}");
    }
}

#[test]
fn an_unfinished_line_or_text_after_the_cursor_is_no_question() {
    for (text, col) in [
        ("Downloading core.db 45%", 23),
        ("Building", 8),
        // The cursor right after the mark: a line still being written.
        ("Status:", 7),
        ("What?", 5),
        ("> ", 2),
        ("", 0),
        // The question mark is not at the cursor.
        ("Proceed? [Y/n] yes, going on", 15),
        ("Is it done? not yet", 12),
    ] {
        let row = screen(&[text], (0, col), false);
        assert!(!question_prompt(&row), "{text:?} with the cursor at {col}");
    }
}

#[test]
fn a_cursor_past_the_cells_of_its_row_stands_after_blanks() {
    let mut asking = screen(&["Continue?"], (0, 12), false);
    assert!(question_prompt(&asking));
    asking.cursor.row = 3;
    assert!(!question_prompt(&asking), "a row that is not on the screen asks nothing");
}

#[test]
fn each_change_is_reported_once() {
    let mut watch = InputWatch::default();
    assert_eq!(watch.settle(InputWait::None, false, Some(JOB), 0), None);
    assert_eq!(watch.settle(InputWait::Hidden, false, Some(JOB), 0), Some(InputWait::Hidden));
    assert_eq!(watch.settle(InputWait::Hidden, false, Some(JOB), 0), None);
    assert_eq!(watch.current(), InputWait::Hidden);
    assert_eq!(watch.waiting(), Some(Waiting { group: JOB, hidden: true }));
    assert_eq!(watch.settle(InputWait::Visible, false, Some(JOB), 0), Some(InputWait::Visible));
    assert_eq!(watch.waiting(), Some(Waiting { group: JOB, hidden: false }));
    assert_eq!(watch.end(), Some(InputWait::None));
    assert_eq!(watch.waiting(), None);
    assert_eq!(watch.end(), None);
}

#[test]
fn an_answer_makes_the_next_look_none_so_a_prompt_asked_again_is_new() {
    let mut watch = InputWatch::default();
    assert_eq!(watch.settle(InputWait::Hidden, false, Some(JOB), 0), Some(InputWait::Hidden));
    // The look right after the answer still sees the old prompt.
    assert_eq!(watch.settle(InputWait::Hidden, false, Some(JOB), 1), Some(InputWait::None));
    assert_eq!(watch.waiting(), None);
    assert_eq!(watch.settle(InputWait::Hidden, false, Some(JOB), 1), Some(InputWait::Hidden));
}

#[test]
fn another_job_in_the_foreground_ends_a_wait_and_its_own_wait_is_new() {
    let mut watch = InputWatch::default();
    assert_eq!(watch.settle(InputWait::Visible, false, Some(JOB), 0), Some(InputWait::Visible));
    assert_eq!(watch.waiting(), Some(Waiting { group: JOB, hidden: false }));
    // The job ended and a command that a later precmd hook started holds the terminal,
    // cooked, with the old prompt still on the screen.
    assert_eq!(watch.settle(InputWait::Visible, false, Some(OTHER_JOB), 0), Some(InputWait::None));
    assert_eq!(watch.waiting(), None);
    assert_eq!(
        watch.settle(InputWait::Visible, false, Some(OTHER_JOB), 0),
        Some(InputWait::Visible)
    );
    assert_eq!(watch.waiting(), Some(Waiting { group: OTHER_JOB, hidden: false }));
    // A wait without a job to answer is none.
    assert_eq!(watch.settle(InputWait::Hidden, false, None, 0), Some(InputWait::None));
    assert_eq!(watch.settle(InputWait::Hidden, false, None, 0), None);
    assert_eq!(watch.waiting(), None);
}

const HIDDEN_WAIT: Waiting = Waiting { group: JOB, hidden: true };
const VISIBLE_WAIT: Waiting = Waiting { group: JOB, hidden: false };

#[test]
fn an_answer_reaches_only_the_job_whose_wait_was_reported() {
    assert_eq!(check_job(Some(HIDDEN_WAIT), JOB), Ok(HIDDEN_WAIT));
    assert!(check_job(Some(HIDDEN_WAIT), OTHER_JOB).is_err(), "a later job holds the terminal");
    assert!(check_job(None, JOB).is_err(), "no wait was reported");
}

#[test]
fn an_answer_is_of_the_kind_of_the_reported_wait() {
    check_kind(HIDDEN_WAIT, true).unwrap();
    check_kind(VISIBLE_WAIT, false).unwrap();
    assert!(check_kind(HIDDEN_WAIT, false).is_err(), "a visible answer for a hidden wait");
    assert!(check_kind(VISIBLE_WAIT, true).is_err(), "a hidden answer for a visible wait");
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
fn a_hidden_answer_needs_a_getpass_read_and_a_visible_one_takes_any_modes() {
    check_modes(HIDDEN, true).unwrap();
    assert!(check_modes(COOKED, true).is_err());
    assert!(check_modes(RAW, true).is_err());
    assert!(check_modes(ECHOING_RAW, true).is_err());
    for modes in [HIDDEN, COOKED, RAW, ECHOING_RAW] {
        check_modes(modes, false).unwrap();
    }
}

#[test]
fn only_the_row_with_the_cursor_counts_as_the_prompt() {
    let asking = screen(&["Password changed earlier", "Continue? "], (1, 10), false);
    assert!(!secret_prompt(&asking));
    let secret = screen(&["Welcome", "Password: "], (1, 10), false);
    assert!(secret_prompt(&secret));
}

#[test]
fn a_visible_wait_that_starts_to_look_secret_is_a_change_and_a_hidden_one_never_does() {
    let mut watch = InputWatch::default();
    assert_eq!(watch.settle(InputWait::Visible, false, Some(JOB), 0), Some(InputWait::Visible));
    assert!(!watch.looks_secret());
    assert_eq!(watch.settle(InputWait::Visible, true, Some(JOB), 0), Some(InputWait::Visible));
    assert!(watch.looks_secret());
    assert_eq!(watch.settle(InputWait::Visible, true, Some(JOB), 0), None);
    // A hidden wait hides the answer anyway; the flag belongs to visible waits only.
    assert_eq!(watch.settle(InputWait::Hidden, true, Some(JOB), 0), Some(InputWait::Hidden));
    assert!(!watch.looks_secret());
    assert_eq!(watch.end(), Some(InputWait::None));
    assert!(!watch.looks_secret());
}
