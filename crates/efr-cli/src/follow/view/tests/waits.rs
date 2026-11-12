//! The status row on a simulated screen while a model call waits for its answer or the
//! model writes a long tool input: a retry that counts down in the frame after it, a
//! connect that shows only when it is slow, the end of both, and the newest words of a
//! tool input at the end of the row (none for a short input, the words after 1 s, a
//! narrow screen).

use std::time::Duration;

use efr_protocol::DraftPart;
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;

use super::super::{Look, TurnView};
use super::{at, draft, sent, turn_started};
use crate::terminal::Size;
use crate::testing::{Grid, turn};

/// No motion and no progress bar, so the screen is the same at every frame.
const LOOK: Look = Look {
    motion: false,
    summary: false,
    progress: false,
    diff_lines: 20,
    diff_lines_long: 8,
    bell: false,
};

/// A started view on a screen `cols` wide, and the screen it draws on.
fn view_of(cols: u16) -> (TurnView, Grid, Size) {
    scoped_view_of(cols, None)
}

/// As [`view_of`], for a prompt in `scope`.
fn scoped_view_of(cols: u16, scope: Option<&str>) -> (TurnView, Grid, Size) {
    let options = RenderOptions::new(cols).with_colour(ColourMode::Ansi16);
    let mut view =
        TurnView::new(turn(), options).with_look(LOOK).with_scope(scope.map(str::to_owned));
    view.start();
    let size = Size { cols, rows: 20 };
    view.envelope(&sent(11, 0, turn_started()), size, false);
    (view, Grid::new(cols), size)
}

/// What the screen shows after the frame at `millis`.
fn frame(view: &mut TurnView, grid: &mut Grid, size: Size, millis: i64) -> Vec<String> {
    grid.write(&view.frame(size, at(millis)));
    grid.lines()
}

fn retrying(attempt: u32, wait: Duration, reason: &str) -> DraftPart {
    DraftPart::Retrying {
        attempt,
        wait_ms: u64::try_from(wait.as_millis()).unwrap(),
        status: Some(503),
        reason: reason.to_owned(),
    }
}

/// The input of call 0 of the answer, `bytes` so far, with its newest part `tail`.
fn input(bytes: usize, tail: Option<&str>) -> DraftPart {
    DraftPart::ToolInput {
        call: 0,
        tool: "shell".to_owned(),
        bytes: bytes as u64,
        tail: tail.map(str::to_owned),
    }
}

#[test]
fn a_retry_shows_in_the_frame_after_it_and_counts_down_to_the_next_try() {
    let (mut view, mut grid, size) = view_of(80);
    assert_eq!(frame(&mut view, &mut grid, size, 0), ["\u{2022} waiting for the model"]);
    view.draft(&draft(11, DraftPart::Connecting), size);
    view.draft(
        &draft(11, retrying(1, Duration::from_millis(1_800), "the provider answered 503")),
        size,
    );
    assert_eq!(
        frame(&mut view, &mut grid, size, 10),
        ["\u{2022} trying again in 2s (the provider answered 503)"]
    );
    assert_eq!(
        frame(&mut view, &mut grid, size, 900),
        ["\u{2022} trying again in 1s (the provider answered 503)"]
    );
    // The next attempt connects: the row keeps the words of the wait.
    view.draft(&draft(11, DraftPart::Connecting), size);
    assert_eq!(
        frame(&mut view, &mut grid, size, 1_900),
        ["\u{2022} trying again (the provider answered 503)   1s"]
    );
    // A second wait names the try that comes.
    view.draft(&draft(11, retrying(2, Duration::from_secs(3), "the connection failed")), size);
    assert_eq!(
        frame(&mut view, &mut grid, size, 2_000),
        ["\u{2022} trying again in 3s (try 3, the connection failed)   2s"]
    );
    // The provider took the call: the row waits for the model again.
    view.draft(&draft(11, DraftPart::Connected), size);
    assert_eq!(frame(&mut view, &mut grid, size, 2_100), ["\u{2022} waiting for the model   2s"]);
}

#[test]
fn the_first_text_after_a_retry_ends_the_wait() {
    let (mut view, mut grid, size) = view_of(80);
    view.draft(&draft(11, retrying(1, Duration::from_secs(1), "the provider answered 429")), size);
    frame(&mut view, &mut grid, size, 10);
    let text = DraftPart::Text { index: 0, offset: 0, delta: "It is zsh.\n\n".to_owned() };
    view.draft(&draft(11, text), size);
    assert_eq!(frame(&mut view, &mut grid, size, 1_200), ["It is zsh.", "\u{2022} writing   1s"]);
}

