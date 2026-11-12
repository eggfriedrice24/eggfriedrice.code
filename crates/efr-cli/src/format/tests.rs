use std::time::{Duration, Instant};

use efr_protocol::Usage;
use efr_render::{Colour, ColourMode, Palette, RenderOptions, Role, WidthMethod};
use efr_test_support::bench::{self, Report};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, proptest};
use serde_json::json;

use super::{
    Block, Spacing, Tone, ago, approval_heading, approval_summary, code_block, cut, elapsed, keys,
    lines, local_time, one_line, paint, run_heading, size, tokens, took, tool_call, tool_result,
    until, usage_line, wrap_command, wrap_spans,
};
use crate::testing::{FAILED_UNITS, FROM_SRC, now};

fn before(seconds: i64) -> Timestamp {
    now() - SignedDuration::from_secs(seconds)
}

fn after(seconds: i64) -> Timestamp {
    now() + SignedDuration::from_secs(seconds)
}

#[test]
fn one_line_flattens_and_shows_control_characters() {
    assert_eq!(one_line("a\nb\tc\rd"), "a b c d");
    assert_eq!(one_line("x\u{1b}[2Jy"), "x\u{241b}[2Jy");
    assert_eq!(one_line("bell\u{7}del\u{7f}c1\u{9b}"), "bell\u{2407}del\u{2421}c1\u{fffd}");
}

#[test]
fn lines_keep_newlines_but_not_escapes() {
    assert_eq!(lines("plain\ntext"), "plain\ntext");
    assert_eq!(lines("a\u{1b}]0;title\u{7}\nb\tc"), "a\u{241b}]0;title\u{2407}\nb c");
}

#[test]
fn paint_styles_only_on_a_terminal() {
    let terminal = RenderOptions::new(80);
    assert_eq!(paint("note", Tone::Dim, &terminal), "\x1b[2mnote\x1b[0m");
    assert_eq!(paint("you", Tone::Bold, &terminal), "\x1b[1myou\x1b[0m");
    assert_eq!(paint("wait", Tone::Attention, &terminal), "\x1b[33mwait\x1b[0m");
    let no_colour = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(paint("wait", Tone::Attention, &no_colour), "\x1b[1mwait\x1b[0m");
    let pipe = RenderOptions::new(80).with_terminal(false);
    assert_eq!(paint("note", Tone::Dim, &pipe), "note");
    assert_eq!(paint("fact", Tone::Plain, &terminal), "fact");
    assert_eq!(paint("exit 2", Tone::Failure, &terminal), "\x1b[31mexit 2\x1b[0m");
    assert_eq!(paint("exit 2", Tone::Failure, &no_colour), "\x1b[1mexit 2\x1b[0m");
}

#[test]
fn the_tones_of_the_legend_add_their_attributes_to_the_role() {
    let terminal = RenderOptions::new(80);
    assert_eq!(paint("app", Tone::Text, &terminal), "app");
    assert_eq!(paint("title", Tone::TextBold, &terminal), "\x1b[1mtitle\x1b[0m");
    assert_eq!(paint("words", Tone::TextItalic, &terminal), "\x1b[3mwords\x1b[0m");
    assert_eq!(paint("\u{2717}", Tone::FailureBold, &terminal), "\x1b[1;31m\u{2717}\x1b[0m");
    assert_eq!(paint("allow", Tone::AttentionBold, &terminal), "\x1b[1;33mallow\x1b[0m");
    assert_eq!(paint(" ? ", Tone::Badge, &terminal), "\x1b[1;7;33m ? \x1b[0m");
    // Without colour the attributes stay, and the badge is a bold `?`.
    let none = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(paint(" ? ", Tone::Badge, &none), "\x1b[1m ? \x1b[0m");
    assert_eq!(paint("words", Tone::TextItalic, &none), "\x1b[3mwords\x1b[0m");
    assert_eq!(paint("app", Tone::Text, &none), "app");
    // The text role takes the palette's colour; plain and bold do not.
    let text = Palette::new().with(Role::Text, Colour::Rgb(0xd8, 0xd3, 0xc3));
    let truecolor = RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(text);
    assert_eq!(paint("note", Tone::Text, &truecolor), "\x1b[38;2;216;211;195mnote\x1b[0m");
    assert_eq!(paint("you", Tone::Bold, &truecolor), "\x1b[1myou\x1b[0m");
    assert_eq!(paint("fact", Tone::Plain, &truecolor), "fact");
}

