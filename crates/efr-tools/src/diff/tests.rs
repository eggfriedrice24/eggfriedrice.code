use std::path::Path;

use efr_protocol::MAX_CALL_DIFF_LINES;
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{
    Lines, MAX_BYTES, MAX_LINES, MAX_WORK, MAX_WRITTEN_BYTES, NO_NEWLINE, UNCHANGED, unified_diff,
    written_diff,
};

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
fn a_gap_too_large_for_the_work_budget_is_one_replacement() {
    let old: String = (0..1100).map(|n| format!("old {n}\n")).collect();
    let new: String = (0..1100).map(|n| format!("new {n}\n")).collect();
    let preview = diff(Some(&old), &new);
    let lines: Vec<&str> = preview.lines().collect();
    assert_eq!(lines[2], "@@ -1,1100 +1,1100 @@");
    assert!(lines[3..].iter().take(10).all(|line| line.starts_with("-old ")), "{preview}");
}

/// `pairs` times the lines `a` and `b`: neither line occurs once, so no line anchors.
fn repeated(pairs: usize, b: &str) -> String {
    (0..pairs).map(|_| format!("a\n{b}\n")).collect()
}

#[test]
fn all_gaps_share_one_work_budget() {
    // Two gaps of 800 by 800 lines around one anchor: the first table takes more than
    // half of the budget, so the second gap is one replacement.
    let pairs: usize = 400;
    assert!(2 * (2 * pairs).pow(2) > MAX_WORK && (2 * pairs).pow(2) < MAX_WORK);
    let old = format!("{}anchor\n{}", repeated(pairs, "b"), repeated(pairs, "b"));
    let new = format!("{}anchor\n{}", repeated(pairs, "c"), repeated(pairs, "c"));
    let written = written_diff(Path::new("/p/a.txt"), Some(&old), &new, usize::MAX).unwrap();
    let (first, second) = written.text.split_once("\n anchor\n").unwrap();
    // The table keeps every `a` of the first gap. The second gap keeps only its common
    // first line and removes and adds all the others.
    assert_eq!(first.lines().filter(|line| *line == " a").count(), pairs, "{first}");
    assert_eq!(second.lines().filter(|line| *line == " a").count(), 1, "{second}");
    let changed = pairs + 2 * pairs - 1;
    assert_eq!((written.added, written.removed), (changed, changed));
    assert_eq!(apply(&old, &written.text), new);
}

#[test]
fn two_far_changes_in_a_large_file_show_as_two_small_hunks() {
    let line = |at: usize| format!("line {at:06}: let value = parse(&input, Kind::Item)?;\n");
    let old: String = (0..3000).map(line).collect();
    let new: String = (0..3000)
        .map(|at| if at == 4 || at == 2995 { format!("changed {at}\n") } else { line(at) })
        .collect();
    let written =
        written_diff(Path::new("/p/big.rs"), Some(&old), &new, MAX_CALL_DIFF_LINES).unwrap();
    assert_eq!((written.added, written.removed), (2, 2));
    let lines: Vec<&str> = written.text.lines().collect();
    assert_eq!(lines.len(), 2 + 2 * 9, "{}", written.text);
    assert_eq!(lines[2], "@@ -2,7 +2,7 @@");
    assert_eq!(lines[11], "@@ -2993,7 +2993,7 @@");
    assert!(lines.contains(&"+changed 2995"), "{}", written.text);
}

#[test]
fn the_caps_of_the_preview_and_of_a_written_diff_are_kept() {
    // Lines of 100 bytes: the preview stops at 16 KiB before 200 lines.
    let long = "x".repeat(99);
    let new: String = (0..1000).map(|n| format!("{n:04}{long}\n")).collect();
    let preview = diff(None, &new);
    let lines: Vec<&str> = preview.lines().collect();
    let body: usize = lines[2..lines.len() - 1].iter().map(|line| line.len() + 1).sum();
    assert!(body <= MAX_BYTES && body + 105 > MAX_BYTES, "{body}");
    let shown = lines.len() - 3;
    assert!(shown < MAX_LINES);
    assert_eq!(
        lines.last().copied(),
        Some(&*format!("[... {} more lines of the diff]", 1001 - shown))
    );

    // A written diff stops at 256 KiB before its line cap and counts what it left out.
    let new: String = (0..5000).map(|n| format!("{n:04}{long}\n")).collect();
    let written =
        written_diff(Path::new("/p/a.txt"), None, &new, MAX_CALL_DIFF_LINES * 10).unwrap();
    let lines: Vec<&str> = written.text.lines().collect();
    let body: usize = lines[2..lines.len() - 1].iter().map(|line| line.len() + 1).sum();
    assert!(body <= MAX_WRITTEN_BYTES && body + 105 > MAX_WRITTEN_BYTES, "{body}");
    let shown = lines.len() - 3;
    assert_eq!(lines.last().copied(), Some(&*format!("... {} more lines", 5001 - shown)));
    assert_eq!((written.added, written.removed), (5000, 0));
}

#[test]
fn the_debug_form_of_a_written_diff_shows_no_text() {
    let written = written_diff(Path::new("/p/a.txt"), Some("token=old\n"), "token=new\n", 10);
    let debug = format!("{written:?}");
    assert!(!debug.contains("token"), "{debug}");
    assert!(debug.contains("text_bytes"), "{debug}");
}

