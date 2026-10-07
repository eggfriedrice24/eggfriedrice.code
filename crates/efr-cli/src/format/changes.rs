//! What a call or a turn changed in the files of its roots, as the daemon's snapshots
//! see them: the muted row under a call, the line at the end of a turn and the list of
//! `efr diff --stat`.
//!
//! ```text
//! changed src/a.rs +3 −1 · deleted old.rs · new notes.md (+2 more)
//! 3 files changed, +24 −7
//! ```
//!
//! Every path comes from the daemon and passes through [`one_line`]. Added lines are in
//! the `success` role and removed lines in the `error` role; the rest is muted.

use std::fmt::Write as _;

use efr_protocol::{ChangeKind, FileChange, FileChanges};
use efr_render::{RenderOptions, WidthMethod, text_width};

use super::{Tone, one_line, paint};

/// The minus sign of a count of removed lines: wider and clearer than a hyphen.
pub(crate) const MINUS: char = '\u{2212}';

/// The files that a call's row names; the rest are counted.
const ROW_FILES: usize = 3;

/// What joins the groups of a call's row.
const JOIN: &str = " \u{b7} ";

/// A piece of text with its tone.
pub(crate) type Piece = (String, Tone);

/// The word of a kind of change, in a call's row and in the list.
fn word(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "new",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed => "renamed",
        _ => "changed",
    }
}

/// The path of `file`, safe to print: `from → path` for a rename.
fn path(file: &FileChange) -> String {
    match (&file.from, file.kind) {
        (Some(from), ChangeKind::Renamed) => {
            format!("{} \u{2192} {}", one_line(from), one_line(&file.path))
        }
        _ => one_line(&file.path),
    }
}

/// `+3 −1` as pieces, a part left out when it is 0; nothing when both are.
pub(crate) fn counts(added: u32, removed: u32) -> Vec<Piece> {
    let mut pieces = Vec::new();
    if added > 0 {
        pieces.push((format!("+{added}"), Tone::Success));
    }
    if removed > 0 {
        if added > 0 {
            pieces.push((" ".to_owned(), Tone::Dim));
        }
        pieces.push((format!("{MINUS}{removed}"), Tone::Failure));
    }
    pieces
}

/// What follows the path of `file` in a call's row: `(binary)`, or the counts of a
/// changed or renamed file. A new or deleted file shows none: the word says it.
fn row_counts(file: &FileChange) -> Vec<Piece> {
    if file.binary && file.kind != ChangeKind::Deleted {
        return vec![(" (binary)".to_owned(), Tone::Dim)];
    }
    match file.kind {
        ChangeKind::Added | ChangeKind::Deleted => Vec::new(),
        _ => {
            let counts = counts(file.added, file.removed);
            if counts.is_empty() {
                return counts;
            }
            let mut pieces = vec![(" ".to_owned(), Tone::Dim)];
            pieces.extend(counts);
            pieces
        }
    }
}