#[test]
fn tones_take_the_colours_of_their_roles_from_the_palette() {
    let palette = Palette::new()
        .with(Role::Warning, Colour::Rgb(0xf2, 0xc1, 0x4e))
        .with(Role::Muted, Colour::Palette(8));
    let truecolor =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette.clone());
    assert_eq!(paint("wait", Tone::Attention, &truecolor), "\x1b[38;2;242;193;78mwait\x1b[0m");
    // A muted role with its own colour is not dim.
    assert_eq!(paint("note", Tone::Dim, &truecolor), "\x1b[90mnote\x1b[0m");
    // In 16 colours a hex colour takes the nearest slot; without colour, plain bold.
    let sixteen = RenderOptions::new(80).with_palette(palette.clone());
    assert_eq!(paint("wait", Tone::Attention, &sixteen), "\x1b[33mwait\x1b[0m");
    let none = RenderOptions::new(80).with_colour(ColourMode::None).with_palette(palette);
    assert_eq!(paint("wait", Tone::Attention, &none), "\x1b[1mwait\x1b[0m");
    assert_eq!(paint("note", Tone::Dim, &none), "\x1b[2mnote\x1b[0m");
}

#[test]
fn the_keys_of_a_question_are_bold_in_a_muted_line() {
    let terminal = RenderOptions::new(80);
    assert_eq!(
        keys(&[("y", "allow"), ("n", "deny")], &terminal),
        "\x1b[1my\x1b[0m\x1b[2m allow\x1b[0m\x1b[2m \u{b7} \x1b[0m\x1b[1mn\x1b[0m\x1b[2m deny\x1b[0m"
    );
    let pipe = RenderOptions::new(80).with_terminal(false);
    assert_eq!(keys(&[("y", "keep"), ("n", "leave")], &pipe), "y keep \u{b7} n leave");
}

#[test]
fn a_cut_counts_widths_as_the_terminal_does() {
    // A family emoji: three wide code points joined, one wide cluster in Ghostty.
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let text = format!("{family} ok");
    assert_eq!(cut(&text, 5, WidthMethod::Grapheme), text);
    assert_eq!(cut(&text, 5, WidthMethod::CodePoint), "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{2026}");
    // A cluster is never cut in two.
    assert_eq!(
        cut(&format!("{family}{family}"), 3, WidthMethod::Grapheme),
        format!("{family}\u{2026}")
    );
}

#[test]
fn blocks_are_separated_by_blank_lines_and_notes_stay_together() {
    let mut spacing = Spacing::default();
    let separators = [
        spacing.before(Block::Prompt),
        spacing.before(Block::Message),
        spacing.before(Block::Note),
        spacing.before(Block::Note),
        spacing.before(Block::Question),
        spacing.before(Block::Settled),
        spacing.before(Block::Call),
        spacing.before(Block::Call),
        spacing.before(Block::Question),
        spacing.before(Block::Answer),
        spacing.before(Block::Call),
        spacing.before(Block::Message),
    ];
    // An answer follows its question, and the call it settled follows the answer.
    assert_eq!(separators, ["", "\n", "\n", "", "\n", "", "", "\n", "\n", "", "\n", "\n"]);
}

/// The spacing of the design, as BeforeAfter 4 and the notes of the Calls and Endings
/// boards give it: rows stick together, prose stands apart, a steer has a blank line
/// above and below, the end lines stick together after one blank line, and nothing
/// stands between the prompt line and what follows it.
#[test]
fn the_rows_of_the_design_stick_together_and_prose_stands_apart() {
    let mut spacing = Spacing::default();
    let kinds = [
        Block::Prompt,
        Block::Row,
        Block::Row,
        Block::Message,
        Block::Row,
        Block::Row,
        Block::Steer,
        Block::Row,
        Block::Message,
        Block::End,
        Block::End,
    ];
    let separators: Vec<&str> = kinds.into_iter().map(|kind| spacing.before(kind)).collect();
    assert_eq!(separators, ["", "", "", "\n", "\n", "", "\n", "\n", "\n", "\n", ""]);
    // The end lines stand one blank line after a row too, and nothing after a prompt.
    let mut spacing = Spacing::default();
    assert_eq!(
        [Block::Row, Block::End, Block::End].map(|kind| spacing.before(kind)),
        ["", "\n", ""]
    );
    let mut spacing = Spacing::default();
    assert_eq!([Block::Prompt, Block::Row].map(|kind| spacing.before(kind)), ["", ""]);
    // Today's notes and calls are rows next to a row of the design.
    let mut spacing = Spacing::default();
    let mixed = [Block::Row, Block::Call, Block::Row, Block::Note, Block::Question, Block::Row];
    assert_eq!(mixed.map(|kind| spacing.before(kind)), ["", "", "", "", "\n", ""]);
    // A thought row sticks to the row after it.
    let mut stuck = Spacing::stuck();
    assert_eq!(stuck.before(Block::Row), "");
}

