use std::path::Path;

use efr_protocol::RecentCommand;
use insta::assert_snapshot;
use jiff::{SignedDuration, Timestamp};
use pretty_assertions::assert_eq;

use super::{LIST_MAX_CHARS, newest, render};

const CWD: &str = "/home/u/p/efr";

fn now() -> Timestamp {
    "2026-10-20T12:00:00Z".parse().unwrap()
}

/// A command that ended `ago_secs` seconds before [`now`], in [`CWD`], with status 0.
fn ran(command: &str, ago_secs: i64) -> RecentCommand {
    RecentCommand {
        status: Some(0),
        cwd: Some(CWD.into()),
        ended_at: Some(now() - SignedDuration::from_secs(ago_secs)),
        ..RecentCommand::new(command)
    }
}

fn list(commands: &[RecentCommand], last: Option<&str>) -> String {
    render(commands, last, Path::new(CWD), now())
}

#[test]
fn the_list_shows_each_command_oldest_first_with_its_facts() {
    let commands = [
        RecentCommand { cwd: Some("/etc".into()), ..ran("cat hosts", 2 * 86_400) },
        RecentCommand { duration_ms: Some(150_000), status: Some(101), ..ran("cargo build", 300) },
        RecentCommand { duration_ms: Some(400), ..ran("git status", 20) },
        RecentCommand::new("ls"),
        RecentCommand { status: Some(1), ..ran("for f in *.log; do\n  gzip \"$f\"\ndone", 7_200) },
        ran("make test", 61),
    ];
    assert_snapshot!(list(&commands, Some("make check")));
}

#[test]
fn the_newest_entry_goes_when_it_is_the_last_command() {
    let commands = [ran("make", 600), ran("vim Makefile", 300), ran("make", 5)];
    let text = list(&commands, Some("make  "));
    assert_eq!(text.matches("`make`").count(), 1, "{text}");
    assert!(text.contains("- `make` (status 0, ended 10 min ago)\n"), "{text}");
    assert!(text.contains("- `vim Makefile` (status 0, ended 5 min ago)\n"), "{text}");
    assert_eq!(list(&[ran("make", 5)], Some("make")), "", "nothing left, no list");
}

#[test]
fn a_secret_in_a_recent_command_is_redacted() {
    let text = list(&[ran("export TOKEN=hunter2 && make", 30)], None);
    assert!(text.contains("- `export TOKEN=[redacted] && make` (status 0, ended just now)"));
    assert!(!text.contains("hunter2"), "{text}");
}

#[test]
fn a_long_recent_command_is_cut() {
    let text = list(&[ran(&format!("echo {}", "x".repeat(500)), 30)], None);
    assert!(text.contains(" [cut]` (status 0"), "{text}");
    assert!(!text.contains(&"x".repeat(400)), "{text}");
}

#[test]
fn the_oldest_entries_go_when_the_list_is_too_long() {
    let commands: Vec<_> =
        (0..10).map(|n| ran(&format!("echo {n} {}", "y".repeat(380)), 600 - n)).collect();
    let text = list(&commands, None);
    assert!(text.chars().count() <= LIST_MAX_CHARS + 100, "{}", text.len());
    assert!(text.contains("echo 9 "), "the newest stays: {text}");
    assert!(!text.contains("echo 0 "), "the oldest goes: {text}");
    let first = text.find("echo 6 ").unwrap();
    assert!(first < text.find("echo 9 ").unwrap(), "still oldest first");
}

#[test]
fn a_long_directory_is_cut_and_the_newest_entry_always_stays() {
    let deep = format!("/{}", "d".repeat(2_500));
    let commands = [ran("echo old", 60), RecentCommand { cwd: Some(deep.into()), ..ran("ls", 30) }];
    let text = list(&commands, None);
    assert!(
        text.contains(&format!(
            "- `ls` (status 0, ended just now, in /{} [cut])\n",
            "d".repeat(199)
        )),
        "{text}"
    );
    assert!(text.contains("- `echo old` (status 0, ended 1 min ago)\n"), "{text}");
    assert!(text.chars().count() <= LIST_MAX_CHARS + 100, "{}", text.len());
}

#[test]
fn an_end_after_now_reads_as_just_now() {
    let text = list(&[ran("date", -30)], None);
    assert!(text.contains("ended just now"), "{text}");
}

#[test]
fn blank_commands_and_no_commands_show_no_list() {
    assert_eq!(list(&[], None), "");
    assert_eq!(list(&[RecentCommand::new("  ")], None), "");
}

#[test]
fn newest_keeps_the_newest_count() {
    let commands = [ran("a", 3), ran("b", 2), ran("c", 1)];
    let names = |kept: Vec<RecentCommand>| -> Vec<String> {
        kept.into_iter().map(|command| command.command).collect()
    };
    assert_eq!(names(newest(&commands, 2)), ["b", "c"]);
    assert_eq!(names(newest(&commands, 8)), ["a", "b", "c"]);
    assert!(newest(&commands, 0).is_empty());
}
