use std::path::Path;
use std::time::Duration;

use efr_protocol::{
    AdminStatusResult, ContextUse, ConversationStatus, ConversationSummary,
    ConversationsListResult, PageCursor, ProviderStatus, Seq, Usage,
};
use efr_render::{Colour, ColourMode, Palette, RenderOptions, Role, WidthMethod};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    Block, CACHE_SHOWN_FROM, Spacing, Tone, ago, approval_heading, approval_summary, catalog,
    code_block, conversations, cut, elapsed, keys, lines, one_line, paint, run_heading, size,
    status, tokens, took, tool_call, tool_result, turn_done, turn_interrupted, until,
};
use crate::testing::{FAILED_UNITS, FROM_SRC, conversation, now};

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
    assert_eq!(paint("wait", Tone::Attention, &terminal), "\x1b[1;33mwait\x1b[0m");
    let no_colour = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(paint("wait", Tone::Attention, &no_colour), "\x1b[1mwait\x1b[0m");
    let pipe = RenderOptions::new(80).with_terminal(false);
    assert_eq!(paint("note", Tone::Dim, &pipe), "note");
    assert_eq!(paint("fact", Tone::Plain, &terminal), "fact");
    assert_eq!(paint("exit 2", Tone::Failure, &terminal), "\x1b[31mexit 2\x1b[0m");
    assert_eq!(paint("exit 2", Tone::Failure, &no_colour), "\x1b[1mexit 2\x1b[0m");
}

#[test]
fn tones_take_the_colours_of_their_roles_from_the_palette() {
    let palette = Palette::new()
        .with(Role::Warning, Colour::Rgb(0xf2, 0xc1, 0x4e))
        .with(Role::Muted, Colour::Palette(8));
    let truecolor =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette.clone());
    assert_eq!(paint("wait", Tone::Attention, &truecolor), "\x1b[1;38;2;242;193;78mwait\x1b[0m");
    // A muted role with its own colour is not dim.
    assert_eq!(paint("note", Tone::Dim, &truecolor), "\x1b[90mnote\x1b[0m");
    // In 16 colours a hex colour takes the nearest slot; without colour, plain bold.
    let sixteen = RenderOptions::new(80).with_palette(palette.clone());
    assert_eq!(paint("wait", Tone::Attention, &sixteen), "\x1b[1;33mwait\x1b[0m");
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

fn status_result() -> AdminStatusResult {
    AdminStatusResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        version: "0.1.0".to_owned(),
        protocol: 1,
        pid: 4242,
        started_at: before(2 * 3_600 + 60),
        screen_backend: "vt100".to_owned(),
        conversations: 2,
        shells: 1,
        providers: vec![
            ProviderStatus {
                provider: "openai".to_owned(),
                logged_in: true,
                expires_at: Some(after(52 * 60)),
                active: false,
                login: None,
                key_hint: None,
                key_refused_at: None,
            },
            ProviderStatus {
                provider: "anthropic".to_owned(),
                logged_in: false,
                expires_at: None,
                active: false,
                login: None,
                key_hint: None,
                key_refused_at: None,
            },
        ],
        catalog: Some(efr_protocol::CatalogStatus {
            provider: None,
            origin: efr_protocol::CatalogOrigin::Backend,
            fetched_at: Some(before(5 * 60)),
        }),
        roots: None,
        config: None,
        sandbox: None,
        sandbox_paths: None,
    }
}

#[test]
fn status_lists_one_fact_per_line() {
    let socket = Path::new("/run/user/1000/efr/daemon.sock");
    insta::assert_snapshot!(status(&status_result(), socket, now()));
}

#[test]
fn status_shows_times_to_the_second() {
    let nanos = SignedDuration::from_nanos(143_411_554);
    let mut result = status_result();
    result.started_at = before(2 * 3_600 + 60) + nanos;
    result.providers[0].expires_at = Some(after(52 * 60) + nanos);
    let text = status(&result, Path::new("/s"), now());
    assert!(text.contains("started        2026-10-04T09:59:00Z (2h 0m ago)\n"), "{text}");
    assert!(text.contains("token expires 2026-10-04T12:52:00Z (in 52m 0s)\n"), "{text}");
}

