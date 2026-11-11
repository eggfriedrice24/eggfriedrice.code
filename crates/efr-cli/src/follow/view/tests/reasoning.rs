//! The reasoning of the model on a simulated screen: the title or the newest words in
//! the status row while the model thinks, the row that a phase of 2 s or more leaves in
//! the scrollback (with a title, with the first words, with no words), a phase under
//! 2 s that leaves nothing, a phase under 1 s that shows no words, a narrow screen, the
//! spacing around the row, `NO_COLOR` and a pipe.

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
fn a_titled_reasoning_shows_its_title_in_the_status_row_and_leaves_a_bold_row() {
    let (mut view, mut grid, size) = view_of(60, ColourMode::Ansi16);
    let title = "**Planning the answer**\n\n";
    let parts = [
        reasoning(0, title, Some("Planning the answer")),
        reasoning(title.len(), "The user asks", Some("Planning the answer")),
        reasoning(title.len() + 13, " about the log size", Some("Planning the answer")),
    ];
    let lines = think(&mut view, &mut grid, size, &parts, 100, 1_000);
    // The title stands in the status row, and no row of the live zone holds the words.
    assert_eq!(lines, ["\u{2022} thinking: Planning the answer  2s"]);
    // A draft that repeats text that the view holds adds only the rest.
    let repeated =
        reasoning(title.len() + 13, " about the log size, in GiB", Some("Planning the answer"));
    view.draft(&draft(11, repeated), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 3_100);
    assert_eq!(lines, ["\u{2022} thinking: Planning the answer  3s"]);
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
    think(&mut view, &mut grid, size, &parts, 100, 0);
    let (lines, _) = frame(&mut view, &mut grid, size, 1_200);
    assert_eq!(
        lines,
        // Without a title, the newest words take its place in the status row.
        ["\u{2022} thinking: \u{2026} at the vacuum options.  1s"]
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
    // No words: the status row says only `thinking`.
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
    assert_eq!(lines, ["\u{2022} thinking: Checking"]);
    let (lines, _) = frame(&mut view, &mut grid, size, 2_000);
    assert_eq!(lines, ["\u{2022} thinking: Checking  1s"]);
    view.draft(&draft(11, text(0, "Done.\n\n")), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 2_050);
    assert_eq!(lines, ["Done.", "\u{2022} writing  1s"]);
    assert!(!bare(&view.frame(size, at(2_100))).contains("thought"));
}

#[test]
fn a_phase_under_one_second_shows_no_words() {
    // The summary of a short phase comes in one piece just before the tool call or the
    // text that follows it. Its words would show for a moment and go.
    let (mut view, mut grid, size) = view_of(60, ColourMode::Ansi16);
    view.draft(&draft(11, reasoning(0, "I check the client file first.", None)), size);
    for millis in [100, 400, 900] {
        let (lines, _) = frame(&mut view, &mut grid, size, millis);
        assert_eq!(lines, ["\u{2022} thinking"], "at {millis} ms");
    }
    view.draft(&draft(11, text(0, "Done.\n\n")), size);
    let (lines, _) = frame(&mut view, &mut grid, size, 950);
    assert_eq!(lines, ["Done.", "\u{2022} writing"]);
}

/// The words of the reasoning in the status row `line`, between `thinking: ` and the
/// time.
fn words_of(line: &str) -> &str {
    let rest = line.split_once("thinking: ").map_or("", |(_, rest)| rest);
    rest.rsplit_once("  ").map_or(rest, |(words, _)| words)
}

#[test]
fn the_newest_words_are_whole_words_and_their_start_moves_in_steps() {
    let (mut view, mut grid, size) = view_of(100, ColourMode::Ansi16);
    let words: Vec<String> = (0..40).map(|n| format!("word{n:02}")).collect();
    let mut sent = 0;
    let mut starts = Vec::new();
    for (n, word) in words.iter().enumerate() {
        let delta = if n == 0 { word.clone() } else { format!(" {word}") };
        view.draft(&draft(11, reasoning(sent, &delta, None)), size);
        sent += delta.len();
        let (lines, _) = frame(&mut view, &mut grid, size, 100 + 100 * n as i64);
        if n < 10 {
            continue;
        }
        assert_eq!(lines.len(), 1, "{lines:?}");
        let shown = words_of(&lines[0]);
        // At most 50 columns of whole words, and the newest word is the last one.
        assert!(shown.chars().count() <= 50, "{shown}");
        assert!(shown.ends_with(word.as_str()), "{shown}");
        let rest = shown.strip_prefix("\u{2026} ").unwrap_or_else(|| panic!("{shown}"));
        starts.push(rest[..6].to_owned());
    }
    // The start stands for some words and then jumps on by 4 words. It never moves
    // with each word.
    starts.dedup();
    let steps: Vec<String> = (1..10).map(|n| format!("word{:02}", n * 4)).collect();
    assert_eq!(starts, steps);
}

#[test]
fn a_wide_screen_keeps_the_words_to_50_columns() {
    let (mut view, mut grid, size) = view_of(232, ColourMode::Ansi16);
    let long: String = (0..80).map(|n| format!("word{n:02} ")).collect();
    view.draft(&draft(11, reasoning(0, long.trim_end(), None)), size);
    frame(&mut view, &mut grid, size, 100);
    let (lines, _) = frame(&mut view, &mut grid, size, 1_200);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let shown = words_of(&lines[0]);
    assert!(shown.chars().count() <= 50, "{shown}");
    assert!(shown.starts_with("\u{2026} word"), "{shown}");
    assert!(shown.ends_with("word79"), "{shown}");
}

#[test]
fn a_narrow_screen_cuts_the_status_row_and_the_row_to_one_row() {
    let (mut view, mut grid, size) = view_of(24, ColourMode::Ansi16);
    let parts = [
        reasoning(0, "**Planning the long answer**\n\n", Some("Planning the long answer")),
        reasoning(30, "one two three four five six seven", Some("Planning the long answer")),
    ];
    let lines = think(&mut view, &mut grid, size, &parts, 100, 2_900);
    assert_eq!(lines, ["\u{2022} thinking: Plannin\u{2026}  2s"]);
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
    think_after(12, &mut view, &mut grid, size, &parts, 100, 0);
    let (lines, _) = frame(&mut view, &mut grid, size, 1_200);
    assert_eq!(lines, ["First.", "\u{2022} thinking: Next step  1s"]);
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
    let parts = [reasoning(0, "**Planning**\n\nThe user asks", Some("Planning"))];
    think(&mut view, &mut grid, size, &parts, 100, 0);
    frame(&mut view, &mut grid, size, 2_550);
    view.draft(&draft(11, text(0, "Done.\n\n")), size);
    let (lines, written) = frame(&mut view, &mut grid, size, 2_600);
    assert_eq!(lines[0], "\u{25b8} Planning  thought 2s");
    assert!(written.contains("\x1b[1mPlanning\x1b[0m"), "{}", readable(&written));
    // Without a title, the first words of the row are in italic.
    let (mut view, mut grid, size) = view_of(60, ColourMode::None);
    think(&mut view, &mut grid, size, &[reasoning(0, "The user asks more", None)], 100, 0);
    frame(&mut view, &mut grid, size, 2_550);
    view.draft(&draft(11, text(0, "Done.\n\n")), size);
    let (lines, written) = frame(&mut view, &mut grid, size, 2_600);
    assert_eq!(lines[0], "\u{25b8} The user asks more  thought 2s");
    assert!(written.contains("\x1b[2;3mThe user asks more\x1b[0m"), "{}", readable(&written));
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

#[test]
fn the_end_line_after_the_row_keeps_its_blank_line() {
    let options = RenderOptions::new(60).with_colour(ColourMode::Ansi16);
    let mut view = TurnView::new(turn(), options).with_look(Look { summary: true, ..LOOK });
    view.start();
    let size = Size { cols: 60, rows: 20 };
    view.envelope(&sent(11, 0, turn_started()), size, false);
    let mut grid = Grid::new(60);
    let parts = [reasoning(0, "**Plan it**\n\nFirst the tests", Some("Plan it"))];
    think(&mut view, &mut grid, size, &parts, 100, 0);
    frame(&mut view, &mut grid, size, 2_600);
    // The turn ends with no text and no call: the row sticks to nothing.
    let step = view.envelope(&sent(12, 3_500, turn_completed()), size, false);
    grid.write(&step.out);
    grid.write(&step.err);
    let (lines, _) = frame(&mut view, &mut grid, size, 3_500);
    assert_eq!(lines[..3], ["\u{25b8} Plan it  thought 2s", "", "done in 3.5s"], "{lines:?}");
}
