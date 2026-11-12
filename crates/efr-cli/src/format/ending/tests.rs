use std::time::Duration;

use efr_protocol::{ContextUse, ErrorBody, ErrorCode, StopReason, TurnStop, Usage};
use efr_render::Glyphs;
use efr_test_support::design::{Depth, Frame, assert_rows};
use pretty_assertions::assert_eq;

use super::{
    UNDER, done_line, failed_line, failure_rows, interrupted_line, plain, resent_row, rows,
    stop_line, usage_line,
};
use crate::error::turn_failure;
use crate::format::Tone;
use crate::format::route::Pieces;
use crate::testing::design_options;

const G: &Glyphs = &Glyphs::UNICODE;

/// Lines of the end of a turn with their indents.
type Lines = Vec<(usize, Pieces)>;

fn secs(seconds: u64) -> Option<Duration> {
    Some(Duration::from_secs(seconds))
}

fn ctx(percent: u64) -> ContextUse {
    ContextUse { tokens: percent * 1_000, limit: 100_000, window: 272_000 }
}

fn usage(input: u64, cached: u64, output: u64) -> Usage {
    let mut usage = Usage::new(input, output);
    usage.cached_input_tokens = cached;
    usage
}

fn stop(reason: StopReason) -> TurnStop {
    TurnStop { reason, empty: false }
}

/// The rows of `lines` at their indents, as the view writes them.
fn written(lines: &[(usize, Pieces)], depth: Depth, cols: u16) -> String {
    let options = design_options(depth, cols);
    lines.iter().map(|(indent, line)| rows(line, *indent, &options)).collect()
}

/// Checks rows `range` of TM-Endings `number` against `lines`, in the three depths.
fn endings(number: u32, range: std::ops::RangeInclusive<usize>, lines: &[(usize, Pieces)]) {
    let frame = Frame::load("TM-Endings", number).unwrap();
    for depth in Depth::ALL {
        assert_rows(&frame, range.clone(), &written(lines, depth, frame.cols), depth);
    }
}

/// TM-Endings 15 to 17: the outcome line of a completed turn, `✓` in `success`,
/// `done` in `text`, the cost muted after two spaces, the gauge muted too.
#[test]
fn a_done_turn_is_the_endings_board() {
    let done =
        |took, percent, usage| (0, done_line(secs(took), Some(&usage), Some(&ctx(percent)), G));
    endings(15, 4..=4, &[done(42, 43, usage(12_000, 10_920, 1_100))]);
    endings(16, 3..=3, &[done(3, 6, usage(1_900, 0, 212))]);
    endings(17, 5..=5, &[done(130, 51, usage(20_000, 18_000, 6_300))]);
    endings(18, 4..=4, &[done(242, 88, usage(100_000, 93_000, 32_000))]);
}

/// TM-Legend 35 at its 44 columns.
#[test]
fn the_outcome_line_is_the_legend() {
    let line = done_line(secs(42), Some(&usage(1_500, 0, 1_100)), Some(&ctx(43)), G);
    let frame = Frame::load("TM-Legend", 35).unwrap();
    for depth in Depth::ALL {
        assert_rows(&frame, .., &written(&[(0, line.clone())], depth, frame.cols), depth);
    }
}

/// TM-Endings 18, 19: the stop reason right before the outcome line, `!` and `stopped:`
/// in `warning`, the sentence in `text`, the reason muted.
#[test]
fn a_stopped_answer_is_the_endings_board() {
    let usage_19 = usage(1_200, 0, 0);
    endings(
        18,
        3..=4,
        &[
            (0, stop_line(&stop(StopReason::MaxTokens), G)),
            (0, done_line(secs(242), Some(&usage(100_000, 93_000, 32_000)), Some(&ctx(88)), G)),
        ],
    );
    endings(
        19,
        1..=2,
        &[
            (0, stop_line(&stop(StopReason::Refusal), G)),
            (0, done_line(secs(2), Some(&usage_19), Some(&ctx(4)), G)),
        ],
    );
}

