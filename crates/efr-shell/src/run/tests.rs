use efr_protocol::{Cell, Cursor, RowCells, ScreenSnapshot, Seq, Size};
use efr_screen::ShellMarkScanner;
use pretty_assertions::assert_eq;

use super::{
    CommandResult, Completion, Delimiter, MarkRun, MarkStep, OutputUpdate, RunRequest, marked_line,
    screen_tail, waits_for_input,
};

fn row(text: &str) -> RowCells {
    RowCells {
        cells: text.chars().map(|c| Cell { text: c.to_string(), ..Cell::default() }).collect(),
        wrapped: false,
    }
}

fn screen(rows: &[&str], cursor: (u16, u16)) -> ScreenSnapshot {
    ScreenSnapshot {
        size: Size { cols: 80, rows: 24 },
        cursor: Cursor { row: cursor.0, col: cursor.1, hidden: false },
        rows: rows.iter().map(|text| row(text)).collect(),
        ..ScreenSnapshot::default()
    }
}

/// Feeds `bytes` (one chunk at offset `at`) to `run` the way the session does: bytes
/// between marks, then each mark.
fn feed(run: &mut MarkRun, at: u64, bytes: &[u8]) -> Vec<MarkStep> {
    let mut steps = Vec::new();
    let mut cursor = at;
    for mark in ShellMarkScanner::new().scan(bytes, Seq::new(at)) {
        let from = usize::try_from(cursor - at).unwrap();
        let to = usize::try_from(mark.start.get() - at).unwrap();
        run.on_bytes(Seq::new(cursor), &bytes[from..to]);
        steps.push(run.on_mark(&mark));
        cursor = mark.end.get();
    }
    let from = usize::try_from(cursor - at).unwrap();
    run.on_bytes(Seq::new(cursor), &bytes[from..]);
    steps
}

#[test]
fn a_request_has_the_defaults() {
    let request = RunRequest::new("ls", "/tmp");
    assert_eq!(request.timeout, RunRequest::DEFAULT_TIMEOUT);
    assert_eq!(request.output_limit, RunRequest::DEFAULT_OUTPUT_LIMIT);
    assert_eq!(request.mode, super::RunMode::Auto);
}

#[test]
fn a_marked_line_is_the_clear_key_one_bracketed_paste_and_enter() {
    let line = marked_line("for f in *; do\n\techo $f\ndone").unwrap();
    assert_eq!(&line[..], b"\x1b[efr-clear~\x1b[200~for f in *; do\n\techo $f\ndone\x1b[201~\r");
}

#[test]
fn a_marked_line_refuses_what_a_paste_cannot_carry() {
    assert!(marked_line("a\0b").is_err());
    assert!(marked_line("x\x1b[201~y").is_err());
    assert!(marked_line("\n").is_err());
}

#[test]
fn the_output_lies_between_c_and_d() {
    let mut run = MarkRun::new(Seq::new(10), 1024);
    let steps = feed(&mut run, 10, b"echo\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07");
    let MarkStep::Ended(output) = &steps[1] else {
        panic!("the run should end at D: {steps:?}");
    };
    assert_eq!(output.completion, Completion::Finished);
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.kept.clean().text, "hi\n");
    // "echo\r\n" is 10..16 and C (ESC ] 1 3 3 ; C BEL) is 16..24.
    assert_eq!(output.range, Some(Seq::new(24)..Seq::new(28)));
}

#[test]
fn marks_from_before_the_line_was_typed_are_ignored() {
    let mut run = MarkRun::new(Seq::new(100), 1024);
    let steps = feed(&mut run, 0, b"\x1b]133;D;1\x07");
    assert_eq!(steps, [MarkStep::Continue]);
}

#[test]
fn a_continuation_prompt_before_c_cancels_once_at_its_input_mark() {
    let mut run = MarkRun::new(Seq::ZERO, 1024);
    let steps =
        feed(&mut run, 0, b"\x1b]133;P;k=s\x07> \x1b]133;B\x07\x1b]133;P;k=s\x07\x1b]133;B\x07");
    assert_eq!(
        steps,
        [MarkStep::Continue, MarkStep::Cancel, MarkStep::Continue, MarkStep::Continue]
    );
}

#[test]
fn a_continuation_prompt_inside_the_output_is_output() {
    let mut run = MarkRun::new(Seq::ZERO, 1024);
    let steps = feed(&mut run, 0, b"\x1b]133;C\x07\x1b]133;P;k=s\x07\x1b]133;B\x07");
    assert_eq!(steps, [MarkStep::Continue, MarkStep::Continue, MarkStep::Continue]);
}

#[test]
fn a_prompt_with_text_before_the_cursor_waits_for_input() {
    assert!(waits_for_input(&screen(&["$ sudo true", "[sudo] password for u: "], (1, 23))));
    assert!(waits_for_input(&screen(&["Proceed? [Y/n] "], (0, 15))));
}

#[test]
fn a_cursor_at_the_start_of_a_line_does_not_wait() {
    assert!(!waits_for_input(&screen(&["compiling", ""], (1, 0))));
    assert!(!waits_for_input(&screen(&["   "], (0, 3))));
}

#[test]
fn a_full_screen_program_waits() {
    let mut snapshot = screen(&[""], (0, 0));
    snapshot.alternate_screen = true;
    assert!(waits_for_input(&snapshot));
}

#[test]
fn the_screen_tail_ends_at_the_cursor_row() {
    let snapshot = screen(&["one", "two", "", "four", ""], (3, 4));
    assert_eq!(screen_tail(&snapshot), "one\ntwo\n\nfour");
    let snapshot = screen(&["one", "", ""], (2, 0));
    assert_eq!(screen_tail(&snapshot), "one");
}

#[test]
fn other_runners_can_build_results() {
    let result = CommandResult::finished(Some(0), "ok\n", "/tmp")
        .with_completion(Completion::Interactive)
        .with_screen_tail("Password:")
        .with_truncation(10_000)
        .with_delimiter(Delimiter::Sentinel);
    assert!(result.interactive());
    assert_eq!(result.output, "ok\n");
    assert_eq!(result.output_bytes, 10_000);
    assert!(result.truncated);
    assert_eq!(result.screen_tail.as_deref(), Some("Password:"));
    assert_eq!(result.delimiter, Delimiter::Sentinel);
    assert_eq!(CommandResult::finished(None, "abc", "/").output_bytes, 3);
    assert_eq!(OutputUpdate::new(3, "abc").tail, "abc");
}