/// The row of changes under a call, such as `changed src/a.rs +3 −1 · deleted old.rs ·
/// new notes.md (+2 more)`: the first files, grouped by their kind in the order the
/// kinds first come, then how many more files changed. Empty when nothing changed.
pub(crate) fn call_row(changes: &FileChanges) -> Vec<Piece> {
    let shown = &changes.files[..changes.files.len().min(ROW_FILES)];
    let hidden = changes.files.len() - shown.len() + usize::try_from(changes.more).unwrap_or(0);
    let mut kinds: Vec<ChangeKind> = Vec::new();
    for file in shown {
        if !kinds.contains(&file.kind) {
            kinds.push(file.kind);
        }
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for (at, kind) in kinds.iter().enumerate() {
        if at > 0 {
            pieces.push((JOIN.to_owned(), Tone::Dim));
        }
        pieces.push((format!("{} ", word(*kind)), Tone::Dim));
        for (number, file) in shown.iter().filter(|file| file.kind == *kind).enumerate() {
            if number > 0 {
                pieces.push((", ".to_owned(), Tone::Dim));
            }
            pieces.push((path(file), Tone::Dim));
            pieces.extend(row_counts(file));
        }
    }
    match (pieces.is_empty(), hidden) {
        (_, 0) => {}
        (true, hidden) => pieces.push((files_changed(hidden), Tone::Dim)),
        (false, hidden) => pieces.push((format!(" (+{hidden} more)"), Tone::Dim)),
    }
    pieces
}

/// `1 file changed` or `3 files changed`.
fn files_changed(count: usize) -> String {
    let unit = if count == 1 { "file" } else { "files" };
    format!("{count} {unit} changed")
}

/// The number of files in `changes`, those left out of its list too.
fn file_count(changes: &FileChanges) -> usize {
    changes.files.len() + usize::try_from(changes.more).unwrap_or(0)
}

/// The line at the end of a turn that changed files, such as `3 files changed, +24 −7`;
/// `None` when it changed none.
pub(crate) fn turn_line(changes: &FileChanges) -> Option<String> {
    let count = file_count(changes);
    if count == 0 {
        return None;
    }
    let mut line = files_changed(count);
    let counts: String =
        counts(changes.added, changes.removed).into_iter().map(|(text, _)| text).collect();
    if !counts.is_empty() {
        let _ = write!(line, ", {counts}");
    }
    Some(line)
}

/// `efr diff --stat`: one line per file (its kind, its path and its counts, or
/// `(binary)`), the files left out, then the totals. The paths start in one column.
pub(crate) fn stat(changes: &FileChanges, options: &RenderOptions) -> String {
    let method = options.width_method();
    let paths: Vec<String> = changes.files.iter().map(path).collect();
    let widest = paths.iter().map(|path| text_width(path, method)).max().unwrap_or(0);
    let mut out = String::new();
    for (file, path) in changes.files.iter().zip(&paths) {
        out.push_str(&paint(&format!("{:<8}", word(file.kind)), Tone::Dim, options));
        out.push_str(path);
        let counts = if file.binary {
            vec![("(binary)".to_owned(), Tone::Dim)]
        } else {
            counts(file.added, file.removed)
        };
        if !counts.is_empty() {
            out.push_str(&pad(path, widest, method));
            out.push_str("  ");
            for (text, tone) in &counts {
                out.push_str(&paint(text, *tone, options));
            }
        }
        out.push('\n');
    }
    if changes.more > 0 {
        let unit = if changes.more == 1 { "file" } else { "files" };
        let more = format!("\u{2026} {} more {unit}", changes.more);
        let _ = writeln!(out, "{}", paint(&more, Tone::Dim, options));
    }
    if let Some(line) = turn_line(changes) {
        let _ = writeln!(out, "{}", paint(&line, Tone::Dim, options));
    }
    out
}

/// The lines of a file header of a diff, which the call above it names already.
const HEADER_LINES: &[&str] = &[
    "diff --git ",
    "index ",
    "--- ",
    "+++ ",
    "new file mode",
    "deleted file mode",
    "old mode",
    "new mode",
    "similarity index",
    "rename from",
    "rename to",
];

/// A diff, split for the rows under a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffParts<'a> {
    /// The file headers before the first hunk: they repeat what the call names, but
    /// they give the file's syntax colours.
    pub(crate) head: Vec<&'a str>,
    /// The lines from the first hunk on.
    pub(crate) body: Vec<&'a str>,
    /// The lines that the daemon left out, from its last line `... N more lines`.
    pub(crate) cut: usize,
}

/// `diff` split into its file headers, its lines and the lines that the daemon cut.
/// Headers count only when every line before the first hunk is one.
pub(crate) fn diff_parts(diff: &str) -> DiffParts<'_> {
    let (diff, cut) = split_cut(diff);
    let mut lines: Vec<&str> = diff.lines().collect();
    let head = match lines.iter().position(|line| line.starts_with("@@")) {
        Some(at) if lines[..at].iter().all(|line| is_header(line)) => at,
        _ => 0,
    };
    let body = lines.split_off(head);
    DiffParts { head: lines, body, cut }
}

fn is_header(line: &str) -> bool {
    HEADER_LINES.iter().any(|start| line.starts_with(start))
}

/// `diff` without the daemon's last line `... 12 more lines` of a diff that it cut, and
/// that count; the whole diff and 0 without that line.
pub(crate) fn split_cut(diff: &str) -> (&str, usize) {
    let text = diff.trim_end_matches('\n');
    let (before, last) = match text.rfind('\n') {
        Some(at) => (&text[..=at], &text[at + 1..]),
        None => ("", text),
    };
    match cut_lines(last) {
        Some(count) => (before, count),
        None => (diff, 0),
    }
}

/// The count of the daemon's last line of a diff that it cut, `... 12 more lines`.
fn cut_lines(line: &str) -> Option<usize> {
    let rest = line.trim().strip_prefix("... ")?;
    let (count, unit) = rest.split_once(' ')?;
    if matches!(unit, "more lines" | "more line") { count.parse().ok() } else { None }
}

/// `… 12 more lines`, the row after the lines of a diff that are shown.
pub(crate) fn more_lines(count: usize) -> String {
    let unit = if count == 1 { "line" } else { "lines" };
    format!("\u{2026} {count} more {unit}")
}

/// The spaces that bring `text` to `width` columns.
fn pad(text: &str, width: usize, method: WidthMethod) -> String {
    " ".repeat(width.saturating_sub(text_width(text, method)))
}

#[cfg(test)]
mod tests;
