//! How full the model's context is, as a followed turn shows it: the gauge in the
//! status row and in the running call's line, `compacting context`, the line of each
//! compaction and the end-of-turn line, against the events and drafts of the context
//! contract (the README of `efr-conversation`, section "Context"). Then the frame of a
//! turn as a whole against the boards of the design: the `⇄` line with the compaction
//! that came first, and the end lines (TM-Endings, TM-LongTurn 3).

use std::path::PathBuf;

use efr_protocol::{
    CompactionTrigger, ContextUse, Draft, DraftPart, EffectiveSettings, ErrorBody, ErrorCode,
    Event, EventEnvelope, Mode, OverriddenSettings, Scope, Seq, StopReason, SwitchedModel,
    TurnStop, Usage,
};
use efr_render::{ColourMode, RenderOptions};
use efr_test_support::design::{Depth, Frame, assert_rows};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{Look, TurnView};
use super::at;
use crate::terminal::Size;
use crate::testing::{Grid, bare, call, compaction, design_options, readable, turn};

const SIZE: Size = Size { cols: 80, rows: 20 };

/// No motion and no progress bar, so a frame shows the same at every tick.
const LOOK: Look = Look {
    motion: false,
    summary: true,
    progress: false,
    diff_lines: 20,
    diff_lines_long: 8,
    bell: false,
};

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
        stop: None,
        undo: None,
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
    assert_eq!(grid.lines(), ["\u{2022} compacting context   1s   ctx 99%"]);
    let event = Event::ConversationCompacted(compaction(CompactionTrigger::Auto, 24_100));
    view.envelope(&sent(12, 6_000, event), SIZE, false);
    let frame = view.frame(SIZE, at(6_000));
    grid.write(&frame);
    assert_eq!(
        grid.lines(),
        [
            "context compacted (auto): 231k \u{2192} 24k tokens \u{b7} kept 3 turns \u{b7} summary 3.2k",
            "\u{2022} waiting for the model   6s   ctx 11%",
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
    assert!(bare(&counted).ends_with("waiting for the model   ctx 14%"), "{counted}");
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
        frame.contains("$\\e[0m cargo test -p app  \\e[2m12s\\e[0m   \\e[31mctx 90%"),
        "{frame}"
    );
    assert!(!frame.contains("waiting for"), "the call's line replaces the row: {frame}");
    // At 30 columns the gauge stays and the command is cut; at 20 the command keeps
    // what room is left, and the gauge goes.
    let frame = bare(&view.tick(Size { cols: 30, rows: 20 }, at(13_400)));
    assert!(frame.contains("$ cargo test\u{2026}  13s   ctx 90%"), "{frame}");
    let frame = bare(&view.tick(Size { cols: 20, rows: 20 }, at(14_400)));
    assert!(frame.contains("$ cargo test\u{2026}  14s") && !frame.contains("ctx"), "{frame}");
}

#[test]
fn the_end_of_the_turn_shows_the_gauge_of_its_event_muted() {
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
        let event = Event::TurnInterrupted {
            turn_id: turn(),
            usage: None,
            context,
            changes: None,
            undo: None,
        };
        view.envelope(&sent(12, 12_400, event), SIZE, false);
        bare(&view.frame(SIZE, at(12_400)))
    };
    let counted = interrupted(Some(context(89_400)));
    assert!(counted.contains("\u{25a0} interrupted  12s \u{b7} ctx 43%\n"), "{counted}");
    let old = interrupted(None);
    assert!(old.contains("\u{25a0} interrupted  12s\n"), "{old}");
    let done = {
        let mut view = view(RenderOptions::new(SIZE.cols));
        view.envelope(&sent(11, 0, turn_started()), SIZE, false);
        let end = Event::TurnCompleted {
            turn_id: turn(),
            usage: Some(Usage { cached_input_tokens: 16_700, ..Usage::new(18_250, 1_100) }),
            changes: None,
            context: None,
            stop: None,
            undo: None,
        };
        view.envelope(&sent(12, 42_000, end), SIZE, false);
        bare(&view.frame(SIZE, at(42_000)))
    };
    // Without a count the line has no gauge, and never the input tokens.
    assert!(done.contains("\u{2713} done  42s \u{b7} 1.1k out \u{b7} cache 91%\n"), "{done}");
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
        "context compacted (auto): 231k \u{2192} 24k tokens \u{b7} kept 3 turns \u{b7} summary 3.2k\n\
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