/// TM-Endings 20: `■` in `warning`, `interrupted` in `text`, then what goes again, 2
/// columns in, muted.
#[test]
fn an_interrupted_turn_is_the_endings_board() {
    endings(
        20,
        3..=4,
        &[
            (0, interrupted_line(secs(12), Some(&ctx(43)), G)),
            (UNDER, resent_row("use TurnState", G.more)),
        ],
    );
}

/// TM-Endings 21, 22: `✗ failed` in `error` bold with its time, then the reason in
/// `text` and the command that fixes it in `code`, 2 columns in; `,compact or efr new`
/// with a muted `or`.
#[test]
fn a_failed_turn_is_the_endings_board() {
    let login =
        ErrorBody::new(ErrorCode::Unauthorized, "the login of anthropic-api expired at 14:02");
    let (reason, fix) = turn_failure(&login, "anthropic-api");
    let mut lines = vec![(0, failed_line(secs(9), G))];
    lines.extend(failure_rows(&reason, fix.as_ref()).into_iter().map(|row| (UNDER, row)));
    endings(21, 1..=3, &lines);

    let full = ErrorBody::new(
        ErrorCode::Internal,
        "the context is full: compaction did not free enough room (still 240k); run ,compact \
         or start a new conversation",
    )
    .with_data(serde_json::json!({ "cause": "context_overflow" }));
    let (reason, fix) = turn_failure(&full, "anthropic-api");
    let mut lines = vec![(0, failed_line(secs(31), G))];
    lines.extend(failure_rows(&reason, fix.as_ref()).into_iter().map(|row| (UNDER, row)));
    endings(22, 1..=3, &lines);
}

/// TM-Legend 36: the fix alone on its row, 2 columns in, in `code`.
#[test]
fn the_fix_is_the_legend() {
    let body = ErrorBody::new(ErrorCode::Unauthorized, "no key");
    let (_, fix) = turn_failure(&body, "anthropic-api");
    let frame = Frame::load("TM-Legend", 36).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        assert_rows(&frame, .., &rows(&fix.as_ref().unwrap().pieces(), UNDER, &options), depth);
    }
}

#[test]
fn the_cost_leaves_out_what_is_not_known() {
    assert_eq!(plain(&done_line(None, None, None, G)), "✓ done");
    assert_eq!(plain(&done_line(secs(64), None, None, G)), "✓ done  1m 04s");
    assert_eq!(
        plain(&done_line(None, Some(&usage(900, 0, 1_234)), None, &Glyphs::ASCII)),
        "+ done  1.2k out"
    );
    assert_eq!(plain(&interrupted_line(None, None, G)), "■ interrupted");
    assert_eq!(plain(&interrupted_line(None, None, &Glyphs::ASCII)), "# interrupted");
    assert_eq!(plain(&failed_line(None, &Glyphs::ASCII)), "x failed");
    // The tenths of a count of tokens show unless they are 0.
    let big = usage(10_000, 0, 32_050);
    assert_eq!(plain(&done_line(None, Some(&big), None, G)), "✓ done  32k out \u{b7} cache 0%");
}

#[test]
fn the_cache_share_shows_from_a_large_enough_input_and_is_rounded_down() {
    let share = |input, cached| {
        let line = plain(&done_line(None, Some(&usage(input, cached, 1_100)), None, G));
        line.strip_prefix("✓ done  1.1k out").unwrap().to_owned()
    };
    assert_eq!(share(18_250, 16_700), " \u{b7} cache 91%");
    // Rounded down, never up to a full cache.
    assert_eq!(share(10_000, 9_999), " \u{b7} cache 99%");
    assert_eq!(share(10_000, 10_000), " \u{b7} cache 100%");
    // A count that says more was cached than sent stays at 100%.
    assert_eq!(share(4_000, 9_000), " \u{b7} cache 100%");
    // From 2048 tokens on; below, no cache can form.
    assert_eq!(share(super::CACHE_SHOWN_FROM, 1_024), " \u{b7} cache 50%");
    assert_eq!(share(super::CACHE_SHOWN_FROM - 1, 1_024), "");
    // A turn whose input went to the cache only as a write read nothing from it.
    let mut written = usage(18_250, 0, 1_100);
    written.cache_write_tokens = 18_000;
    assert_eq!(
        plain(&done_line(None, Some(&written), None, G)),
        "✓ done  1.1k out \u{b7} cache 0%"
    );
}

