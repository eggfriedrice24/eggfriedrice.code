use std::path::Path;

use efr_protocol::{
    AdminStatusResult, ConversationStatus, ConversationSummary, ConversationsListResult,
    PageCursor, ProviderStatus, Seq,
};
use efr_render::{ColourMode, RenderOptions};
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{
    Block, Spacing, Tone, ago, approval_heading, approval_summary, code_block, conversations,
    lines, one_line, paint, run_heading, status, tool_call, tool_result, until,
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
}

#[test]
fn blocks_are_separated_by_blank_lines_and_notes_stay_together() {
    let mut spacing = Spacing::default();
    let separators = [
        spacing.before(Block::Prompt),
        spacing.before(Block::Message),
        spacing.before(Block::Note),
        spacing.before(Block::Note),
        spacing.before(Block::Approval),
        spacing.before(Block::Message),
    ];
    assert_eq!(separators, ["", "\n", "\n", "", "\n", "\n"]);
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
    assert_eq!(call("shell", json!({"command": "ls -la", "timeout": 30})), "shell: ls -la");
    assert_eq!(call("read_file", json!({"path": "/etc/hosts"})), "read_file: /etc/hosts");
    assert_eq!(call("fetch", json!({"url": "https://x"})), "fetch: https://x");
    assert_eq!(call("odd", json!({"n": 1})), r#"odd: {"n":1}"#);
    assert_eq!(call("echo", json!("hi")), "echo: hi");
    assert_eq!(call("noop", json!(null)), "noop");
}

#[test]
fn a_tool_call_of_several_lines_shows_the_first_and_how_many_follow() {
    let call = |command: &str, columns| tool_call("shell", &json!({ "command": command }), columns);
    assert_eq!(call(FROM_SRC, None), "shell: cd src (and 3 more lines)");
    assert_eq!(call(FAILED_UNITS, None), "shell: systemctl --failed --no-pager (and 1 more line)");
    // A cut keeps the count: the reader must see that more lines follow.
    assert_eq!(call(FAILED_UNITS, Some(32)), "shell: system\u{2026} (and 1 more line)");
    // Blank lines at the start and the end run nothing.
    assert_eq!(call("\n\nls -la\n\n", None), "shell: ls -la");
    assert_eq!(call("ls\n\n  \nuptime\n", None), "shell: ls (and 3 more lines)");
    // Control characters stay visible, a carriage return too.
    assert_eq!(call("a\rb\u{1b}[2J\nc", None), "shell: a\u{240d}b\u{241b}[2J (and 1 more line)");
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
            },
            ProviderStatus { provider: "anthropic".to_owned(), logged_in: false, expires_at: None },
        ],
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
