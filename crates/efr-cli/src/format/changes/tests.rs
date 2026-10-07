use efr_protocol::{ChangeKind, FileChange, FileChanges};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;

use super::{DiffParts, call_row, diff_parts, more_lines, stat, turn_line};
use crate::format::Tone;
use crate::testing::readable;

fn file(path: &str, kind: ChangeKind, added: u32, removed: u32) -> FileChange {
    FileChange { path: path.to_owned(), kind, from: None, added, removed, binary: false }
}

fn changes(files: Vec<FileChange>, more: u32) -> FileChanges {
    let added = files.iter().map(|file| file.added).sum();
    let removed = files.iter().map(|file| file.removed).sum();
    FileChanges { files, more, added, removed }
}

fn text(pieces: &[(String, Tone)]) -> String {
    pieces.iter().map(|(text, _)| text.as_str()).collect()
}

#[test]
fn a_row_names_the_first_files_by_kind_and_counts_the_rest() {
    let changes = changes(
        vec![
            file("src/a.rs", ChangeKind::Modified, 3, 1),
            file("old.rs", ChangeKind::Deleted, 0, 40),
            file("notes.md", ChangeKind::Added, 12, 0),
            file("b.rs", ChangeKind::Modified, 1, 0),
        ],
        1,
    );
    assert_eq!(
        text(&call_row(&changes)),
        "changed src/a.rs +3 \u{2212}1 \u{b7} deleted old.rs \u{b7} new notes.md (+2 more)"
    );
}

#[test]
fn files_of_one_kind_share_their_word() {
    let changes = changes(
        vec![
            file("a.rs", ChangeKind::Modified, 3, 1),
            file("gone.rs", ChangeKind::Deleted, 0, 2),
            file("b.rs", ChangeKind::Modified, 0, 2),
        ],
        0,
    );
    assert_eq!(
        text(&call_row(&changes)),
        "changed a.rs +3 \u{2212}1, b.rs \u{2212}2 \u{b7} deleted gone.rs"
    );
}

#[test]
fn a_rename_shows_both_paths_and_a_binary_file_no_counts() {
    let mut renamed = file("src/new.rs", ChangeKind::Renamed, 1, 1);
    renamed.from = Some("src/old.rs".to_owned());
    let mut logo = file("logo.png", ChangeKind::Modified, 0, 0);
    logo.binary = true;
    let changes = changes(vec![renamed, logo], 0);
    assert_eq!(
        text(&call_row(&changes)),
        "renamed src/old.rs \u{2192} src/new.rs +1 \u{2212}1 \u{b7} changed logo.png (binary)"
    );
}

#[test]
fn the_counts_are_in_the_success_and_error_roles_and_the_rest_is_muted() {
    let changes = changes(vec![file("a.rs", ChangeKind::Modified, 3, 1)], 0);
    let row = call_row(&changes);
    assert!(row.contains(&("+3".to_owned(), Tone::Success)), "{row:?}");
    assert!(row.contains(&("\u{2212}1".to_owned(), Tone::Failure)), "{row:?}");
    assert!(row.iter().all(|(_, tone)| matches!(tone, Tone::Dim | Tone::Success | Tone::Failure)));
}

#[test]
fn a_path_cannot_drive_the_terminal() {
    let changes = changes(vec![file("a\x1b[2Jb.rs", ChangeKind::Added, 1, 0)], 0);
    assert_eq!(text(&call_row(&changes)), "new a\u{241b}[2Jb.rs");
}

#[test]
fn nothing_changed_gives_no_row_and_no_line() {
    let none = FileChanges::default();
    assert!(call_row(&none).is_empty());
    assert_eq!(turn_line(&none), None);
    // Only files that the list left out still count.
    let left_out = FileChanges { more: 3, ..FileChanges::default() };
    assert_eq!(text(&call_row(&left_out)), "3 files changed");
    assert_eq!(turn_line(&left_out).as_deref(), Some("3 files changed"));
}

#[test]
fn the_line_of_a_turn_counts_files_and_lines() {
    let three = changes(
        vec![
            file("a.rs", ChangeKind::Modified, 20, 7),
            file("b.rs", ChangeKind::Added, 4, 0),
            file("c.rs", ChangeKind::Deleted, 0, 0),
        ],
        0,
    );
    assert_eq!(turn_line(&three).as_deref(), Some("3 files changed, +24 \u{2212}7"));
    let one = changes(vec![file("a.rs", ChangeKind::Modified, 5, 0)], 0);
    assert_eq!(turn_line(&one).as_deref(), Some("1 file changed, +5"));
}

#[test]
fn a_diff_splits_into_its_headers_its_lines_and_the_cut() {
    let diff = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n... 120 more lines\n";
    assert_eq!(
        diff_parts(diff),
        DiffParts {
            head: vec!["diff --git a/a.rs b/a.rs", "index 1..2 100644", "--- a/a.rs", "+++ b/a.rs"],
            body: vec!["@@ -1 +1 @@", "-old", "+new"],
            cut: 120,
        }
    );
    // Without headers, and with a line before the first hunk that is not one, nothing
    // is a header.
    assert_eq!(diff_parts("@@ -1 +1 @@\n-a\n+b\n").head, Vec::<&str>::new());
    let odd = diff_parts("note\n--- a/a.rs\n@@ -1 +1 @@\n");
    assert_eq!((odd.head.len(), odd.body.len()), (0, 3));
    // A line of the diff that only looks like the cut stays a line.
    assert_eq!(diff_parts("+... 3 more lines\n").cut, 0);
    assert_eq!(more_lines(1), "\u{2026} 1 more line");
}

#[test]
fn the_stat_lists_each_file_with_its_counts() {
    let mut renamed = file("src/new.rs", ChangeKind::Renamed, 0, 0);
    renamed.from = Some("src/old.rs".to_owned());
    let mut logo = file("logo.png", ChangeKind::Added, 0, 0);
    logo.binary = true;
    let changes = changes(
        vec![
            file("src/a.rs", ChangeKind::Modified, 3, 1),
            renamed,
            logo,
            file("~/notes/old.md", ChangeKind::Deleted, 0, 12),
        ],
        2,
    );
    let mut shown = Vec::new();
    for (name, options) in [
        ("colour", RenderOptions::new(80)),
        ("NO_COLOR", RenderOptions::new(80).with_colour(ColourMode::None)),
        ("not a terminal", RenderOptions::new(80).with_terminal(false)),
    ] {
        shown.push(format!("=== {name}\n{}", readable(&stat(&changes, &options))));
    }
    insta::assert_snapshot!(shown.join("\n"));
}