#[test]
fn a_stop_says_why_in_words_and_keeps_the_reason_of_the_provider() {
    let empty = TurnStop { reason: StopReason::EndTurn, empty: true };
    assert_eq!(
        plain(&stop_line(&empty, G)),
        "! stopped: the model stopped with no answer (end_turn)"
    );
    assert_eq!(
        plain(&stop_line(&stop(StopReason::ContentFilter), G)),
        "! stopped: the provider stopped the answer for its content (content_filter)"
    );
}

#[test]
fn what_goes_again_names_the_first_line_of_the_steer() {
    let row = |text: &str| plain(&resent_row(text, G.more));
    assert_eq!(row("use TurnState"), "your steer \"use TurnState\" goes again as a new prompt");
    assert_eq!(row("first\nsecond"), "your steer \"first\u{2026}\" goes again as a new prompt");
    assert_eq!(row(""), "your steer goes again as a new prompt");
    assert_eq!(resent_row("x", G.more)[0].1, Tone::Dim);
}

#[test]
fn a_long_line_goes_on_two_columns_further_in() {
    let options = design_options(Depth::NoColor, 20).with_terminal(true);
    let line = vec![("one two three four five six".to_owned(), Tone::Text)];
    assert_eq!(rows(&line, UNDER, &options), "  one two three four\n    five six\n");
    let piped = options.clone().with_terminal(false);
    assert_eq!(rows(&line, UNDER, &piped), "  one two three four five six\n");
}

#[test]
fn the_tokens_of_history_keep_their_form() {
    let mut usage = usage(18_200, 16_562, 1_100);
    usage.cache_write_tokens = 1_200;
    usage.cache_write_1h_tokens = 1_200;
    assert_eq!(usage_line(&usage), "18.2k in, 1.1k out, cache 91%, wrote 1.2k (1h)");
}

/// The outcome lines of the other boards: TM-BeforeAfter 3 and 4, TM-Sequence 13,
/// TM-LongTurn 3 and the four turns of TM-Scope 21.
#[test]
fn the_outcome_lines_of_the_other_boards() {
    let done = |took, percent, usage: Usage| {
        vec![(0, done_line(secs(took), Some(&usage), Some(&ctx(percent)), G))]
    };
    let cases: [(&str, u32, usize, Lines); 8] = [
        ("TM-BeforeAfter", 3, 4, done(3, 6, usage(1_500, 0, 212))),
        ("TM-BeforeAfter", 4, 35, done(102, 43, usage(80_000, 72_800, 4_200))),
        ("TM-Sequence", 13, 6, done(102, 43, usage(80_000, 72_800, 4_200))),
        ("TM-LongTurn", 3, 76, done(372, 71, usage(150_000, 132_000, 18_400))),
        ("TM-Scope", 21, 6, done(14, 9, usage(1_500, 0, 640))),
        ("TM-Scope", 21, 14, done(6, 14, usage(1_500, 0, 180))),
        ("TM-Scope", 21, 22, done(5, 16, usage(1_500, 0, 120))),
        ("TM-Scope", 21, 30, done(19, 7, usage(1_500, 0, 820))),
    ];
    for (board, number, row, lines) in cases {
        let frame = Frame::load(board, number).unwrap();
        for depth in Depth::ALL {
            assert_rows(&frame, row..=row, &written(&lines, depth, frame.cols), depth);
        }
    }
}

/// TM-Endings 17, row 4: a file at home that undo reverts has its own muted line, its
/// parts apart by the separator.
#[test]
fn a_file_at_home_is_the_endings_board() {
    use std::path::Path;

    let line = crate::format::sandbox::home_undo_line(
        Path::new("/home/u/.zshrc"),
        None,
        true,
        true,
        None,
        Some(Path::new("/home/u")),
        G.separator,
    )
    .unwrap();
    endings(17, 4..=4, &[(0, vec![(line, Tone::Dim)])]);
}
