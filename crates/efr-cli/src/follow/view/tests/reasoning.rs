//! The reasoning of the model on a simulated screen: the one live line while the model
//! thinks, the row that a phase of 2 s or more leaves in the scrollback (with a title,
//! with the first words, with no words), a phase under 2 s that leaves nothing, a
//! narrow screen, the spacing around the row, `NO_COLOR` and a pipe.

use efr_protocol::{DraftPart, Event};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;

use super::super::{Look, TurnView};
use super::{at, completed, draft, sent, turn_completed, turn_started};
use crate::terminal::Size;
use crate::testing::{Grid, bare, readable, turn};

/// No motion and no progress bar, so the screen is the same at every frame.
const LOOK: Look = Look { motion: false, summary: false, progress: false, diff_lines: 20 };

/// A started view on a screen `cols` wide, with `colour`, and the screen it draws on.
fn view_of(cols: u16, colour: ColourMode) -> (TurnView, Grid, Size) {
    let options = RenderOptions::new(cols).with_colour(colour);
    let mut view = TurnView::new(turn(), options).with_look(LOOK);
    view.start();
    view.envelope(&sent(11, 0, turn_started()), Size { cols, rows: 20 }, false);
    (view, Grid::new(cols), Size { cols, rows: 20 })
}

/// The reasoning `delta` at byte `offset` of the turn's reasoning, under `title`.
fn reasoning(offset: usize, delta: &str, title: Option<&str>) -> DraftPart {
    DraftPart::Reasoning {
        offset: offset as u64,
        delta: delta.to_owned(),
        title: title.map(str::to_owned),
    }
}

fn text(index: u32, delta: &str) -> DraftPart {
    DraftPart::Text { index, offset: 0, delta: delta.to_owned() }
}

/// What the screen shows after the frame at `millis`, and what that frame wrote.
fn frame(view: &mut TurnView, grid: &mut Grid, size: Size, millis: i64) -> (Vec<String>, String) {
    let written = view.frame(size, at(millis));
    grid.write(&written);
    (grid.lines(), written)
}

/// Feeds `parts` as drafts made after event 11, one frame after each, `step` ms apart
/// from `from` on.
fn think(
    view: &mut TurnView,
    grid: &mut Grid,
    size: Size,
    parts: &[DraftPart],
    from: i64,
    step: i64,
) -> Vec<String> {
    think_after(11, view, grid, size, parts, from, step)
}

/// As [`think`], with drafts made after event `after`.
fn think_after(
    after: u64,
    view: &mut TurnView,
    grid: &mut Grid,
    size: Size,
    parts: &[DraftPart],
    from: i64,
    step: i64,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (n, part) in parts.iter().enumerate() {
        view.draft(&draft(after, part.clone()), size);
        lines = frame(view, grid, size, from + step * n as i64).0;
    }
    lines
}

#[test]
fn a_titled_reasoning_shows_its_newest_words_live_and_leaves_a_bold_row() {
    let (mut view, mut grid, size) = view_of(60, ColourMode::Ansi16);
    let title = "**Planning the answer**\n\n";
    let parts = [
        reasoning(0, title, Some("Planning the answer")),
        reasoning(title.len(), "The user asks", Some("Planning the answer")),
        reasoning(title.len() + 13, " about the log size", Some("Planning the answer")),
    ];
    let lines = think(&mut view, &mut grid, size, &parts, 100, 1_000);
    assert_eq!(
        lines,
        ["The user asks about the log size", "\u{2022} thinking: Planning the answer  2s"]
    );
    // A draft that repeats text that the view holds adds only the rest.
    let repeated =
        reasoning(title.len() + 13, " about the log size, in GiB", Some("Planning the answer"));
    view.draft(&draft(11, repeated), size);
    let (lines, written) = frame(&mut view, &mut grid, size, 3_100);
    assert_eq!(
        lines,
        ["The user asks about the log size, in GiB", "\u{2022} thinking: Planning the answer  3s"]
    );
    assert!(written.contains("\x1b[2;3mThe user asks"), "{}", readable(&written));
    // The text ends the phase: its row stays at column 0, the text sticks to it.
    view.draft(&draft(11, text(0, "It takes 3.1 GiB.\n\n")), size);
    let (lines, written) = frame(&mut view, &mut grid, size, 3_200);
    assert_eq!(
        lines,
        ["\u{25b8} Planning the answer  thought 3s", "It takes 3.1 GiB.", "\u{2022} writing  3s"]
    );
    assert!(
        written.contains(
            "\x1b[2m\u{25b8} \x1b[0m\x1b[1mPlanning the answer\x1b[0m\x1b[2m  thought 3s\x1b[0m\n"
        ),
        "{}",
        readable(&written)
    );
}

