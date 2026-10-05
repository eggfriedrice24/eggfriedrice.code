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
    Block, Spacing, Tone, ago, code_block, conversations, lines, one_line, paint, status,
    tool_call, tool_result, until,
};
use crate::testing::{conversation, now};

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
    assert_eq!(tool_call("shell", &json!({"command": "ls -la", "timeout": 30})), "shell: ls -la");
    assert_eq!(tool_call("read_file", &json!({"path": "/etc/hosts"})), "read_file: /etc/hosts");
    assert_eq!(tool_call("fetch", &json!({"url": "https://x"})), "fetch: https://x");
    assert_eq!(tool_call("odd", &json!({"n": 1})), r#"odd: {"n":1}"#);
    assert_eq!(tool_call("echo", &json!("hi")), "echo: hi");
    assert_eq!(tool_call("noop", &json!(null)), "noop");
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