#[test]
fn the_catalog_says_where_it_came_from_and_when() {
    let status =
        |origin, fetched_at| efr_protocol::CatalogStatus { provider: None, origin, fetched_at };
    let backend = status(efr_protocol::CatalogOrigin::Backend, Some(before(5 * 60)));
    assert_eq!(catalog(&backend, now()), "from the backend, fetched 5m 0s ago");
    let cache = status(efr_protocol::CatalogOrigin::Cache, Some(before(26 * 3_600)));
    assert_eq!(catalog(&cache, now()), "from the cache, fetched 1d 2h ago");
    let builtin = status(efr_protocol::CatalogOrigin::Builtin, None);
    assert_eq!(catalog(&builtin, now()), "built into efr");
    let missing = status(efr_protocol::CatalogOrigin::Missing, None);
    assert_eq!(catalog(&missing, now()), "no list yet; efrd fetches it before the next prompt");
}

#[test]
fn the_catalog_names_its_provider() {
    let status = efr_protocol::CatalogStatus {
        provider: Some("anthropic-api".to_owned()),
        origin: efr_protocol::CatalogOrigin::Backend,
        fetched_at: Some(before(5 * 60)),
    };
    assert_eq!(catalog(&status, now()), "anthropic-api, from the backend, fetched 5m 0s ago");
    let missing = efr_protocol::CatalogStatus {
        origin: efr_protocol::CatalogOrigin::Missing,
        fetched_at: None,
        ..status
    };
    assert_eq!(
        catalog(&missing, now()),
        "anthropic-api, no list yet; efrd fetches it before the next prompt"
    );
}

#[test]
fn status_without_providers_says_so() {
    let result = AdminStatusResult { providers: Vec::new(), ..status_result() };
    let text = status(&result, Path::new("/s"), now());
    assert!(text.contains("providers      none configured\n"), "{text}");
}