#[test]
fn a_reasoning_without_a_title_leaves_its_first_words_in_italic() {
    let (mut view, mut grid, size) = view_of(100, ColourMode::Ansi16);
    let words = "The user wants the size of the journal. I should run `journalctl \
                 --disk-usage` first, then look at **the vacuum** options.";
    let parts = [reasoning(0, words, None)];
    let lines = think(&mut view, &mut grid, size, &parts, 100, 0);
    assert_eq!(
        lines,
        [
            "\u{2026}e size of the journal. I should run journalctl --disk-usage first, then look at the vacuum options.",
            "\u{2022} thinking",
        ]
    );
    frame(&mut view, &mut grid, size, 4_600);
    view.envelope(&sent(12, 4_700, turn_completed()), size, false);
    let (lines, written) = frame(&mut view, &mut grid, size, 4_700);
    assert_eq!(
        lines,
        ["\u{25b8} The user wants the size of the journal. I should run\u{2026}  thought 4s"]
    );
    assert!(
        written.contains("\x1b[2;3mThe user wants the size of the journal."),
        "{}",
        readable(&written)
    );
}

#[test]
fn an_empty_reasoning_leaves_only_the_time() {
    let (mut view, mut grid, size) = view_of(60, ColourMode::Ansi16);
    let lines = think(&mut view, &mut grid, size, &[reasoning(0, "\n\n", None)], 100, 0);
    // No words, so no live line: the status row alone.
    assert_eq!(lines, ["\u{2022} thinking"]);
    frame(&mut view, &mut grid, size, 3_400);
    view.draft(&draft(11, text(0, "Done.\n\n")), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 3_500);
    assert_eq!(lines, ["\u{25b8} thought 3s", "Done.", "\u{2022} writing  3s"]);
}

#[test]
fn a_phase_under_two_seconds_leaves_no_row() {
    let (mut view, mut grid, size) = view_of(60, ColourMode::Ansi16);
    let parts = [reasoning(0, "**Checking**\n\nA quick look", Some("Checking"))];
    let lines = think(&mut view, &mut grid, size, &parts, 100, 0);
    assert_eq!(lines, ["A quick look", "\u{2022} thinking: Checking"]);
    frame(&mut view, &mut grid, size, 2_000);
    view.draft(&draft(11, text(0, "Done.\n\n")), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 2_050);
    assert_eq!(lines, ["Done.", "\u{2022} writing  1s"]);
    assert!(!bare(&view.frame(size, at(2_100))).contains("thought"));
}

#[test]
fn a_narrow_screen_cuts_the_live_line_and_the_row_to_one_row() {
    let (mut view, mut grid, size) = view_of(24, ColourMode::Ansi16);
    let parts = [
        reasoning(0, "**Planning the long answer**\n\n", Some("Planning the long answer")),
        reasoning(30, "one two three four five six seven", Some("Planning the long answer")),
    ];
    let lines = think(&mut view, &mut grid, size, &parts, 100, 2_900);
    assert_eq!(
        lines,
        ["\u{2026}ree four five six seven", "\u{2022} thinking: Plannin\u{2026}  2s"]
    );
    view.draft(&draft(11, text(0, "Yes.\n\n")), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 3_100);
    assert_eq!(lines, ["\u{25b8} Planning\u{2026}  thought 2s", "Yes.", "\u{2022} writing  3s"]);
    for line in &lines {
        assert!(line.chars().count() <= 24, "{line}");
    }
    // Narrower still, the title leaves the row and the time stays.
    let (mut view, mut grid, size) = view_of(16, ColourMode::Ansi16);
    think(&mut view, &mut grid, size, &parts, 100, 2_900);
    view.draft(&draft(11, text(0, "Yes.\n\n")), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 3_100);
    assert_eq!(lines[0], "\u{25b8} thought 2s");
}