#[test]
fn a_written_diff_counts_every_line_and_cuts_with_a_plain_note() {
    let old: String = (0..30).map(|n| format!("line {n}\n")).collect();
    let new: String = (0..30).map(|n| format!("LINE {n}\n")).collect();
    let written = written_diff(Path::new("/p/a.txt"), Some(&old), &new, 10).unwrap();
    assert_eq!((written.added, written.removed), (30, 30));
    let lines: Vec<&str> = written.text.lines().collect();
    assert_eq!(lines[0], "--- a/p/a.txt");
    assert_eq!(lines.len(), 2 + 10 + 1);
    assert_eq!(lines.last(), Some(&"... 51 more lines"));
}

#[test]
fn a_written_diff_of_a_new_file_and_of_no_change() {
    let written = written_diff(Path::new("/p/new.txt"), None, "a\nb\n", 2000).unwrap();
    assert_eq!((written.added, written.removed), (2, 0));
    assert_eq!(written.text, "--- /dev/null\n+++ b/p/new.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n");
    assert_eq!(written_diff(Path::new("/p/same"), Some("x\n"), "x\n", 2000), None);
}

/// `old` with the hunks of the unified diff `diff` applied. It panics when a kept or a
/// removed line or a hunk header does not match.
fn apply(old: &str, diff: &str) -> String {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let lines: Vec<&str> = diff
        .lines()
        .skip_while(|line| line.starts_with("--- ") || line.starts_with("+++ "))
        .collect();
    let mut out: Vec<String> = Vec::new();
    let (mut at, mut i) = (0, 0);
    while i < lines.len() {
        let header = lines[i].strip_prefix("@@ -").and_then(|rest| rest.strip_suffix(" @@"));
        let (from, to) = header.and_then(|range| range.split_once(" +")).unwrap();
        let range = |text: &str| -> (usize, usize) {
            let (start, count) = text.split_once(',').unwrap();
            let (start, count) = (start.parse::<usize>().unwrap(), count.parse().unwrap());
            (if count == 0 { start } else { start - 1 }, count)
        };
        let ((old_first, old_count), (new_first, new_count)) = (range(from), range(to));
        out.extend(old_lines[at..old_first].iter().map(|line| (*line).to_owned()));
        assert_eq!(out.len(), new_first, "{diff}");
        at = old_first;
        let (before_old, before_new) = (at, out.len());
        i += 1;
        while i < lines.len() && !lines[i].starts_with("@@") {
            let line = lines[i];
            let end = lines.get(i + 1) == Some(&NO_NEWLINE);
            let text = format!("{}{}", &line[1..], if end { "" } else { "\n" });
            match &line[..1] {
                " " => {
                    assert_eq!(old_lines[at], text, "{diff}");
                    out.push(text);
                    at += 1;
                }
                "-" => {
                    assert_eq!(old_lines[at], text, "{diff}");
                    at += 1;
                }
                "+" => out.push(text),
                other => panic!("a diff line starts with {other:?}: {diff}"),
            }
            i += if end { 2 } else { 1 };
        }
        assert_eq!((at - before_old, out.len() - before_new), (old_count, new_count), "{diff}");
    }
    out.extend(old_lines[at..].iter().map(|line| (*line).to_owned()));
    out.concat()
}

/// A text of the lines that `tokens` name. A token below 4 names a line that recurs,
/// any other one a line that may occur once. Without `end` the last line has no line
/// end.
fn text(tokens: &[u8], end: bool) -> String {
    let mut text: String = tokens.iter().map(|token| format!("line {token}\n")).collect();
    if !end {
        text.pop();
    }
    text
}

/// `old` with each edit applied: 0 deletes the line at the position, 1 inserts the
/// token before it, 2 changes it to the token.
fn edited(old: &[u8], edits: &[(usize, u8, u8)]) -> Vec<u8> {
    let mut new = old.to_vec();
    for &(at, kind, token) in edits {
        let at = if new.is_empty() { 0 } else { at % (new.len() + 1) };
        match kind {
            0 if at < new.len() => {
                new.remove(at);
            }
            2 if at < new.len() => new[at] = token,
            _ => new.insert(at, token),
        }
    }
    new
}

proptest! {
    #[test]
    fn applying_the_diff_to_the_old_text_gives_the_new_text(
        old in prop::collection::vec(0_u8..30, 0..60),
        edits in prop::collection::vec((0_usize..64, 0_u8..3, 0_u8..30), 0..10),
        other in prop::collection::vec(0_u8..30, 0..60),
        independent in any::<bool>(),
        old_end in any::<bool>(),
        new_end in any::<bool>(),
        budget in prop_oneof![Just(0_usize), 0_usize..400, Just(MAX_WORK)],
    ) {
        let new_tokens = if independent { other } else { edited(&old, &edits) };
        let (old, new) = (text(&old, old_end), text(&new_tokens, new_end));
        let lines = Lines::within(Some(&old), &new, budget);
        let shown: Vec<String> = lines.iter().collect();
        prop_assert_eq!(shown.len(), lines.len());
        let diff: String = shown.iter().map(|line| format!("{line}\n")).collect();
        prop_assert_eq!(apply(&old, &diff), new.clone());
        let marked = |mark: char| shown.iter().filter(|line| line.starts_with(mark)).count();
        prop_assert_eq!((marked('+'), marked('-')), (lines.added, lines.removed));
    }
}