#[test]
fn status_text_from_the_daemon_cannot_drive_the_terminal() {
    let result = AdminStatusResult { screen_backend: "vt\u{1b}[2J".to_owned(), ..status_result() };
    let text = status(&result, Path::new("/s"), now());
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn conversations_list_newest_first_with_status_and_age() {
    let summary = |n: u64, status, title: Option<&str>, age| ConversationSummary {
        id: format!("019a9b1c-3d00-7a10-8b20-00000000000{n}").parse().unwrap(),
        title: title.map(str::to_owned),
        status,
        created_at: before(age + 600),
        updated_at: before(age),
        last_seq: Seq::new(n * 10),
        cwd: None,
        scope: None,
        tty: None,
    };
    let list = ConversationsListResult {
        conversations: vec![
            summary(1, ConversationStatus::Running, Some("fix the nginx config"), 30),
            summary(2, ConversationStatus::AwaitingApproval, Some("tidy\n~/.zshrc"), 3_700),
            summary(3, ConversationStatus::Idle, Some("rotate the logs"), 758),
            summary(4, ConversationStatus::Idle, None, 200_000),
        ],
        next_cursor: Some(PageCursor::new("c2")),
    };
    insta::assert_snapshot!(conversations(&list, now()));
    assert_eq!(list.conversations[0].id, conversation());
}

#[test]
fn an_empty_list_says_so() {
    assert_eq!(conversations(&ConversationsListResult::default(), now()), "no conversations yet\n");
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
}

#[test]
fn the_end_of_turn_line_leaves_out_what_is_not_known() {
    let usage = Usage::new(18_250, 1_100);
    let done = |took, usage, context| turn_done(took, usage, context).plain();
    assert_eq!(
        done(Some(Duration::from_secs(42)), Some(&usage), None),
        "done in 42s, 18.2k tokens in, 1.1k out, cache 0%"
    );
    assert_eq!(done(None, Some(&usage), None), "done, 18.2k tokens in, 1.1k out, cache 0%");
    assert_eq!(done(Some(Duration::from_millis(1_500)), None, None), "done in 1.5s");
    assert_eq!(done(None, None, None), "done");
}

#[test]
fn the_end_of_turn_line_shows_the_cache_share_from_a_large_enough_input() {
    let usage = |input, cached| Usage { cached_input_tokens: cached, ..Usage::new(input, 1_100) };
    let done = |usage: &Usage| turn_done(None, Some(usage), None).plain();
    assert_eq!(done(&usage(18_250, 16_700)), "done, 18.2k tokens in, 1.1k out, cache 91%");
    // Rounded down, never up to a full cache.
    assert_eq!(done(&usage(10_000, 9_999)), "done, 10.0k tokens in, 1.1k out, cache 99%");
    assert_eq!(done(&usage(10_000, 10_000)), "done, 10.0k tokens in, 1.1k out, cache 100%");
    // A turn whose input went to the cache only as a write read nothing from it.
    let written = Usage { cache_write_tokens: 18_000, ..usage(18_250, 0) };
    assert_eq!(done(&written), "done, 18.2k tokens in, 1.1k out, cache 0%");
    // From 2048 tokens on; below, no cache can form.
    assert_eq!(done(&usage(CACHE_SHOWN_FROM, 1_024)), "done, 2.0k tokens in, 1.1k out, cache 50%");
    assert_eq!(done(&usage(CACHE_SHOWN_FROM - 1, 1_024)), "done, 2.0k tokens in, 1.1k out");
    // A count that says more was cached than sent stays at 100%.
    assert_eq!(done(&usage(4_000, 9_000)), "done, 4.0k tokens in, 1.1k out, cache 100%");
    // With the gauge it follows the output, in the muted rest of the line.
    let context = ContextUse { tokens: 89_400, limit: 206_720, window: 272_000 };
    let line = turn_done(None, Some(&usage(18_250, 16_700)), Some(&context));
    assert_eq!(line.plain(), "done, ctx 43% (89k/206k), 1.1k out, cache 91%");
    assert_eq!(
        line.render(&RenderOptions::new(80)),
        "\x1b[2mdone, \x1b[0m\x1b[32mctx 43%\x1b[0m\x1b[2m (89k/206k), 1.1k out, cache 91%\x1b[0m\n"
    );
    assert_eq!(
        line.render(&RenderOptions::new(80).with_colour(ColourMode::None)),
        "\x1b[2mdone, \x1b[0mctx 43%\x1b[2m (89k/206k), 1.1k out, cache 91%\x1b[0m\n"
    );
    assert_eq!(
        line.render(&RenderOptions::new(80).with_terminal(false)),
        "done, ctx 43% (89k/206k), 1.1k out, cache 91%\n"
    );
}

#[test]
fn with_the_context_the_end_of_turn_line_shows_it_in_place_of_the_input() {
    let usage = Usage { cached_input_tokens: 900_000, ..Usage::new(918_250, 1_100) };
    let context = ContextUse { tokens: 89_400, limit: 206_720, window: 272_000 };
    let took = Some(Duration::from_secs(42));
    assert_eq!(
        turn_done(took, Some(&usage), Some(&context)).plain(),
        "done in 42s, ctx 43% (89k/206k), 1.1k out, cache 98%"
    );
    assert_eq!(turn_done(None, None, Some(&context)).plain(), "done, ctx 43% (89k/206k)");
    assert_eq!(
        turn_interrupted(Some(Duration::from_millis(12_400)), Some(&context)).plain(),
        "interrupted after 12s, ctx 43% (89k/206k)"
    );
    assert_eq!(turn_interrupted(None, None).plain(), "interrupted");
    // The gauge is in the colour of its level, the rest is muted.
    let options = RenderOptions::new(80);
    assert_eq!(
        turn_done(took, Some(&usage), Some(&context)).render(&options),
        "\x1b[2mdone in 42s, \x1b[0m\x1b[32mctx 43%\x1b[0m\x1b[2m (89k/206k), 1.1k out, cache 98%\x1b[0m\n"
    );
    // Too wide for the screen: one muted trace line, cut, as before.
    let narrow = turn_done(took, Some(&usage), Some(&context)).render(&RenderOptions::new(30));
    assert!(!narrow.contains("\x1b[32m") && narrow.contains('\u{2026}'), "{narrow:?}");
    let piped = RenderOptions::new(80).with_terminal(false);
    assert_eq!(
        turn_done(took, Some(&usage), Some(&context)).render(&piped),
        "done in 42s, ctx 43% (89k/206k), 1.1k out, cache 98%\n"
    );
}
