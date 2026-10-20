use std::time::Duration;

use efr_render::{Colour, ColourMode, Palette, RenderOptions, Role};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{State, Status, band};
use crate::testing::{bare, now, readable};

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
    let still = "\x1b[2mwaiting for the model\x1b[0m";
    assert!(first.contains(still) && later.contains(still), "{first:?} {later:?}");
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
        "\\e[33m\u{2839}\\e[0m \\e[2m\\e[0mwa\\e[0;2miting for the model\\e[0m\n"
    );
}

#[test]
fn the_band_leaves_a_muted_colour_for_the_text_role() {
    let status = started(true);
    let palette = Palette::new()
        .with(Role::Muted, Colour::Rgb(0xa8, 0xa3, 0x96))
        .with(Role::Text, Colour::Rgb(0xe8, 0xe2, 0xd4));
    let options = options().with_colour(ColourMode::TrueColor).with_palette(palette);
    assert_eq!(
        readable(&status.row(at(200), &options)),
        "\\e[33m\u{2839}\\e[0m \\e[38;2;168;163;150m\\e[0;38;2;232;226;212mwa\\e[0;38;2;168;163;150miting for the model\\e[0m\n"
    );
    // A muted colour without a text colour: the band is the terminal's own foreground.
    let palette = Palette::new().with(Role::Muted, Colour::Palette(8));
    let row = status.row(at(200), &self::options().with_palette(palette));
    assert!(readable(&row).contains("\\e[90m\\e[0mwa\\e[0;90miting"), "{row:?}");
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

#[test]
fn status_row_shows_full_before_away() {
    let palette = Palette::new()
        .with(Role::Error, Colour::Rgb(0xe0, 0x6c, 0x75))
        .with(Role::Warning, Colour::Palette(5));
    let options = RenderOptions::new(60).with_colour(ColourMode::TrueColor).with_palette(palette);
    let mut status = started(false);
    status.set_full(true);
    status.set_away(true);
    status.at(at(2_000), false);
    let row = status.row(at(2_000), &options);
    let full = "  \x1b[38;2;224;108;117mfull\x1b[0m";
    let away = "  \x1b[1;35maway\x1b[0m";
    assert!(row.ends_with(&format!("{full}{away}\n")), "{}", readable(&row));
    // Without colour the tag stays, as the other roles do, and a turn that is not full
    // has none.
    let plain = RenderOptions::new(60).with_colour(ColourMode::None);
    assert!(bare(&status.row(at(2_000), &plain)).ends_with("2s  full  away\n"));
    status.set_full(false);
    assert!(bare(&status.row(at(2_000), &plain)).ends_with("2s  away\n"));
}

#[test]
fn the_full_tag_counts_in_the_width_of_the_row() {
    let mut status = started(false);
    status.set_full(true);
    status.set_away(true);
    status.set(State::Thinking(Some("x".repeat(100))));
    status.at(at(12_000), false);
    let row = status.row(at(12_000), &RenderOptions::new(40).with_colour(ColourMode::None));
    assert_eq!(
        efr_render::display_width(row.trim_end(), efr_render::WidthMethod::CodePoint),
        40,
        "{row:?}"
    );
    assert!(bare(&row).ends_with("12s  full  away\n"), "{row:?}");
}

/// A row at 4 s into the turn, thinking, with the context 43% full and `scope` at its
/// end, `width` columns wide and without colour.
fn scoped(scope: &str, width: u16) -> String {
    let mut status = started(false);
    status.set(State::Thinking(None));
    status.set_context(efr_protocol::ContextUse {
        tokens: 43_000,
        limit: 100_000,
        window: 272_000,
    });
    status.set_scope(Some(scope.to_owned()));
    status.at(at(4_000), false);
    bare(&status.row(at(4_000), &RenderOptions::new(width).with_colour(ColourMode::None)))
}

#[test]
fn status_row_names_the_project() {
    insta::assert_snapshot!(scoped("app", 60), @"• thinking  4s  ctx 43%  app");
}

#[test]
fn status_row_names_no_project() {
    insta::assert_snapshot!(scoped("○ ~/notes", 60), @"• thinking  4s  ctx 43%  ○ ~/notes");
}

#[test]
fn status_row_drops_the_scope_first_when_narrow() {
    // 32 columns hold the state, the time and the gauge, but not the scope too.
    insta::assert_snapshot!(scoped("○ ~/notes", 32), @"• thinking  4s  ctx 43%");
    // Narrower still, the gauge goes too, and what the turn does stays.
    insta::assert_snapshot!(scoped("○ ~/notes", 20), @"• thinking  4s");
    // The scope is muted, as every routing line is.
    let palette = Palette::new().with(Role::Muted, Colour::Palette(8));
    let options = RenderOptions::new(60).with_colour(ColourMode::TrueColor).with_palette(palette);
    let mut status = started(false);
    status.set_scope(Some("app".to_owned()));
    status.at(at(2_000), false);
    let row = status.row(at(2_000), &options);
    let muted = options.paint(Role::Muted, "app");
    assert!(row.ends_with(&format!("  {muted}\n")), "{}", readable(&row));
}