#[test]
fn a_code_block_fence_outgrows_the_backticks_inside() {
    assert_eq!(code_block("diff", "-a\n+b\n"), "```diff\n-a\n+b\n```\n");
    assert_eq!(code_block("", "no newline"), "```\nno newline\n```\n");
    assert_eq!(code_block("md", "```rust\nx\n```"), "````md\n```rust\nx\n```\n````\n");
}

#[test]
fn a_tool_call_shows_its_most_telling_input() {
    let call = |tool, input| tool_call(tool, &input, None);
    assert_eq!(call("shell", json!({"command": "ls -la", "timeout": 30})), "$ ls -la");
    assert_eq!(call("read_file", json!({"path": "/etc/hosts"})), "read /etc/hosts");
    assert_eq!(call("write_file", json!({"path": "src/main.rs"})), "write src/main.rs");
    let edit = json!({"path": "src/a.rs", "old_string": "a", "new_string": "b"});
    assert_eq!(call("edit", edit), "edit src/a.rs");
    assert_eq!(call("settings", json!({"key": "model.name"})), r#"settings {"key":"model.name"}"#);
    assert_eq!(call("shell", json!({"command": ""})), "$");
    assert_eq!(call("fetch", json!({"url": "https://x"})), "fetch: https://x");
    assert_eq!(call("odd", json!({"n": 1})), r#"odd: {"n":1}"#);
    assert_eq!(call("echo", json!("hi")), "echo: hi");
    assert_eq!(call("noop", json!(null)), "noop");
}

#[test]
fn a_tool_call_of_several_lines_shows_the_first_and_how_many_follow() {
    let call = |command: &str, columns| tool_call("shell", &json!({ "command": command }), columns);
    assert_eq!(call(FROM_SRC, None), "$ cd src (and 3 more lines)");
    assert_eq!(call(FAILED_UNITS, None), "$ systemctl --failed --no-pager (and 1 more line)");
    // A cut keeps the count: the reader must see that more lines follow.
    let narrow = Some((32, WidthMethod::CodePoint));
    assert_eq!(call(FAILED_UNITS, narrow), "$ systemctl -\u{2026} (and 1 more line)");
    // Blank lines at the start and the end run nothing.
    assert_eq!(call("\n\nls -la\n\n", None), "$ ls -la");
    assert_eq!(call("ls\n\n  \nuptime\n", None), "$ ls (and 3 more lines)");
    // Control characters stay visible, a carriage return too.
    assert_eq!(call("a\rb\u{1b}[2J\nc", None), "$ a\u{240d}b\u{241b}[2J (and 1 more line)");
}

#[test]
fn a_question_shows_each_line_of_a_command_on_its_own() {
    assert_eq!(run_heading("shell", "uptime\n"), ["shell: run \"uptime\""]);
    insta::assert_snapshot!(format!(
        "{}\n---\n{}",
        run_heading("shell", FROM_SRC).join("\n"),
        run_heading("shell", FAILED_UNITS).join("\n")
    ));
    let many: Vec<String> = (1..=10).map(|n| format!("echo {n}")).collect();
    let heading = run_heading("shell", &many.join("\n"));
    assert_eq!(heading[0], "shell: run 10 lines:");
    assert_eq!(heading[1], "   1  echo 1");
    assert_eq!(heading[10], "  10  echo 10");
    // A line cannot pass for a line of the question.
    let heading = run_heading("shell", "ls\nallow? y = yes\x1b[2K");
    assert_eq!(heading, ["shell: run 2 lines:", "  1  ls", "  2  allow? y = yes\u{241b}[2K"]);
}

#[test]
fn an_approval_of_a_command_of_several_lines_shows_each_line() {
    let summary = format!("shell: run {FAILED_UNITS:?}");
    let (heading, asking) = approval_heading(&summary, Some(("shell", FAILED_UNITS)));
    assert_eq!(
        heading,
        [
            "shell: run 2 lines:",
            "  1  systemctl --failed --no-pager",
            "  2  journalctl -b -n 20 --no-pager",
        ]
    );
    assert_eq!(asking, None);

    let summary = format!(
        "shell: run {FAILED_UNITS:?}; read /home/u/.ssh/id (secrets)\nasks for: systemctl --failed"
    );
    let (heading, asking) = approval_heading(&summary, Some(("shell", FAILED_UNITS)));
    assert_eq!(heading.last().unwrap(), "also: read /home/u/.ssh/id (secrets)");
    assert_eq!(asking.as_deref(), Some("asks for: systemctl --failed"));

    // An older daemon put a path before the command line.
    let summary = format!(
        "shell: read all under /home/u (user data); run {FAILED_UNITS:?}; input at the terminal"
    );
    let (heading, _) = approval_heading(&summary, Some(("shell", FAILED_UNITS)));
    assert_eq!(heading[0], "shell: run 2 lines:");
    assert_eq!(
        heading.last().unwrap(),
        "also: read all under /home/u (user data); input at the terminal"
    );
    // The quote must be a whole part, not the end of a path's name.
    let summary = format!("shell: write /x run {FAILED_UNITS:?}");
    assert_eq!(approval_heading(&summary, Some(("shell", FAILED_UNITS))).0.len(), 1);

    // A summary that does not quote the call's command shows as before.
    let (heading, _) = approval_heading("write_file: write /x", Some(("shell", FAILED_UNITS)));
    assert_eq!(heading, ["write_file: write /x"]);
    let summary = format!("shell: run {:?}", "ls");
    assert_eq!(approval_heading(&summary, Some(("shell", "ls"))).0, ["shell: run \"ls\""]);
    assert_eq!(approval_heading(&summary, None).0, ["shell: run \"ls\""]);
}

#[test]
fn only_failed_tool_calls_get_a_result_line() {
    assert_eq!(tool_result("shell", false, Some(0)), None);
    assert_eq!(tool_result("shell", false, None), None);
    assert_eq!(tool_result("shell", true, Some(2)).as_deref(), Some("shell exited with 2"));
    assert_eq!(tool_result("shell", false, Some(1)).as_deref(), Some("shell exited with 1"));
    assert_eq!(tool_result("write_file", true, None).as_deref(), Some("write_file failed"));
}

#[test]
fn ages_use_their_two_largest_units() {
    assert_eq!(ago(now(), now()), "just now");
    assert_eq!(ago(before(45), now()), "45s ago");
    assert_eq!(ago(before(5 * 60 + 3), now()), "5m 3s ago");
    assert_eq!(ago(before(2 * 3_600 + 5 * 60), now()), "2h 5m ago");
    assert_eq!(ago(before(3 * 86_400 + 4 * 3_600), now()), "3d 4h ago");
    // A clock that runs behind the daemon's must not print a negative age.
    assert_eq!(ago(now() + SignedDuration::from_secs(10), now()), "just now");
}

#[test]
fn a_future_time_counts_down_and_a_past_one_up() {
    assert_eq!(until(after(52 * 60), now()), "in 52m 0s");
    assert_eq!(until(before(90), now()), "1m 30s ago");
}

#[test]
fn a_local_time_reads_to_the_second() {
    assert_eq!(local_time(now(), &TimeZone::UTC), "2026-10-04 12:00:00 UTC");
    let berlin = TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
    assert_eq!(local_time(now(), &berlin), "2026-10-04 14:00:00 CEST");
}

#[test]
fn an_approval_summary_splits_off_the_parts_that_ask() {
    assert_eq!(
        approval_summary("shell: run \"uptime; hostnamectl\"\nasks for: hostnamectl"),
        ("shell: run \"uptime; hostnamectl\"".to_owned(), Some("asks for: hostnamectl".to_owned()))
    );
    assert_eq!(approval_summary("write /etc/hosts"), ("write /etc/hosts".to_owned(), None));
}

#[test]
fn an_approval_summary_keeps_a_line_that_is_not_a_list_of_names() {
    for summary in
        ["write /a\nasks for: x (user data)", "run x\nasks for: \u{1b}[31mx", "x\nasks for: "]
    {
        let (first, asking) = approval_summary(summary);
        assert_eq!(asking, None, "{summary:?}");
        assert_eq!(first, one_line(summary));
    }
}

#[test]
fn format_characters_show_as_a_stand_in_in_every_line() {
    let line = "ls \u{202e}gpj.exe\u{202c} \u{2067}x\u{2069}\u{200d}\u{feff}";
    for shown in [
        one_line(line),
        lines(&format!("{line}\nnext")).into_owned(),
        tool_call("shell", &json!({ "command": line }), None),
        run_heading("shell", &format!("{line}\nls")).join("\n"),
        approval_summary(&format!("shell: write /home/u/{line}")).0,
    ] {
        assert!(
            !shown.chars().any(|c| matches!(
                c,
                '\u{202c}' | '\u{202e}' | '\u{2067}' | '\u{2069}' | '\u{200d}' | '\u{feff}'
            )),
            "{shown:?}"
        );
        assert!(shown.contains("\u{fffd}gpj.exe\u{fffd}"), "{shown:?}");
    }
    // Text without one stays as it is.
    assert!(matches!(lines("plain\ntext"), std::borrow::Cow::Borrowed(_)));
    assert_eq!(one_line("naïve 日本 \u{1f600}"), "naïve 日本 \u{1f600}");
}

#[test]
fn elapsed_time_counts_seconds_then_minutes_then_hours() {
    let cases = [
        (0, "0s"),
        (1, "1s"),
        (59, "59s"),
        (60, "1m 00s"),
        (61, "1m 01s"),
        (3_599, "59m 59s"),
        (3_600, "1h 00m 00s"),
        (3_725, "1h 02m 05s"),
    ];
    for (seconds, shown) in cases {
        assert_eq!(elapsed(Duration::from_secs(seconds)), shown, "{seconds}");
    }
    assert_eq!(elapsed(Duration::from_millis(1_999)), "1s");
}

#[test]
fn a_turn_under_ten_seconds_shows_tenths() {
    assert_eq!(took(Duration::from_millis(420)), "0.4s");
    assert_eq!(took(Duration::from_millis(9_990)), "9.9s");
    assert_eq!(took(Duration::from_secs(42)), "42s");
    assert_eq!(took(Duration::from_secs(66)), "1m 06s");
}

#[test]
fn token_counts_are_short_and_never_round_up() {
    let cases = [
        (0, "0"),
        (999, "999"),
        (1_000, "1.0k"),
        (1_234, "1.2k"),
        (18_250, "18.2k"),
        (99_999, "99.9k"),
        (120_000, "120k"),
        (999_999, "999k"),
        (1_200_000, "1.2M"),
    ];
    for (count, shown) in cases {
        assert_eq!(tokens(count), shown, "{count}");
    }
}

#[test]
fn sizes_count_in_thousands() {
    assert_eq!(size(0), "0 B");
    assert_eq!(size(999), "999 B");
    assert_eq!(size(3_250), "3.2 KB");
    assert_eq!(size(1_450_000), "1.4 MB");
    assert_eq!(size(2_180_000_000), "2.1 GB");
}

#[test]
fn usage_line_holds_numbers_only() {
    let usage = Usage {
        cached_input_tokens: 16_600,
        cache_write_tokens: 1_200,
        cache_write_1h_tokens: 1_200,
        reasoning_tokens: 400,
        context_tokens: 19_300,
        ..Usage::new(18_200, 1_100)
    };
    assert_eq!(usage_line(&usage), "18.2k in, 1.1k out, cache 91%, wrote 1.2k (1h)");

    let five_minutes = Usage { cache_write_1h_tokens: 0, ..usage };
    assert_eq!(usage_line(&five_minutes), "18.2k in, 1.1k out, cache 91%, wrote 1.2k");
    let both = Usage { cache_write_1h_tokens: 800, ..usage };
    assert_eq!(usage_line(&both), "18.2k in, 1.1k out, cache 91%, wrote 1.2k (800 1h)");

    // Below the cache's minimum the share says nothing; an answer with no cache use
    // has only its two counts.
    assert_eq!(usage_line(&Usage::new(1_500, 20)), "1.5k in, 20 out");
    let line = usage_line(&usage);
    assert!(
        line.chars().all(|c| c.is_ascii_digit() || " .,%()kMinoutcachewr1h".contains(c)),
        "{line}"
    );
}

/// A command line of `bytes` bytes: words of options and paths between spaces, or one
/// word without a space, as a long base64 argument is.
fn long_command(bytes: usize, spaces: bool) -> String {
    let unit = if spaces { "--flag=src/x.rs " } else { "QWxhZGRpbjpvcGVu" };
    unit.repeat(bytes / unit.len() + 1)[..bytes].to_owned()
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_wrap_command() {
    let mut report = Report::new("bench_perf_wrap_command");
    let mut slopes = Vec::new();
    for method in [WidthMethod::CodePoint, WidthMethod::Grapheme] {
        for spaces in [true, false] {
            let kind = format!(
                "{} {}",
                if method == WidthMethod::Grapheme { "grapheme" } else { "code point" },
                if spaces { "spaces" } else { "no spaces" }
            );
            let mut points = Vec::new();
            for kb in [1_u64, 10, 100] {
                let line = long_command(kb as usize * 1024, spaces);
                let start = Instant::now();
                let rows = wrap_command(&line, 80, 4, method);
                let first = start.elapsed();
                let joined: String = rows.iter().map(|row| row.text.as_str()).collect();
                assert_eq!(joined, line);
                let runs = if first > Duration::from_millis(500) { 0 } else { 4 };
                let mut samples = vec![first];
                samples.extend(bench::run(0, runs, || {
                    std::hint::black_box(wrap_command(&line, 80, 4, method));
                }));
                let stats = report.time(&format!("{kind} {kb} KB"), &samples);
                report.count(&format!("{kind} {kb} KB: rows"), rows.len() as u64);
                points.push((kb, stats.p50));
            }
            slopes.push((kind.clone(), report.slope(&format!("{kind} slope over bytes"), &points)));
        }
    }
    report.finish();
    // Budget: the cost grows with the line, not with its square (it was 1.9).
    for (kind, slope) in slopes {
        assert!(slope < 1.3, "{kind}: slope {slope:.2} over bytes");
    }
}

#[test]
fn a_piece_wider_than_the_row_shows_once() {
    let spans = [("\u{65e5}".to_owned(), Tone::Plain), (" ok".to_owned(), Tone::Plain)];
    assert_eq!(
        wrap_spans(&spans, 1, 1, WidthMethod::CodePoint),
        [
            vec![("\u{65e5}".to_owned(), Tone::Plain)],
            vec![("o".to_owned(), Tone::Plain)],
            vec![("k".to_owned(), Tone::Plain)]
        ]
    );
}

/// The code of `wrap_command`, `wrap_spans` and `cut` before `pieces` became an
/// iterator and the width checks stopped at a full row: the reference of the property
/// tests below. `wrap_spans` has the fix of `a_piece_wider_than_the_row_shows_once`.
mod before {
    use efr_render::{WidthMethod, text_width};
    use unicode_segmentation::UnicodeSegmentation as _;

    use super::super::{CommandRow, RowEnd, Tone, WORD_MARK, WRAP_MARK, push_span, runs};

    fn pieces(text: &str, method: WidthMethod) -> Vec<&str> {
        match method {
            WidthMethod::Grapheme => text.graphemes(true).collect(),
            _ => text.split_inclusive(|_| true).collect(),
        }
    }

    pub(super) fn wrap_command(
        line: &str,
        room: usize,
        indent: usize,
        method: WidthMethod,
    ) -> Vec<CommandRow> {
        let mark = text_width(WRAP_MARK, method).max(text_width(WORD_MARK, method));
        let mut rows = Vec::new();
        let mut left = line;
        let mut indented = false;
        loop {
            let here = if indented { room.saturating_sub(indent) } else { room }.max(1);
            if text_width(left, method) <= here {
                rows.push(CommandRow { text: left.to_owned(), indented, end: RowEnd::Last });
                return rows;
            }
            let budget = here.saturating_sub(mark);
            let (mut end, mut used, mut after_space, mut word) = (0, 0, None, false);
            for piece in pieces(left, method) {
                let width = text_width(piece, method);
                if end > 0 && used + width > budget {
                    break;
                }
                end += piece.len();
                used += width;
                if piece == " " {
                    if word {
                        after_space = Some(end);
                    }
                } else {
                    word = true;
                }
            }
            let (cut, kind) = match after_space {
                Some(at) if at == end || text_width(&left[..at], method) * 2 >= here => {
                    (at, RowEnd::Space)
                }
                _ => (end, RowEnd::Word),
            };
            if cut >= left.len() {
                rows.push(CommandRow { text: left.to_owned(), indented, end: RowEnd::Last });
                return rows;
            }
            let (row, after) = left.split_at(cut);
            rows.push(CommandRow { text: row.to_owned(), indented, end: kind });
            left = after;
            indented |= kind == RowEnd::Space;
        }
    }

    pub(super) fn wrap_spans(
        spans: &[(String, Tone)],
        first: usize,
        rest: usize,
        method: WidthMethod,
    ) -> Vec<Vec<(String, Tone)>> {
        let mut rows: Vec<Vec<(String, Tone)>> = vec![Vec::new()];
        let mut used = 0;
        let mut room = first;
        for (text, tone) in spans {
            for token in runs(text) {
                let width = text_width(token, method);
                let space = token.starts_with(' ');
                if space && used == 0 && rows.len() > 1 {
                    continue;
                }
                if used + width <= room {
                    push_span(&mut rows, token, *tone);
                    used += width;
                    continue;
                }
                if used > 0 {
                    rows.push(Vec::new());
                    used = 0;
                    room = rest;
                }
                if space {
                    continue;
                }
                let mut left = token;
                while text_width(left, method) > room {
                    let mut end = 0;
                    let mut taken = 0;
                    for piece in pieces(left, method) {
                        let width = text_width(piece, method);
                        if end > 0 && taken + width > room {
                            break;
                        }
                        end += piece.len();
                        taken += width;
                    }
                    // The one change: a piece wider than the row showed twice.
                    if end == left.len() {
                        break;
                    }
                    let (row, after) = left.split_at(end);
                    push_span(&mut rows, row, *tone);
                    rows.push(Vec::new());
                    room = rest;
                    left = after;
                }
                push_span(&mut rows, left, *tone);
                used = text_width(left, method);
            }
        }
        for row in &mut rows {
            if let Some((text, _)) = row.last_mut() {
                let kept = text.trim_end_matches(' ').len();
                text.truncate(kept);
            }
            row.retain(|(text, _)| !text.is_empty());
        }
        while rows.len() > 1 && rows.last().is_some_and(Vec::is_empty) {
            rows.pop();
        }
        rows
    }

    pub(super) fn cut(text: &str, columns: usize, method: WidthMethod) -> String {
        if text_width(text, method) <= columns {
            return text.to_owned();
        }
        let mut out = String::new();
        let mut used = 0;
        for piece in pieces(text, method) {
            let w = text_width(piece, method);
            if used + w + 1 > columns {
                break;
            }
            out.push_str(piece);
            used += w;
        }
        out.push('\u{2026}');
        out
    }
}

/// Text of the kinds that make a cut hard: words and runs of spaces, wide characters,
/// combining marks, a zero width joiner in an emoji, flags, a variation selector and a
/// zero width space.
const HARD_TEXT: &str = "(a|bc|--flag=x|  | |\u{65e5}\u{672c}|e\u{301}|\u{1f468}\u{200d}\u{1f469}|\u{1f1e9}\u{1f1ea}|\u{2764}\u{fe0f}|\u{200b}){0,40}";

proptest! {
    /// The rows of a command are the ones that the code before the fix gave.
    #[test]
    fn wrap_command_keeps_the_rows_it_had(
        line in HARD_TEXT,
        room in 1_usize..24,
        indent in 0_usize..8,
        grapheme in proptest::bool::ANY,
    ) {
        let method = if grapheme { WidthMethod::Grapheme } else { WidthMethod::CodePoint };
        assert_eq!(
            wrap_command(&line, room, indent, method),
            before::wrap_command(&line, room, indent, method)
        );
    }

    /// The rows of spans and a cut text are the ones that the code before the fix gave.
    #[test]
    fn wrap_spans_and_cut_keep_what_they_gave(
        texts in prop::collection::vec(HARD_TEXT, 1..4),
        first in 0_usize..24,
        rest in 1_usize..24,
        grapheme in proptest::bool::ANY,
    ) {
        let method = if grapheme { WidthMethod::Grapheme } else { WidthMethod::CodePoint };
        let spans: Vec<(String, Tone)> = texts
            .iter()
            .enumerate()
            .map(|(at, text)| (text.clone(), if at % 2 == 0 { Tone::Plain } else { Tone::Dim }))
            .collect();
        assert_eq!(
            wrap_spans(&spans, first, rest, method),
            before::wrap_spans(&spans, first, rest, method)
        );
        for text in &texts {
            assert_eq!(cut(text, rest, method), before::cut(text, rest, method));
        }
    }
}
