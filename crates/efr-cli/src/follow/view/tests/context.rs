//! How full the model's context is, as a followed turn shows it: the gauge in the
//! status row and in the running call's line, `compacting context`, the line of each
//! compaction and the end-of-turn line, against the events and drafts of the context
//! contract (the README of `efr-conversation`, section "Context").

use std::path::PathBuf;

use efr_protocol::{
    CompactionTrigger, ContextUse, Draft, DraftPart, Event, EventEnvelope, Scope, Seq, Usage,
};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{Look, TurnView};
use super::at;
use crate::terminal::Size;
use crate::testing::{Grid, bare, call, compaction, readable, turn};

const SIZE: Size = Size { cols: 80, rows: 20 };

/// No motion and no progress bar, so a frame shows the same at every tick.
const LOOK: Look = Look { motion: false, summary: true, progress: false, diff_lines: 20 };

/// The limit of the tests: 76% of a window of 272k.
const LIMIT: u64 = 206_720;

fn context(tokens: u64) -> ContextUse {
    ContextUse { tokens, limit: LIMIT, window: 272_000 }
}

fn view(options: RenderOptions) -> TurnView {
    let mut view = TurnView::new(turn(), options).with_look(LOOK);
    view.start();
    view
}

fn sent(seq: u64, millis: i64, event: Event) -> EventEnvelope {
    EventEnvelope { seq: Seq::new(seq), conversation_id: None, at: at(millis), event }
}

fn draft(after: u64, part: DraftPart) -> Draft {
    Draft { turn_id: turn(), after_seq: Seq::new(after), draft: part }
}

fn turn_started() -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/user/project"),
        scope: Scope::Machine,
        settings: None,
    }
}

fn completed(tokens: u64) -> Event {
    Event::TurnCompleted {
        turn_id: turn(),
        usage: Some(Usage { cached_input_tokens: 870_000, ..Usage::new(918_000, 1_100) }),
        changes: None,
        context: Some(context(tokens)),
    }
}

/// The status row of `view` after `part`, as it is painted: the frame after its last
/// erase, without its newline and the end of synchronized output.
fn row_after(view: &mut TurnView, part: DraftPart, millis: i64) -> String {
    view.draft(&draft(11, part), SIZE);
    let frame = view.frame(SIZE, at(millis));
    let row = frame.rsplit(['J', 'K']).next().unwrap_or_default();
    row.trim_end_matches("\x1b[?2026l").trim_end_matches('\n').to_owned()
}

#[test]
fn the_status_row_shows_the_gauge_in_the_colour_of_its_level() {
    let mut shown = Vec::new();
    for (name, colour) in [("16 colours", ColourMode::Ansi16), ("NO_COLOR", ColourMode::None)] {
        let mut view = view(RenderOptions::new(SIZE.cols).with_colour(colour));
        view.envelope(&sent(11, 0, turn_started()), SIZE, false);
        view.frame(SIZE, at(0));
        let mut rows = Vec::new();
        for (tokens, millis) in
            [(89_400, 500), (103_360, 2_000), (186_048, 3_000), (240_000, 4_000)]
        {
            rows.push(readable(&row_after(&mut view, DraftPart::Context(context(tokens)), millis)));
        }
        shown.push(format!("{name}:\n{}", rows.join("\n")));
    }
    insta::assert_snapshot!(shown.join("\n===\n"));
}

#[test]
fn a_compaction_says_so_in_the_status_row_until_its_event_and_leaves_one_line() {
    let mut view = view(RenderOptions::new(SIZE.cols));
    let mut grid = Grid::new(SIZE.cols);
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    view.draft(&draft(11, DraftPart::Context(context(205_000))), SIZE);
    grid.write(&view.frame(SIZE, at(0)));
    view.draft(&draft(11, DraftPart::Compacting { trigger: CompactionTrigger::Auto }), SIZE);
    grid.write(&view.frame(SIZE, at(1_200)));
    assert_eq!(grid.lines(), ["\u{2022} compacting context  1s  ctx 99%"]);
    let event = Event::ConversationCompacted(compaction(CompactionTrigger::Auto, 24_100));
    view.envelope(&sent(12, 6_000, event), SIZE, false);
    let frame = view.frame(SIZE, at(6_000));
    grid.write(&frame);
    assert_eq!(
        grid.lines(),
        [
            "context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k",
            "\u{2022} waiting for the model  6s  ctx 11%",
        ]
    );
    // The line is muted, the gauge is in the colour of its level.
    let frame = readable(&frame);
    assert!(frame.contains("\\e[2mcontext compacted (auto): 231k"), "{frame}");
    assert!(frame.contains("\\e[32mctx 11%\\e[0m"), "{frame}");
}

#[test]
fn the_next_count_ends_a_compaction_that_shows_and_the_end_of_the_turn_does_too() {
    let mut view = view(RenderOptions::new(SIZE.cols).with_colour(ColourMode::None));
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    let compacting = DraftPart::Compacting { trigger: CompactionTrigger::Overflow };
    assert!(row_after(&mut view, compacting.clone(), 100).contains("compacting context"));
    let counted = row_after(&mut view, DraftPart::Context(context(30_000)), 200);
    assert!(bare(&counted).ends_with("waiting for the model  ctx 14%"), "{counted}");
    assert!(row_after(&mut view, compacting, 300).contains("compacting context"));
    view.envelope(&sent(12, 2_000, completed(30_000)), SIZE, false);
    let end = readable(&view.frame(SIZE, at(2_000)));
    assert!(!end.contains("compacting"), "{end}");
}

