use efr_protocol::{
    CacheSummary, CompactionBrief, CompactionSummary, CompactionTrigger, ContextItem, ContextPart,
    ContextPartKind, ConversationContextResult, CountedUse, Usage,
};
use efr_render::RenderOptions;
use jiff::SignedDuration;
use pretty_assertions::assert_eq;

use super::lines;
use crate::testing::{now, readable, turn};

fn part(kind: ContextPartKind, tokens: u64, count: u32) -> ContextPart {
    ContextPart { kind, tokens, count }
}

fn item(kind: ContextPartKind, tokens: u64, turn: u32, tool: &str, label: &str) -> ContextItem {
    ContextItem {
        kind,
        tokens,
        turn: Some(turn),
        tool: (!tool.is_empty()).then(|| tool.to_owned()),
        label: (!label.is_empty()).then(|| label.to_owned()),
    }
}

/// A conversation of 14 turns with one auto compaction, on a model with a 1M window.
fn report() -> ConversationContextResult {
    ConversationContextResult {
        model: "claude-opus-5-5".to_owned(),
        window: 1_000_000,
        window_known: true,
        trigger: 760_000,
        hard_cap: 950_000,
        auto: true,
        counted: Some(CountedUse {
            tokens: 98_400,
            turn_id: turn(),
            at: now() - SignedDuration::from_mins(3),
        }),
        estimate: 96_700,
        running: false,
        parts: vec![
            part(ContextPartKind::System, 6_100, 1),
            part(ContextPartKind::Tools, 4_800, 9),
            part(ContextPartKind::Summary, 3_200, 2),
            part(ContextPartKind::Prompts, 2_000, 14),
            part(ContextPartKind::Preambles, 9_800, 14),
            part(ContextPartKind::Notices, 300, 1),
            part(ContextPartKind::Answers, 18_000, 14),
            part(ContextPartKind::ToolCalls, 4_100, 31),
            part(ContextPartKind::ToolOutputs, 48_300, 28),
            part(ContextPartKind::Pruned, 100, 3),
        ],
        largest: vec![
            item(ContextPartKind::ToolOutputs, 12_300, 12, "shell", "cargo test --workspace"),
            item(
                ContextPartKind::ToolOutputs,
                8_000,
                2,
                "read_file",
                "crates/efr-daemon/src/run.rs",
            ),
            item(ContextPartKind::Notices, 2_100, 9, "", "j3"),
            item(ContextPartKind::ToolCalls, 1_900, 7, "apply_patch", "src/a.rs, src/b.rs"),
            item(ContextPartKind::Answers, 1_200, 14, "", ""),
        ],
        compactions: CompactionSummary {
            count: 1,
            newest: Some(CompactionBrief {
                at: now() - SignedDuration::from_mins(125),
                trigger: CompactionTrigger::Auto,
                tokens_before: 231_000,
                tokens_after: 24_000,
                summarized: true,
                pruned_outputs: 12,
            }),
        },
        cache: CacheSummary {
            last_turn: Some(Usage {
                cached_input_tokens: 89_500,
                cache_write_tokens: 8_000,
                cache_write_1h_tokens: 8_000,
                ..Usage::new(98_000, 1_100)
            }),
            conversation: Usage {
                cached_input_tokens: 1_100_000,
                cache_write_tokens: 210_000,
                cache_write_1h_tokens: 200_000,
                ..Usage::new(1_310_000, 21_000)
            },
            turns: 14,
        },
    }
}

#[test]
fn context_lines_at_80_columns() {
    let options = RenderOptions::new(80);
    insta::assert_snapshot!(readable(&lines(&report(), now(), &options)));
}

#[test]
fn context_in_a_pipe() {
    let options = RenderOptions::new(80).with_terminal(false);
    insta::assert_snapshot!(lines(&report(), now(), &options));
}

#[test]
fn a_long_line_is_cut_to_the_width_on_a_terminal_only() {
    let mut report = report();
    report.largest = vec![item(ContextPartKind::ToolOutputs, 12_300, 1, "shell", &"x".repeat(200))];
    let terminal = lines(&report, now(), &RenderOptions::new(60));
    let pipe = lines(&report, now(), &RenderOptions::new(60).with_terminal(false));

    let cut = terminal.lines().find(|line| line.starts_with("largest")).unwrap();
    assert!(cut.ends_with('\u{2026}') && cut.chars().count() == 60, "{cut}");
    assert!(pipe.contains(&"x".repeat(200)), "{pipe}");
}

#[test]
fn without_a_count_the_estimate_carries_the_gauge() {
    let mut report = report();
    report.counted = None;
    report.running = true;
    report.window_known = false;
    report.window = 128_000;
    report.trigger = 97_280;
    report.cache = CacheSummary::default();
    let text = lines(&report, now(), &RenderOptions::new(200).with_terminal(false));

    let line = |key: &str| text.lines().find(|line| line.starts_with(key)).unwrap().to_owned();
    assert_eq!(
        line("counted"),
        "counted      none: no turn has ended with a count since the newest compaction"
    );
    assert_eq!(
        line("estimate"),
        "estimate     about 96.7k for the next request, without its prompt, with the running \
         turn so far, ctx 99%"
    );
    assert!(line("model").contains("efr does not know the window"), "{}", line("model"));
    assert_eq!(line("cache"), "cache        no model call has reported its use yet");
}

#[test]
fn auto_compaction_off_shows_the_cap() {
    let mut report = report();
    report.auto = false;
    let text = lines(&report, now(), &RenderOptions::new(200).with_terminal(false));
    assert!(
        text.starts_with(
            "model        claude-opus-5-5, window 1.0M, auto compaction off, cap 950k\n"
        ),
        "{text}"
    );
}
