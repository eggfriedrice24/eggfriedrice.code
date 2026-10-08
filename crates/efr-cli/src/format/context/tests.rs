use efr_protocol::{Compaction, CompactionTrigger, ContextUse};
use efr_render::{ColourMode, RenderOptions, Role};
use pretty_assertions::assert_eq;

use super::{Gauge, compacted, count};
use crate::testing::compaction;

fn context(tokens: u64, limit: u64) -> ContextUse {
    ContextUse { tokens, limit, window: 272_000 }
}

fn gauge(tokens: u64, limit: u64) -> Gauge {
    Gauge::of(&context(tokens, limit)).unwrap()
}

#[test]
fn the_gauge_is_a_percent_of_the_limit_rounded_down() {
    assert_eq!(gauge(0, 206_720).text(), "ctx 0%");
    assert_eq!(gauge(89_400, 206_720).text(), "ctx 43%");
    assert_eq!(gauge(206_719, 206_720).text(), "ctx 99%");
    assert_eq!(gauge(206_720, 206_720).text(), "ctx 100%");
    assert_eq!(gauge(240_000, 206_720).text(), "ctx 116%");
    assert_eq!(Gauge::of(&context(10, 0)), None, "no limit, no gauge");
    assert_eq!(gauge(u64::MAX, 1).text(), format!("ctx {}%", u64::MAX), "no overflow");
}

#[test]
fn the_level_of_the_gauge_picks_its_role() {
    let roles: Vec<(u64, Role)> =
        [0, 49, 50, 89, 90, 100, 130].into_iter().map(|p| (p, gauge(p, 100).role())).collect();
    assert_eq!(
        roles,
        [
            (0, Role::Success),
            (49, Role::Success),
            (50, Role::Warning),
            (89, Role::Warning),
            (90, Role::Error),
            (100, Role::Error),
            (130, Role::Error),
        ]
    );
}

#[test]
fn the_gauge_has_the_colour_of_its_level_and_never_the_bold_of_a_warning() {
    let options = RenderOptions::new(80);
    assert_eq!(gauge(43, 100).paint(&options), "\x1b[32mctx 43%\x1b[0m");
    assert_eq!(gauge(60, 100).paint(&options), "\x1b[33mctx 60%\x1b[0m");
    assert_eq!(gauge(95, 100).paint(&options), "\x1b[31mctx 95%\x1b[0m");
    // NO_COLOR: only the top level stands out, in bold.
    let none = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(gauge(43, 100).paint(&none), "ctx 43%");
    assert_eq!(gauge(60, 100).paint(&none), "ctx 60%");
    assert_eq!(gauge(95, 100).paint(&none), "\x1b[1mctx 95%\x1b[0m");
    let piped = RenderOptions::new(80).with_terminal(false);
    assert_eq!(gauge(95, 100).paint(&piped), "ctx 95%");
}

#[test]
fn counts_are_short_and_cut() {
    let shown: Vec<String> =
        [0, 999, 1_000, 3_250, 9_999, 10_000, 24_400, 206_720, 999_999, 1_250_000]
            .into_iter()
            .map(count)
            .collect();
    assert_eq!(shown, ["0", "999", "1.0k", "3.2k", "9.9k", "10k", "24k", "206k", "999k", "1.2M"]);
}

#[test]
fn each_compaction_has_its_line() {
    use CompactionTrigger::{Auto, Manual, Overflow};
    let pruned = Compaction {
        summary: None,
        usage: None,
        pruned_outputs: 12,
        pruned_tokens: 48_000,
        kept_turns: 1,
        ..compaction(Auto, 180_200)
    };
    let estimated =
        Compaction { usage: None, summary: Some("x".repeat(12_801)), ..compaction(Manual, 19_000) };
    let lines = [
        compacted(&compaction(Auto, 24_100)),
        compacted(&compaction(Manual, 19_000)),
        compacted(&pruned),
        compacted(&estimated),
        compacted(&Compaction { tokens_before: 281_300, ..compaction(Overflow, 24_100) }),
        compacted(&compaction(Auto, 240_000)),
        compacted(&compaction(Overflow, 206_720)),
        compacted(&compaction(Manual, 240_000)),
        compacted(&Compaction { kept_turns: 0, ..compaction(Manual, 19_000) }),
    ];
    assert_eq!(
        lines,
        [
            "context compacted (auto): 231k -> 24k tokens, kept 3 turns, summary 3.2k",
            "context compacted (efr compact): 231k -> 19k tokens, kept 3 turns, summary 3.2k",
            "context compacted (auto): 231k -> 180k tokens, kept 1 turn, pruned 12 outputs",
            "context compacted (efr compact): 231k -> 19k tokens, kept 3 turns, summary 3.2k",
            "context full: the request was 281k of 272k tokens; compacted and retried",
            "context full: compaction did not free enough room (still 240k); run ,compact or efr new",
            "context full: compaction did not free enough room (still 206k); run ,compact or efr new",
            "context full: compaction did not free enough room (still 240k); run efr new",
            "context compacted (efr compact): 231k -> 19k tokens, summary 3.2k",
        ]
    );
}