#[test]
fn the_reason_of_a_retry_is_one_line_that_is_safe_to_print() {
    let (mut view, mut grid, size) = view_of(80);
    view.draft(&draft(11, retrying(1, Duration::from_secs(1), "busy\n\x1b[2J")), size);
    assert_eq!(
        frame(&mut view, &mut grid, size, 10),
        ["\u{2022} trying again in 1s (busy \u{241b}[2J)"]
    );
}

#[test]
fn a_connect_shows_only_when_it_takes_half_a_second() {
    let (mut view, mut grid, size) = view_of(80);
    view.draft(&draft(11, DraftPart::Connecting), size);
    // A fast connect leaves the row as it was, so the row does not flash.
    assert_eq!(frame(&mut view, &mut grid, size, 10), ["\u{2022} waiting for the model"]);
    assert_eq!(frame(&mut view, &mut grid, size, 400), ["\u{2022} waiting for the model"]);
    assert_eq!(frame(&mut view, &mut grid, size, 510), ["\u{2022} connecting"]);
    view.draft(&draft(11, DraftPart::Connected), size);
    assert_eq!(frame(&mut view, &mut grid, size, 600), ["\u{2022} waiting for the model"]);
    // A connect of the next model call counts from its own start.
    view.draft(&draft(11, DraftPart::Connecting), size);
    assert_eq!(frame(&mut view, &mut grid, size, 700), ["\u{2022} waiting for the model"]);
}

#[test]
fn a_connect_does_not_take_the_row_from_the_model_once_it_answers() {
    let (mut view, mut grid, size) = view_of(80);
    let reasoning = DraftPart::Reasoning {
        offset: 0,
        delta: "**Checking**\n".to_owned(),
        title: Some("Checking".to_owned()),
    };
    view.draft(&draft(11, reasoning), size);
    // A late `connecting` of the call that already answers changes nothing.
    view.draft(&draft(11, DraftPart::Connecting), size);
    view.draft(&draft(11, DraftPart::Connected), size);
    assert_eq!(frame(&mut view, &mut grid, size, 900), ["\u{2022} thinking   Checking"]);
}

#[test]
fn a_short_tool_input_shows_no_detail() {
    let (mut view, mut grid, size) = view_of(80);
    let command = r#"{"command":"ls -la"}"#;
    view.draft(&draft(11, input(0, None)), size);
    frame(&mut view, &mut grid, size, 100);
    view.draft(&draft(11, input(command.len(), Some(command))), size);
    // Before a second, the input shows no words: it comes in one piece.
    assert_eq!(frame(&mut view, &mut grid, size, 300), ["\u{2022} preparing shell   20 B"]);
    assert_eq!(frame(&mut view, &mut grid, size, 1_000), ["\u{2022} preparing shell   20 B"]);
}

#[test]
fn a_long_tool_input_shows_its_newest_words_after_a_second_at_the_end_of_the_row() {
    let (mut view, mut grid, size) = scoped_view_of(100, Some("app"));
    let command = r#"{"command":"python3 - <<'EOF'\nimport sys\ns = sys.stdin.read()\nprint(len(s.split()))\nEOF"}"#;
    let mut lines = Vec::new();
    // The model writes the input 8 bytes at a time, a draft each 100 ms.
    for (n, end) in (8..command.len()).step_by(8).chain([command.len()]).enumerate() {
        let tail = &command[end.saturating_sub(256)..end];
        view.draft(&draft(11, input(end, Some(tail))), size);
        lines.push(frame(&mut view, &mut grid, size, 100 + 100 * n as i64).remove(0));
    }
    assert!(
        lines[..9].iter().all(|line| {
            line.starts_with("\u{2022} preparing shell   app   ") && !line.contains('\u{2026}')
        }),
        "{lines:#?}"
    );
    // From 1 s on, the newest words end the row after the size, after the time and the
    // scope, on one line: a newline of the input is a space. Their start moves 4 words
    // at a time.
    assert_eq!(
        lines[10],
        "\u{2022} preparing shell   1s   app   88 B  \u{2026} sys s = sys.stdin.read() print(len(s.split()))"
    );
    assert_eq!(
        lines.last().unwrap(),
        "\u{2022} preparing shell   1s   app   93 B  \u{2026} print(len(s.split())) EOF\"}"
    );
}