#[test]
fn the_running_call_carries_the_gauge_while_the_status_row_hides() {
    let mut view = view(RenderOptions::new(SIZE.cols));
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    view.draft(&draft(11, DraftPart::Context(context(186_048))), SIZE);
    let started = Event::ToolCallStarted {
        turn_id: turn(),
        call_id: call(),
        tool: "shell".to_owned(),
        input: json!({ "command": "cargo test -p app" }),
        manual_input: false,
        launch: None,
        freeform: false,
    };
    view.envelope(&sent(12, 0, started), SIZE, false);
    view.frame(SIZE, at(0));
    let frame = readable(&view.tick(SIZE, at(12_300)));
    assert!(
        frame.contains("$ cargo test -p app\\e[0m  \\e[2m12s\\e[0m  \\e[31mctx 90%"),
        "{frame}"
    );
    assert!(!frame.contains("waiting for"), "the call's line replaces the row: {frame}");
    // At 30 columns the gauge stays and the command is cut; at 20 the command keeps
    // what room is left, and the gauge goes.
    let frame = bare(&view.tick(Size { cols: 30, rows: 20 }, at(13_400)));
    assert!(frame.contains("$ cargo test \u{2026}  13s  ctx 90%"), "{frame}");
    let frame = bare(&view.tick(Size { cols: 20, rows: 20 }, at(14_400)));
    assert!(frame.contains("$ cargo test\u{2026}  14s") && !frame.contains("ctx"), "{frame}");
}

#[test]
fn the_end_of_the_turn_shows_the_gauge_of_its_event() {
    let mut shown = Vec::new();
    for (name, options) in [
        ("16 colours", RenderOptions::new(SIZE.cols)),
        ("NO_COLOR", RenderOptions::new(SIZE.cols).with_colour(ColourMode::None)),
    ] {
        for tokens in [89_400, 120_000, 196_000] {
            let mut view = view(options.clone());
            view.envelope(&sent(11, 0, turn_started()), SIZE, false);
            view.frame(SIZE, at(0));
            let step = view.envelope(&sent(12, 42_000, completed(tokens)), SIZE, false);
            assert!(step.end.is_some());
            let frame = view.frame(SIZE, at(42_000));
            let line = frame.lines().find(|line| line.contains("done")).unwrap_or_default();
            let line = line.rsplit('J').next().unwrap_or_default();
            shown.push(format!("{name}, {tokens}: {}", readable(line)));
        }
    }
    insta::assert_snapshot!(shown.join("\n"));
}

#[test]
fn an_interrupted_turn_shows_the_gauge_and_one_without_a_count_keeps_its_line() {
    let interrupted = |context| {
        let mut view = view(RenderOptions::new(SIZE.cols).with_colour(ColourMode::None));
        view.envelope(&sent(11, 0, turn_started()), SIZE, false);
        view.frame(SIZE, at(0));
        let event = Event::TurnInterrupted { turn_id: turn(), usage: None, context };
        view.envelope(&sent(12, 12_400, event), SIZE, false);
        bare(&view.frame(SIZE, at(12_400)))
    };
    let counted = interrupted(Some(context(89_400)));
    assert!(counted.contains("interrupted after 12s, ctx 43% (89k/206k)\n"), "{counted}");
    let old = interrupted(None);
    assert!(old.contains("interrupted after 12s\n"), "{old}");
    let done = {
        let mut view = view(RenderOptions::new(SIZE.cols));
        view.envelope(&sent(11, 0, turn_started()), SIZE, false);
        let end = Event::TurnCompleted {
            turn_id: turn(),
            usage: Some(Usage { cached_input_tokens: 16_700, ..Usage::new(18_250, 1_100) }),
            changes: None,
            context: None,
        };
        view.envelope(&sent(12, 42_000, end), SIZE, false);
        bare(&view.frame(SIZE, at(42_000)))
    };
    assert!(done.contains("done in 42s, 18.2k tokens in, 1.1k out, cache 91%\n"), "{done}");
}

#[test]
fn piped_output_writes_the_lines_plain_on_stderr_and_no_gauge() {
    let mut view = view(RenderOptions::new(80).with_terminal(false));
    let mut err = String::new();
    for (seq, event) in (11..).zip([
        turn_started(),
        Event::ConversationCompacted(compaction(CompactionTrigger::Auto, 24_100)),
        Event::ConversationCompacted(compaction(CompactionTrigger::Overflow, 240_000)),
        completed(89_400),
    ]) {
        view.draft(&draft(seq, DraftPart::Context(context(100_000))), SIZE);
        let step = view.envelope(&sent(seq, 0, event), SIZE, false);
        assert_eq!(step.out, "");
        err.push_str(&step.err);
        assert_eq!(view.tick(SIZE, at(100)), "");
    }
    assert_eq!(
        err,
        "context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k\n\
         context full: compaction did not free enough room (still 240k); run ,compact or efr new\n"
    );
}

#[test]
fn a_compaction_of_another_turn_shows_nothing() {
    let mut view = view(RenderOptions::new(SIZE.cols));
    view.envelope(&sent(11, 0, turn_started()), SIZE, false);
    let mut other = compaction(CompactionTrigger::Auto, 24_100);
    other.turn_id = Some("019a9b1c-3d00-7a10-8b20-000000000099".parse().unwrap());
    let step = view.envelope(&sent(12, 0, Event::ConversationCompacted(other)), SIZE, false);
    assert_eq!((step.out.as_str(), step.err.as_str()), ("", ""));
    assert!(!bare(&view.frame(SIZE, at(0))).contains("compacted"));
}
