//! The status row against the frames of the design (TM-Live, TM-Legend 30, TM-Scope 7
//! and 11): the text, the role and the attributes of each cell, in truecolor with the
//! palette of the design, in the 16 colours and without colour. Each frame shows the
//! band over the 3rd to the 5th character of the state, so each row is drawn at that
//! tick.

use std::time::Duration;

use efr_protocol::{ContextUse, EffectiveSettings, Mode, OverriddenSettings};
use efr_test_support::design::{Depth, Frame, assert_rows};
use jiff::SignedDuration;

use super::super::{Model, State, Status};
use crate::testing::{design_options, now};

/// What a row of a frame shows.
#[derive(Clone)]
struct Shown {
    state: State,
    motion: bool,
    /// The time the turn has run, in seconds.
    secs: u64,
    /// The seconds since the model sent something last.
    silent: Option<u64>,
    /// How full the context is, in percent.
    percent: Option<u64>,
    model: Option<Model>,
    scope: Option<&'static str>,
    full: bool,
    away: bool,
    /// The tick of the band's cycle: the band ends after this character.
    band: i64,
}

impl Shown {
    fn new(state: State, secs: u64) -> Shown {
        Shown {
            state,
            motion: true,
            secs,
            silent: None,
            percent: None,
            model: Some(work()),
            scope: Some("app"),
            full: false,
            away: false,
            band: 5,
        }
    }

    fn ctx(mut self, percent: u64) -> Shown {
        self.percent = Some(percent);
        self
    }

    /// The row at the depth of `options`: the frame at the tick at which the band covers
    /// the 3rd to the 5th character of the state.
    fn row(&self, options: &efr_render::RenderOptions) -> String {
        let mut status = Status::new(self.motion);
        status.at(now(), false);
        let length = state_length(&self.state);
        // The frame of the boards: the band over characters 2..5, at the 5th tick of its
        // cycle.
        let secs = i64::try_from(self.secs).unwrap();
        let least = (secs + self.silent.map_or(0, |s| i64::try_from(s).unwrap())) * 10 + 1;
        let cycle = i64::try_from(length + 13).unwrap();
        let ticks = (least..).find(|t| t % cycle == self.band).unwrap();
        let frame = ticks * 100;
        let at = |millis: i64| now() + SignedDuration::from_millis(millis);
        status.turn_started();
        status.at(at(frame - secs * 1_000), false);
        if let Some(silent) = self.silent {
            status.stir();
            status.at(at(frame - i64::try_from(silent).unwrap() * 1_000), false);
        }
        if self.silent.is_none() {
            status.stir();
        }
        status.set(self.state.clone());
        if let Some(percent) = self.percent {
            status.set_context(ContextUse {
                tokens: percent * 1_000,
                limit: 100_000,
                window: 200_000,
            });
        }
        status.set_model(self.model.clone());
        status.set_scope(self.scope.map(str::to_owned));
        status.set_full(self.full);
        status.set_away(self.away);
        status.at(at(frame), false);
        board_spinner(&status.row(at(frame), options), options)
    }
}

/// `row` with the spinner frame that the boards draw, `⠼`. NOTE: a board draws one
/// frame of the animation, and the spinner and the band move with the same ticks, so no
/// tick shows `⠼` and the band of each frame at once. The test checks that the cell is
/// a frame of the spinner, then compares its role and its attributes with the board's.
fn board_spinner(row: &str, options: &efr_render::RenderOptions) -> String {
    let glyphs = options.glyphs();
    let drawn = glyphs.spinner[4];
    if row.contains(glyphs.still) {
        return row.to_owned();
    }
    let frame = glyphs.spinner.iter().find(|frame| row.contains(**frame)).unwrap();
    row.replacen(frame, drawn, 1)
}

/// The characters of the words of `state` as the row shows them.
fn state_length(state: &State) -> usize {
    state.words(Duration::ZERO, Duration::from_secs(5)).chars().count()
}

/// The model of the frames: `work/claude-opus-5-5 high`.
fn work() -> Model {
    Model {
        account: Some("work".to_owned()),
        name: "claude-opus-5-5".to_owned(),
        effort: Some("high".to_owned()),
    }
}

/// Checks the rows of frame `number` of `board`, each drawn from what `shown` gives, in
/// each depth.
fn check(board: &str, number: u32, shown: &[Shown]) {
    let frame = Frame::load(board, number).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let rows: String = shown.iter().map(|shown| shown.row(&options)).collect();
        assert_rows(&frame, .., &rows, depth);
    }
}

#[test]
fn waiting_for_the_model_is_live_1() {
    check("Live", 1, &[Shown::new(State::Model, 2).ctx(12)]);
}

/// TM-Live 1 for a turn that sends no effort: the model's default effort shows, as
/// `,model` names it, so the field is always the model and its effort.
#[test]
fn a_turn_on_the_default_effort_names_that_effort_live_1() {
    let settings = EffectiveSettings {
        mode: Mode::Cautious,
        model: "claude-opus-5-5".to_owned(),
        effort: None,
        default_effort: Some("high".to_owned()),
        provider: Some("anthropic-api.work".to_owned()),
        overridden: OverriddenSettings::default(),
        fallback: None,
    };
    let shown = Shown { model: Some(Model::of(&settings)), ..Shown::new(State::Model, 2).ctx(12) };
    check("Live", 1, &[shown]);
}

#[test]
fn twenty_seconds_without_data_is_a_warning_note_live_2() {
    let shown = Shown { silent: Some(25), ..Shown::new(State::Model, 47).ctx(12) };
    check("Live", 2, &[shown]);
}