#[test]
fn a_narrow_screen_cuts_the_input_words_first() {
    let (mut view, mut grid, size) = view_of(40);
    let command = r#"{"command":"cargo test --workspace --all-features --no-fail-fast"}"#;
    view.draft(&draft(11, input(24, Some(&command[..24]))), size);
    frame(&mut view, &mut grid, size, 100);
    view.draft(&draft(11, input(command.len(), Some(command))), size);
    let lines = frame(&mut view, &mut grid, size, 1_200);
    assert_eq!(lines, ["\u{2022} preparing shell   1s   66 B  \u{2026} --no-f\u{2026}"]);
    assert!(lines[0].chars().count() <= 40, "{lines:?}");
    // Narrower still, the size and the words leave the row and what the turn does
    // stays.
    let (mut view, mut grid, size) = view_of(30);
    view.draft(&draft(11, input(24, Some(&command[..24]))), size);
    frame(&mut view, &mut grid, size, 100);
    view.draft(&draft(11, input(command.len(), Some(command))), size);
    assert_eq!(frame(&mut view, &mut grid, size, 1_200), ["\u{2022} preparing shell   1s"]);
}

#[test]
fn the_next_call_starts_its_input_words_anew() {
    let (mut view, mut grid, size) = view_of(100);
    let first = r#"{"command":"echo one two three four"}"#;
    view.draft(&draft(11, input(first.len(), Some(first))), size);
    frame(&mut view, &mut grid, size, 100);
    let lines = frame(&mut view, &mut grid, size, 1_200);
    assert!(lines[0].ends_with("four\"}"), "{lines:?}");
    // Call 1 of the same answer: its input is new, and it shows no words for a second.
    let second = DraftPart::ToolInput {
        call: 1,
        tool: "shell".to_owned(),
        bytes: 14,
        tail: Some(r#"{"command":"ls"#.to_owned()),
    };
    view.draft(&draft(11, second), size);
    assert_eq!(frame(&mut view, &mut grid, size, 1_300), ["\u{2022} preparing shell   1s   14 B"]);
}

mod design {
    //! A command that waits for input against the frames of the design (TM-Live 21-24):
    //! the answer line under the command's output and the row of what is typed, and the
    //! row that offers `Ctrl+\` after a silence, in the three depths. The rows above them
    //! are the running call's (`call.rs`).

    use efr_render::Glyphs;
    use efr_test_support::design::{Depth, Frame, assert_rows};

    use super::super::super::AnswerKind;
    use super::super::super::waits::{answer_line, echo_row, piece_row, program, silence_hint};
    use crate::testing::design_options;

    /// Checks rows `range` of TM-Live frame `number` against what `draw` gives.
    fn check(
        number: u32,
        range: std::ops::Range<usize>,
        draw: impl Fn(&efr_render::RenderOptions) -> String,
    ) {
        let frame = Frame::load("Live", number).unwrap();
        for depth in Depth::ALL {
            let options = design_options(depth, frame.cols);
            assert_rows(&frame, range.clone(), &draw(&options), depth);
        }
    }

    /// The answer line of `kind` for `command` and the row under it, with `typed`.
    fn asked(
        kind: AnswerKind,
        command: &str,
        typed: Option<&str>,
        options: &efr_render::RenderOptions,
    ) -> String {
        let line = answer_line(kind, program(command), options.glyphs());
        format!("{}{}", piece_row(&line, options), echo_row(typed, options))
    }

    #[test]
    fn a_command_asks_and_the_answer_line_takes_the_input_rows_place_live_21() {
        check(21, 2..4, |options| {
            asked(AnswerKind::Visible, "sudo pacman -Syu", Some("y"), options)
        });
    }

    #[test]
    fn a_password_shows_nothing_typed_not_even_dots_live_22() {
        check(22, 2..4, |options| asked(AnswerKind::Hidden, "sudo pacman -Syu", None, options));
    }

    #[test]
    fn a_password_behind_another_program_names_it_live_23() {
        let command = "ssh laptop sudo systemctl restart efrd";
        check(23, 2..4, |options| asked(AnswerKind::Masked, command, None, options));
    }

    #[test]
    fn a_silence_offers_ctrl_backslash_live_24() {
        check(24, 2..3, |options| {
            piece_row(&[(silence_hint(options.glyphs()), crate::format::Tone::Dim)], options)
        });
    }

    #[test]
    fn the_first_program_names_the_command() {
        assert_eq!(program("sudo pacman -Syu"), Some("sudo"));
        assert_eq!(program("LANG=C /usr/bin/ssh host"), Some("ssh"));
        assert_eq!(program("  "), None);
        let ascii = answer_line(AnswerKind::Visible, None, &Glyphs::ASCII);
        assert_eq!(ascii[0].0, "?");
        assert!(ascii[4].0.contains(" - "), "{ascii:?}");
    }
}