/// The width of the boards of the design.
const BOARD: Size = Size { cols: 100, rows: 40 };

/// The settings of a turn on `provider`, `model` and `effort`.
fn settings(provider: &str, model: &str, effort: &str) -> EffectiveSettings {
    EffectiveSettings {
        mode: Mode::Auto,
        model: model.to_owned(),
        effort: Some(effort.to_owned()),
        provider: Some(provider.to_owned()),
        overridden: OverriddenSettings::default(),
        fallback: None,
    }
}

fn started_on(settings: Option<EffectiveSettings>) -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/user/app"),
        scope: Scope::Machine,
        settings,
    }
}

fn message(text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id: turn(), index: 0, text: text.to_owned() }
}

fn usage(input: u64, cached: u64, output: u64) -> Usage {
    Usage { cached_input_tokens: cached, ..Usage::new(input, output) }
}

fn done(percent: u64, usage: Usage, stop: Option<StopReason>) -> Event {
    Event::TurnCompleted {
        turn_id: turn(),
        usage: Some(usage),
        changes: None,
        context: Some(ContextUse { tokens: percent * 1_000, limit: 100_000, window: 272_000 }),
        stop: stop.map(|reason| TurnStop { reason, empty: false }),
        undo: None,
    }
}

fn failed(error: ErrorBody) -> Event {
    Event::TurnFailed {
        turn_id: turn(),
        error,
        usage: None,
        context: None,
        changes: None,
        undo: None,
    }
}

fn switched(from: (&str, &str, &str), to: (&str, &str, &str)) -> Event {
    let side = |(provider, model, effort): (&str, &str, &str)| SwitchedModel {
        provider: Some(provider.to_owned()),
        model: model.to_owned(),
        effort: Some(effort.to_owned()),
    };
    Event::ModelSwitched { turn_id: turn(), from: side(from), to: side(to) }
}

/// A turn on a terminal of the boards' width in `depth`, fed `events` at their times
/// (milliseconds after the start), with no status row: one frame at the end writes all
/// that it committed, the scrollback that the user reads.
fn scrollback(depth: Depth, events: &[(i64, Event)]) -> String {
    let mut view = TurnView::new(turn(), design_options(depth, BOARD.cols)).with_look(LOOK);
    let mut last = 0;
    for (seq, (millis, event)) in (11..).zip(events) {
        view.envelope(&sent(seq, *millis, event.clone()), BOARD, false);
        last = *millis;
    }
    view.frame(BOARD, at(last))
}

/// Checks rows `rows` of TM-`board` frame `number` against the scrollback of `events`,
/// in the three depths.
fn board(board: &str, number: u32, rows: std::ops::RangeInclusive<usize>, events: &[(i64, Event)]) {
    in_depths(board, number, rows, events, &Depth::ALL);
}

/// [`board`] in `depths` only.
fn in_depths(
    board: &str,
    number: u32,
    rows: std::ops::RangeInclusive<usize>,
    events: &[(i64, Event)],
    depths: &[Depth],
) {
    let frame = Frame::load(board, number).unwrap();
    for depth in depths {
        assert_rows(&frame, rows.clone(), &scrollback(*depth, events), *depth);
    }
}

