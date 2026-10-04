use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{Block, Spacing, Tone, code_block, lines, one_line, paint, tool_call, tool_result};

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
        spacing.before(Block::Message),
        spacing.before(Block::Note),
        spacing.before(Block::Note),
        spacing.before(Block::Approval),
        spacing.before(Block::Message),
    ];
    assert_eq!(separators, ["", "\n", "", "\n", "\n"]);
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
