use std::path::Path;

use pretty_assertions::assert_eq;

use super::{MAX_LINES, UNCHANGED, unified_diff};

fn diff(old: Option<&str>, new: &str) -> String {
    unified_diff(Path::new("/home/u/.zshrc"), old, new)
}

#[test]
fn a_changed_line_shows_with_its_context() {
    let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
    let new = "a\nb\nc\nd\nE\nf\ng\nh\n";
    assert_eq!(
        diff(Some(old), new),
        "--- a/home/u/.zshrc\n+++ b/home/u/.zshrc\n@@ -2,7 +2,7 @@\n b\n c\n d\n-e\n+E\n f\n g\n h\n"
    );
}

#[test]
fn distant_changes_get_hunks_of_their_own() {
    let line = |n: u32| match n {
        2 => "two\n".to_owned(),
        19 => "nineteen\n".to_owned(),
        n => format!("{n}\n"),
    };
    let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
    let new: String = (1..=20).map(line).collect();
    assert_eq!(
        diff(Some(&old), &new),
        "--- a/home/u/.zshrc\n+++ b/home/u/.zshrc\n\
         @@ -1,5 +1,5 @@\n 1\n-2\n+two\n 3\n 4\n 5\n\
         @@ -16,5 +16,5 @@\n 16\n 17\n 18\n-19\n+nineteen\n 20\n"
    );
}

#[test]
fn lines_added_at_the_end_keep_the_old_last_lines_as_context() {
    let old = "export PATH\nalias ll='ls -l'\n";
    let new = "export PATH\nalias ll='ls -l'\nalias la='ls -a'\n";
    assert_eq!(
        diff(Some(old), new),
        "--- a/home/u/.zshrc\n+++ b/home/u/.zshrc\n@@ -1,2 +1,3 @@\n export PATH\n alias ll='ls -l'\n+alias la='ls -a'\n"
    );
}

#[test]
fn a_new_file_shows_every_line_as_added() {
    assert_eq!(
        diff(None, "one\ntwo\n"),
        "--- /dev/null\n+++ b/home/u/.zshrc\n@@ -0,0 +1,2 @@\n+one\n+two\n"
    );
}

#[test]
fn emptying_a_file_shows_every_line_as_removed() {
    assert_eq!(
        diff(Some("one\ntwo\n"), ""),
        "--- a/home/u/.zshrc\n+++ b/home/u/.zshrc\n@@ -1,2 +0,0 @@\n-one\n-two\n"
    );
}

#[test]
fn a_missing_newline_at_the_end_is_marked() {
    assert_eq!(
        diff(Some("a\nb"), "a\nb\n"),
        "--- a/home/u/.zshrc\n+++ b/home/u/.zshrc\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"
    );
}

#[test]
fn the_same_content_says_so() {
    assert_eq!(diff(Some("same\n"), "same\n"), UNCHANGED);
}

#[test]
fn a_long_diff_stops_and_says_how_much_is_left() {
    let new: String = (0..500).map(|n| format!("line {n}\n")).collect();
    let preview = diff(None, &new);
    let lines: Vec<&str> = preview.lines().collect();
    assert_eq!(lines.len(), 2 + MAX_LINES + 1);
    assert_eq!(lines[2], "@@ -0,0 +1,500 @@");
    assert_eq!(lines.last(), Some(&"[... 301 more lines of the diff]"));
}

#[test]
fn a_middle_too_large_for_the_table_is_one_replacement() {
    let old: String = (0..1100).map(|n| format!("old {n}\n")).collect();
    let new: String = (0..1100).map(|n| format!("new {n}\n")).collect();
    let preview = diff(Some(&old), &new);
    let lines: Vec<&str> = preview.lines().collect();
    assert_eq!(lines[2], "@@ -1,1100 +1,1100 @@");
    assert!(lines[3..].iter().take(10).all(|line| line.starts_with("-old ")), "{preview}");
}