/// TM-Endings 16, 18, 19: the reply, one blank line, then the stop reason and the
/// outcome line with no blank line between them. Row 0 of each frame is the blank line
/// above the reply, which belongs to what stood before it.
#[test]
fn a_completed_turn_ends_as_the_endings_board_draws_it() {
    let pushed = "Pushed. `finish()` drains the channel first.\n";
    let sixteen = [
        (0, started_on(None)),
        (100, message(pushed)),
        (3_000, done(6, usage(1_900, 0, 212), None)),
    ];
    // NOTE: without colour, inline code keeps its backticks (TM-Legend 6 and its note),
    // which beats the table of `design::expect` for that depth; the rows of the end are
    // the same.
    in_depths("TM-Endings", 16, 1..=3, &sixteen, &[Depth::TrueColor, Depth::Ansi16]);
    let none = bare(&scrollback(Depth::NoColor, &sixteen));
    assert!(
        none.contains(
            "Pushed. `finish()` drains the channel first.\n\n\u{2713} done  3s \u{b7} ctx 6% \u{b7} 212 out\n"
        ),
        "{none}"
    );
    board(
        "TM-Endings",
        18,
        1..=4,
        &[
            (0, started_on(None)),
            (100, message("The summary so far: the importer reads all 412 files and\n")),
            (242_000, done(88, usage(100_000, 93_000, 32_000), Some(StopReason::MaxTokens))),
        ],
    );
    board(
        "TM-Endings",
        19,
        1..=2,
        &[(0, started_on(None)), (2_000, done(4, usage(1_200, 0, 0), Some(StopReason::Refusal)))],
    );
}

/// TM-Endings 20, rows 1 to 3: the reply that the interrupt cut, then `■ interrupted`
/// with its time and its gauge. The row under it is in `format/ending/tests.rs` and in
/// `tests/input.rs`, where the view has the input row whose steer goes again.
#[test]
fn an_interrupted_turn_ends_as_the_endings_board_draws_it() {
    let interrupted = Event::TurnInterrupted {
        turn_id: turn(),
        usage: None,
        context: Some(ContextUse { tokens: 43_000, limit: 100_000, window: 272_000 }),
        changes: None,
        undo: None,
    };
    board(
        "TM-Endings",
        20,
        1..=3,
        &[
            (0, started_on(None)),
            (100, message("I drain the channel before the end che")),
            (12_000, interrupted),
        ],
    );
}

/// TM-Endings 21, 22: `✗ failed` and its time, then the reason and the command that
/// fixes it, 2 columns in. The login names the provider that ran the turn.
#[test]
fn a_failed_turn_ends_as_the_endings_board_draws_it() {
    let anthropic = Some(settings("anthropic-api", "claude-opus-5-5", "high"));
    let login =
        ErrorBody::new(ErrorCode::Unauthorized, "the login of anthropic-api expired at 14:02");
    board("TM-Endings", 21, 1..=3, &[(0, started_on(anthropic.clone())), (9_000, failed(login))]);
    let full = ErrorBody::new(
        ErrorCode::Internal,
        "the context is full: compaction did not free enough room (still 240k); run ,compact \
         or start a new conversation",
    )
    .with_data(serde_json::json!({ "cause": "context_overflow", "tokens": 240_000 }));
    board("TM-Endings", 22, 1..=3, &[(0, started_on(anthropic)), (31_000, failed(full))]);
}

/// TM-Endings 5 and TM-LongTurn 3, row 1: a switch to a model with a smaller window
/// compacts first, and the `⇄` line carries the counts in place of a notice of its own.
/// The line waits in the live zone until the compaction comes.
#[test]
fn a_switch_carries_the_compaction_that_came_first() {
    let gpt = settings("openai-subscription.b", "gpt-5.5", "medium");
    let events = [
        (0, started_on(Some(gpt))),
        (
            0,
            switched(
                ("anthropic-api.work", "claude-opus-5-5", "high"),
                ("openai-subscription.b", "gpt-5.5", "medium"),
            ),
        ),
        (
            4_000,
            Event::ConversationCompacted(efr_protocol::Compaction {
                tokens_before: 86_400,
                ..compaction(CompactionTrigger::Auto, 6_050)
            }),
        ),
    ];
    board("TM-Endings", 5, 3..=3, &events);

    let work = settings("openai-api.work", "gpt-5.5", "high");
    let events = [
        (0, started_on(Some(work))),
        (
            0,
            switched(
                ("anthropic-api.work", "claude-opus-5-5", "high"),
                ("openai-api.work", "gpt-5.5", "high"),
            ),
        ),
        (
            3_000,
            Event::ConversationCompacted(efr_protocol::Compaction {
                tokens_before: 41_000,
                ..compaction(CompactionTrigger::Auto, 5_000)
            }),
        ),
    ];
    board("TM-LongTurn", 3, 1..=1, &events);

    // On a terminal the line shows in the live zone while the compaction runs.
    let mut view = TurnView::new(turn(), RenderOptions::new(BOARD.cols)).with_look(LOOK);
    view.start();
    view.envelope(&sent(11, 0, started_on(None)), BOARD, false);
    view.envelope(&sent(12, 0, events[1].1.clone()), BOARD, false);
    view.draft(&draft(12, DraftPart::Compacting { trigger: CompactionTrigger::Auto }), BOARD);
    let mut grid = Grid::new(BOARD.cols);
    grid.write(&view.frame(BOARD, at(1_200)));
    assert_eq!(
        grid.lines(),
        [
            "\u{21c4} work/gpt-5.5 high  was work/claude-opus-5-5 high",
            "\u{2022} compacting context",
        ]
    );
}

