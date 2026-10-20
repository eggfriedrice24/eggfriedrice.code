use std::path::PathBuf;

use efr_protocol::RecentCommand;
use pretty_assertions::assert_eq;

use super::parse;

#[test]
fn the_plugins_entries_become_recent_commands_oldest_first() {
    let json = r#"[
        {"command":"git pull","status":0,"cwd":"/home/u/p","ended":1760954400,"duration_ms":1250},
        {"command":"make","status":2}
    ]"#;
    let recent = parse(json);
    assert_eq!(
        recent,
        [
            RecentCommand {
                status: Some(0),
                cwd: Some(PathBuf::from("/home/u/p")),
                ended_at: Some("2025-10-20T10:00:00Z".parse().unwrap()),
                duration_ms: Some(1250),
                ..RecentCommand::new("git pull")
            },
            RecentCommand { status: Some(2), ..RecentCommand::new("make") },
        ]
    );
}

#[test]
fn a_value_that_does_not_parse_gives_no_recent_commands() {
    for json in ["", "{", r#"{"command":"ls"}"#, r#"[{"status":0}]"#] {
        assert_eq!(parse(json), [], "{json}");
    }
}

#[test]
fn blank_lines_relative_dirs_and_bad_times_are_left_out() {
    let json = r#"[
        {"command":"  "},
        {"command":"ls","cwd":"rel/dir","ended":99999999999999}
    ]"#;
    assert_eq!(parse(json), [RecentCommand::new("ls")]);
}

#[test]
fn only_the_newest_ten_are_read() {
    let entries: Vec<String> = (0..12).map(|n| format!(r#"{{"command":"echo {n}"}}"#)).collect();
    let recent = parse(&format!("[{}]", entries.join(",")));
    assert_eq!(recent.len(), 10);
    assert_eq!(recent[0].command, "echo 2");
    assert_eq!(recent[9].command, "echo 11");
}