#[test]
fn the_row_sticks_to_what_follows_it_and_the_blank_line_goes_above_it() {
    let (mut view, mut grid, size) = view_of(60, ColourMode::Ansi16);
    view.envelope(&sent(12, 10, completed(0, "First.")), size, false);
    frame(&mut view, &mut grid, size, 10);
    let parts = [reasoning(0, "**Next step**\n\nThen the docs", Some("Next step"))];
    let lines = think_after(12, &mut view, &mut grid, size, &parts, 100, 0);
    // The live line takes the place of the row, with the blank line above it.
    assert_eq!(lines, ["First.", "", "Then the docs", "\u{2022} thinking: Next step"]);
    frame(&mut view, &mut grid, size, 2_600);
    view.draft(&draft(12, text(1, "Second.\n\n")), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 2_700);
    assert_eq!(
        lines,
        ["First.", "", "\u{25b8} Next step  thought 2s", "Second.", "\u{2022} writing  2s"]
    );
    // Before a tool call the row sticks to the call too.
    let (mut view, mut grid, size) = view_of(60, ColourMode::Ansi16);
    view.envelope(&sent(12, 10, completed(0, "First.")), size, false);
    think_after(12, &mut view, &mut grid, size, &parts, 100, 0);
    frame(&mut view, &mut grid, size, 2_600);
    let input = DraftPart::ToolInput { call: 0, tool: "shell".to_owned(), bytes: 20 };
    view.draft(&draft(12, input), size);
    let started = Event::ToolCallStarted {
        turn_id: turn(),
        call_id: crate::testing::call(),
        tool: "shell".to_owned(),
        input: serde_json::json!({ "command": "ls" }),
        manual_input: false,
        launch: None,
        freeform: false,
    };
    view.envelope(&sent(13, 2_700, started), size, false);
    let done = Event::ToolCallCompleted {
        turn_id: turn(),
        call_id: crate::testing::call(),
        output: String::new(),
        truncated: false,
        is_error: false,
        exit_code: Some(0),
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
    };
    view.envelope(&sent(14, 2_800, done), size, false);
    let (lines, _) = frame(&mut view, &mut grid, size, 2_800);
    assert_eq!(lines[..3], ["First.", "", "\u{25b8} Next step  thought 2s"]);
    assert!(lines[3].contains("$ ls"), "{lines:?}");
}

#[test]
fn no_color_keeps_the_bold_and_the_italic() {
    let (mut view, mut grid, size) = view_of(60, ColourMode::None);
    let start = "**Planning**\n\nThe user asks";
    let parts =
        [reasoning(0, start, Some("Planning")), reasoning(start.len(), " more", Some("Planning"))];
    think(&mut view, &mut grid, size, &parts, 100, 2_400);
    let (_, live) = frame(&mut view, &mut grid, size, 2_550);
    let _ = live;
    view.draft(&draft(11, reasoning(start.len() + 5, " again", Some("Planning"))), size);
    let (_, live) = frame(&mut view, &mut grid, size, 2_560);
    assert!(live.contains("\x1b[2;3mThe user asks more again\x1b[0m"), "{}", readable(&live));
    view.draft(&draft(11, text(0, "Done.\n\n")), size);
    let (lines, written) = frame(&mut view, &mut grid, size, 2_600);
    assert_eq!(lines[0], "\u{25b8} Planning  thought 2s");
    assert!(written.contains("\x1b[1mPlanning\x1b[0m"), "{}", readable(&written));
}

#[test]
fn a_pipe_shows_nothing_of_the_reasoning() {
    let mut view = TurnView::new(turn(), RenderOptions::new(60).with_terminal(false));
    let size = Size { cols: 60, rows: 20 };
    view.envelope(&sent(11, 0, turn_started()), size, false);
    let step = view.draft(&draft(11, reasoning(0, "**Planning**\n\nThe user asks", None)), size);
    assert_eq!(step.out, "");
    assert_eq!(step.err, "");
    assert_eq!(view.frame(size, at(3_000)), "");
    let step = view.envelope(&sent(12, 3_000, completed(0, "Done.")), size, false);
    assert_eq!(step.out, "Done.\n");
    assert_eq!(step.err, "");
}