/// TM-Endings 6: a switch of account only, with no compaction: the line commits before
/// the first block of the reply, `cache starts cold`. Off a terminal it goes to stderr
/// at once.
#[test]
fn a_switch_of_account_commits_before_the_reply() {
    let events = [
        (0, started_on(Some(settings("anthropic-api.personal", "claude-opus-5-5", "high")))),
        (
            0,
            switched(
                ("anthropic-api.work", "claude-opus-5-5", "high"),
                ("anthropic-api.personal", "claude-opus-5-5", "high"),
            ),
        ),
        (500, message("Docs next.\n")),
    ];
    board("TM-Endings", 6, 1..=1, &events[..2]);
    let shown = bare(&scrollback(Depth::NoColor, &events));
    assert!(
        shown.contains("cache starts cold\n\nDocs next.\n"),
        "the line, then the blank line between rows and prose: {shown}"
    );
    let mut view = TurnView::new(turn(), RenderOptions::new(80).with_terminal(false));
    let mut err = String::new();
    for (seq, (millis, event)) in (11..).zip(&events[..2]) {
        err.push_str(&view.envelope(&sent(seq, *millis, event.clone()), BOARD, false).err);
    }
    assert_eq!(
        err,
        "\u{21c4} personal/claude-opus-5-5 high  was work/claude-opus-5-5 high \u{b7} cache starts cold\n"
    );
}

/// TM-Endings 10, row 0: a compaction with no switch before it leaves its own notice.
#[test]
fn a_compaction_alone_is_the_notice_of_the_endings_board() {
    let events = [
        (0, started_on(None)),
        (4_000, Event::ConversationCompacted(compaction(CompactionTrigger::Auto, 24_100))),
    ];
    board("TM-Endings", 10, 0..=0, &events);
}

/// The settings of the `prompt.send` result name the model before the turn starts, for
/// the first frame of the status row (TM-Sequence 1); `turn_started` then has the last
/// word.
#[test]
fn the_view_knows_the_settings_of_its_turn_from_the_start() {
    let sent_settings = settings("anthropic-api.work", "claude-opus-5-5", "high");
    let mut view = TurnView::new(turn(), RenderOptions::new(BOARD.cols))
        .with_look(LOOK)
        .with_settings(Some(sent_settings.clone()));
    assert_eq!(view.turn_settings(), Some(&sent_settings));
    // A `turn_started` with no settings, from an older efrd, keeps them.
    view.envelope(&sent(11, 0, started_on(None)), BOARD, false);
    assert_eq!(view.turn_settings(), Some(&sent_settings));
    let started = settings("openai-subscription.b", "gpt-5.5", "medium");
    let mut view = TurnView::new(turn(), RenderOptions::new(BOARD.cols)).with_look(LOOK);
    view.envelope(&sent(11, 0, started_on(Some(started.clone()))), BOARD, false);
    assert_eq!(view.turn_settings(), Some(&started));
}
