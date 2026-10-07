use std::time::Duration;

use efr_render::{ColourMode, RenderOptions};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{State, Status, band};
use crate::testing::{now, readable};

/// The time `millis` after the first frame.
fn at(millis: i64) -> Timestamp {
    now() + SignedDuration::from_millis(millis)
}

fn options() -> RenderOptions {
    RenderOptions::new(60)
}

/// A row that started its turn at the first frame.
fn started(motion: bool) -> Status {
    let mut status = Status::new(motion);
    status.turn_started();
    status.at(at(0), false);
    status
}

#[test]
fn the_spinner_turns_one_frame_per_tick_and_the_time_shows_from_one_second() {
    let mut status = started(true);
    let rows: Vec<String> = [0, 100, 250, 999, 1_000, 12_400]
        .into_iter()
        .map(|millis| {
            status.at(at(millis), false);
            readable(&status.row(at(millis), &options()))
        })
        .collect();
    insta::assert_snapshot!(rows.join(""));
}

#[test]
fn without_motion_the_spinner_is_a_still_dot_and_no_band_moves() {
    let mut status = started(false);
    let first = status.row(at(300), &options());
    status.at(at(1_300), false);
    let later = status.row(at(1_300), &options());
    assert!(first.contains('•') && later.contains('•'), "{first:?} {later:?}");
    assert!(!first.contains("\x1b[22m") && !later.contains("\x1b[22m"), "{first:?}");
    assert!(later.ends_with("\x1b[2m1s\x1b[0m\n"), "{later:?}");
}

#[test]
fn the_band_crosses_the_state_one_character_per_tick_then_rests_a_second() {
    assert_eq!(band(0, 5), None);
    assert_eq!(band(1, 5), Some(0..1));
    assert_eq!(band(2, 5), Some(0..2));
    assert_eq!(band(3, 5), Some(0..3));
    assert_eq!(band(4, 5), Some(1..4));
    assert_eq!(band(7, 5), Some(4..5));
    assert_eq!(band(8, 5), None);
    assert_eq!(band(17, 5), None);
    assert_eq!(band(18, 5), None);
    assert_eq!(band(19, 5), Some(0..1));
}

#[test]
fn the_band_is_normal_intensity_inside_the_dim_state() {
    let status = started(true);
    let row = status.row(at(200), &options());
    assert_eq!(
        readable(&row),
        "\\e[33m\u{2839}\\e[0m \\e[2m\\e[22mwa\\e[2miting for the model\\e[0m\n"
    );
}

#[test]
fn every_state_has_its_words() {
    let cases = [
        (State::Queued, "waiting for the running turn"),
        (State::Model, "waiting for the model"),
        (State::Thinking(None), "thinking"),
        (
            State::Thinking(Some("Reading the test output".to_owned())),
            "thinking: Reading the test output",
        ),
        (State::Writing, "writing"),
        (
            State::Preparing { call: 0, tool: "write_file".to_owned(), bytes: 0 },
            "preparing write_file",
        ),
        (
            State::Preparing { call: 1, tool: "write_file".to_owned(), bytes: 3_250 },
            "preparing write_file, 3.2 KB",
        ),
        (State::Tool("shell".to_owned()), "running shell"),
        (State::Answer, "waiting for an answer"),
    ];
    for (state, words) in cases {
        assert_eq!(state.words(Duration::ZERO), words);
    }
}

#[test]
fn twenty_seconds_without_data_while_waiting_or_writing_is_a_stall() {
    let mut status = started(false);
    status.set(State::Writing);
    status.at(at(41_000), false);
    let row = status.row(at(41_000), &RenderOptions::new(80).with_terminal(true));
    assert!(row.contains("waiting for the model, no data for 41s"), "{row:?}");
    status.stir();
    status.at(at(42_000), false);
    assert!(status.row(at(42_000), &options()).contains("writing"));
    status.set(State::Thinking(None));
    status.at(at(70_000), false);
    assert!(status.row(at(70_000), &options()).contains("thinking"), "thinking is no stall");
}

#[test]
fn the_time_stops_while_the_user_is_asked() {
    let mut status = started(false);
    status.at(at(5_000), true);
    status.at(at(65_000), true);
    assert_eq!(status.elapsed(at(65_000)), Duration::from_secs(5));
    status.at(at(66_000), false);
    assert_eq!(status.elapsed(at(70_000)), Duration::from_secs(9));
}

#[test]
fn the_time_counts_from_the_start_of_the_turn() {
    let mut status = Status::new(true);
    status.at(at(0), false);
    assert_eq!(status.elapsed(at(3_000)), Duration::ZERO);
    status.turn_started();
    status.at(at(3_000), false);
    assert_eq!(status.elapsed(at(4_500)), Duration::from_millis(1_500));
}

#[test]
fn a_long_state_is_cut_so_the_row_fits_one_row() {
    let mut status = started(false);
    status.set(State::Thinking(Some("x".repeat(100))));
    status.at(at(12_000), false);
    let row = status.row(at(12_000), &RenderOptions::new(30).with_colour(ColourMode::None));
    assert_eq!(
        efr_render::display_width(row.trim_end(), efr_render::WidthMethod::CodePoint),
        30,
        "{row:?}"
    );
    assert!(row.contains('\u{2026}'), "{row:?}");
}