#[test]
fn the_title_of_the_reasoning_ends_the_row_live_3() {
    // NOTE: the frame's live line of the reasoning above the row is gone (DECISIONS.md
    // 2, "No live line for the reasoning"); the fixture drops it and moves the title to
    // the end of the row, after the scope.
    let state = State::Thinking(Some("planning the migration".to_owned()));
    check("Live", 3, &[Shown::new(state, 9).ctx(12)]);
}

#[test]
fn writing_is_live_4() {
    check("Live", 4, &[Shown::new(State::Writing, 21).ctx(31)]);
}

#[test]
fn the_size_of_a_tool_input_is_the_detail_live_5() {
    let state = State::Preparing { call: 0, tool: "write".to_owned(), bytes: 3_250, words: None };
    check("Live", 5, &[Shown::new(state, 33).ctx(31)]);
}

#[test]
fn what_the_reviewer_reviews_is_the_note_live_6() {
    let state = State::Reviewing("reviewing: write ~/.local/share/efr/sessions.db".to_owned());
    check("Live", 6, &[Shown::new(state, 40).ctx(31)]);
}

#[test]
fn compacting_is_live_7() {
    check("Live", 7, &[Shown::new(State::Compacting, 62).ctx(100)]);
}

#[test]
fn a_queued_turn_shows_no_gauge_and_no_model_live_8() {
    check("Live", 8, &[Shown::new(State::Queued, 12).ctx(12)]);
}

#[test]
fn away_is_a_warning_note_live_10() {
    check("Live", 10, &[Shown { away: true, ..Shown::new(State::Writing, 130).ctx(43) }]);
}

#[test]
fn full_follows_the_scope_live_11() {
    check("Live", 11, &[Shown { full: true, ..Shown::new(State::Writing, 12).ctx(43) }]);
}

#[test]
fn home_and_its_directory_end_the_row_before_the_detail_live_12() {
    let state = State::Thinking(Some("reading the ruleset".to_owned()));
    let shown = Shown { scope: Some("\u{2302} /etc/nftables.d"), ..Shown::new(state, 3).ctx(8) };
    check("Live", 12, &[shown]);
}

#[test]
fn without_motion_the_spinner_is_a_still_dot_live_14() {
    // NOTE: the frame names the project `efr`; the fixture writes `app` (spec 2.2
    // item 6).
    let state = State::Thinking(Some("planning the migration".to_owned()));
    check("Live", 14, &[Shown { motion: false, ..Shown::new(state, 9).ctx(12) }]);
}

#[test]
fn every_field_shows_at_160_and_100_columns_live_25_26() {
    let state = State::Thinking(Some("planning the migration".to_owned()));
    check("Live", 25, &[Shown::new(state.clone(), 9).ctx(12)]);
    check("Live", 26, &[Shown::new(state, 9).ctx(12)]);
}

#[test]
fn at_60_columns_the_detail_then_the_account_and_the_effort_go_live_27() {
    let state = State::Thinking(Some("planning the migration".to_owned()));
    check("Live", 27, &[Shown::new(state, 9).ctx(12)]);
}

#[test]
fn the_scope_and_full_never_go_and_a_directory_is_cut_from_the_left_live_28() {
    let full = Shown { full: true, ..Shown::new(State::Writing, 12).ctx(43) };
    let home = Shown {
        scope: Some("\u{2302} /etc/nftables.d"),
        ..Shown::new(State::Thinking(None), 3).ctx(8)
    };
    check("Live", 28, &[full, home]);
}

#[test]
fn at_40_columns_the_model_then_the_gauge_go_live_30() {
    let thinking = Shown::new(State::Thinking(None), 9).ctx(12);
    let full = Shown { full: true, ..Shown::new(State::Writing, 12).ctx(43) };
    check("Live", 30, &[thinking, full]);
}

#[test]
fn the_status_row_is_the_legend_30() {
    // The Legend draws the row without a model and a scope, with the band one
    // character further.
    let thinking = Shown::new(State::Thinking(None), 9).ctx(12);
    let shown = Shown { model: None, scope: None, band: 6, ..thinking };
    check("Legend", 30, &[shown]);
}

#[test]
fn the_first_frame_names_the_model_before_any_time_sequence_1() {
    // Sequence 1: the row before any event, with no time and no gauge yet. The rows of
    // the prompt line and of the input row are other slices' and the input row's own.
    let frame = Frame::load("Sequence", 1).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let shown = Shown::new(State::Model, 0);
        assert_rows(&frame, 1..2, &shown.row(&options), depth);
    }
}

#[test]
fn the_status_row_under_a_prompt_line_is_scope_7_and_11() {
    // The rows around it are the prompt lines and the event lines of other slices.
    let checks = [
        (7, Shown::new(State::Thinking(None), 2).ctx(9)),
        (
            11,
            Shown {
                model: Some(Model {
                    account: None,
                    name: "claude-opus-5-5".to_owned(),
                    effort: Some("high".to_owned()),
                }),
                scope: Some("\u{2302} /etc/nftables.d"),
                ..Shown::new(State::Thinking(None), 3).ctx(8)
            },
        ),
    ];
    for (number, shown) in checks {
        let frame = Frame::load("Scope", number).unwrap();
        let live = frame.rows.iter().position(|row| row.live).unwrap();
        for depth in Depth::ALL {
            let options = design_options(depth, frame.cols);
            assert_rows(&frame, live..=live, &shown.row(&options), depth);
        }
    }
}
