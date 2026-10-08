//! The `apply_patch` tool as the CLI shows it: the line of a call with each file of
//! its patch and their counts, and a diff of several files cut into one block per
//! file.
//!
//! ```text
//! · apply_patch src/a.rs +3 −1, new notes.md +2, delete old.rs
//!   src/a.rs
//!   │ @@ -1,2 +1,2 @@
//!   │ -    old();
//!   │ +    new();
//!   new notes.md
//!   │ @@ -0,0 +1,2 @@
//!   │ +# Notes
//!   │ +
//!   deleted old.rs
//!   │ @@ -1,1 +0,0 @@
//!   │ -fn old() {}
//!   ✓
//! ```
//!
//! The patch is the model's text in the format of Codex (`*** Begin Patch` to
//! `*** End Patch`). The CLI reads only its file lines and counts its `+` and `-`
//! lines. It does not check the patch: the daemon's engine does that, and a patch
//! that does not apply fails the call. Every path passes through [`one_line`].
//!
//! The daemon sends the diff of a patch as the diffs of its files one after another,
//! in the order of the patch, each with its `---` and `+++` lines. An approval's
//! preview puts a line `delete <path>` or `move <from> -> <to>` before the diff of a
//! file that a patch deletes or moves.

use efr_render::{RenderOptions, WidthMethod, diff_rows, text_width};
use serde_json::Value;

use super::changes::{DiffParts, MINUS, Piece, more_lines};
use super::{Tone, columns, one_line, paint, wrap_spans};

/// The name of the tool.
pub(crate) const TOOL: &str = "apply_patch";

/// The member that holds the patch in the function form of the tool, the same name as
/// `efr_tools::FREEFORM_INPUT`.
const INPUT: &str = "input";

/// What joins the files of a call's line.
const JOIN: &str = ", ";

/// The arrow between the two paths of a move.
const ARROW: &str = " \u{2192} ";

/// The fewest columns a row of a diff gets, so a very narrow screen still shows a few
/// characters of each row.
const MIN_DIFF_WIDTH: usize = 10;

/// The text of a patch in the input of an `apply_patch` call: a JSON string in the
/// freeform form, or the string member `input` in the function form.
pub(crate) fn text(input: &Value) -> Option<&str> {
    match input {
        Value::String(text) => Some(text),
        Value::Object(members) => members.get(INPUT).and_then(Value::as_str),
        _ => None,
    }
}

/// What a patch does to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    /// `*** Update File:` without a move.
    Update,
    /// `*** Add File:`.
    Add,
    /// `*** Delete File:`.
    Delete,
    /// `*** Update File:` with `*** Move to:` this path.
    Move(String),
}

/// One file of a patch, with the lines that its patch adds and removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PatchFile {
    /// The path as the patch writes it.
    pub(crate) path: String,
    pub(crate) op: Op,
    pub(crate) added: u32,
    pub(crate) removed: u32,
}

/// The files of the patch `text`, in its order. Only the file lines and the lines of
/// their changes count: text before `*** Begin Patch` or after `*** End Patch`, and
/// any line that is not a change, says nothing here. The changes of one file that the
/// patch updates twice add up.
pub(crate) fn files(text: &str) -> Vec<PatchFile> {
    let mut files: Vec<PatchFile> = Vec::new();
    // The file whose lines follow, when they are lines of changes.
    let mut current: Option<usize> = None;
    for line in text.lines() {
        // NOTE: the engine accepts spaces around a marker line, as models often write
        // them; a line of a change keeps its first character.
        let marker = line.trim();
        let file = |path: &str, op: Op| PatchFile {
            path: path.trim().to_owned(),
            op,
            added: 0,
            removed: 0,
        };
        if let Some(path) = marker.strip_prefix("*** Add File:") {
            files.push(file(path, Op::Add));
            current = Some(files.len() - 1);
        } else if let Some(path) = marker.strip_prefix("*** Delete File:") {
            files.push(file(path, Op::Delete));
            current = None;
        } else if let Some(path) = marker.strip_prefix("*** Update File:") {
            let path = path.trim();
            current = match files.iter().position(|seen| seen.path == path && seen.op == Op::Update)
            {
                Some(at) => Some(at),
                None => {
                    files.push(file(path, Op::Update));
                    Some(files.len() - 1)
                }
            };
        } else if let Some(to) = marker.strip_prefix("*** Move to:") {
            if let Some(file) = current.and_then(|at| files.get_mut(at)) {
                file.op = Op::Move(to.trim().to_owned());
            }
        } else if marker == "*** End Patch" {
            break;
        } else if marker.starts_with("*** ") {
            // `*** Begin Patch`, `*** End of File`: no change.
        } else if let Some(file) = current.and_then(|at| files.get_mut(at)) {
            match line.as_bytes().first() {
                Some(b'+') => file.added = file.added.saturating_add(1),
                Some(b'-') => file.removed = file.removed.saturating_add(1),
                _ => {}
            }
        }
    }
    files
}

/// The files of a patch as the line of its call, such as `src/a.rs +3 −1, new
/// notes.md +2, delete old.rs, move a.rs → b.rs +1 −1`. Empty for a patch without a
/// file.
pub(crate) fn summary(files: &[PatchFile]) -> String {
    entries(files).join(JOIN)
}

/// Each file of a patch as its entry in the line of its call, such as `src/a.rs +3
/// −1` or `delete old.rs`, safe to print.
pub(crate) fn entries(files: &[PatchFile]) -> Vec<String> {
    files
        .iter()
        .map(|file| {
            let path = one_line(&file.path);
            let mut part = match &file.op {
                Op::Update => path,
                Op::Add => format!("new {path}"),
                Op::Delete => return format!("delete {path}"),
                Op::Move(to) => format!("move {path}{ARROW}{}", one_line(to)),
            };
            if file.added > 0 {
                part.push_str(&format!(" +{}", file.added));
            }
            if file.removed > 0 {
                part.push_str(&format!(" {MINUS}{}", file.removed));
            }
            part
        })
        .collect()
}

/// The entries of a call's line as rows of `room` columns, as a terminal that counts
/// widths by `method` shows them: as many entries in a row as fit, joined by `, `, and
/// a row cut only between two entries. An entry wider than a row is cut at its spaces,
/// or inside a word when one word is wider. Nothing of the entries is left out.
pub(crate) fn flow(entries: &[String], room: usize, method: WidthMethod) -> Vec<String> {
    let room = room.max(1);
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    for (at, entry) in entries.iter().enumerate() {
        let piece = if at + 1 < entries.len() { format!("{entry},") } else { entry.clone() };
        let width = text_width(&piece, method);
        if !row.is_empty() && text_width(&row, method) + 1 + width > room {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
            row.push_str(&piece);
        } else if width <= room {
            row = piece;
        } else {
            let mut cut: Vec<String> = wrap_spans(&[(piece, Tone::Plain)], room, room, method)
                .into_iter()
                .map(|pieces| pieces.into_iter().map(|(text, _)| text).collect())
                .collect();
            row = cut.pop().unwrap_or_default();
            rows.extend(cut);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// The file of one diff in a diff of several files, as its headers name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffFile {
    /// The diff has no headers that name a file.
    Unknown,
    /// The file changed in place.
    Update(String),
    /// The file is new.
    Add(String),
    /// The file is gone.
    Delete(String),
    /// The file moved, and may have changed too.
    Move { from: String, to: String },
}

impl DiffFile {
    /// The file whose syntax colours the diff takes: where it is after the change.
    pub(crate) fn path(&self) -> Option<&str> {
        match self {
            DiffFile::Unknown => None,
            DiffFile::Update(path) | DiffFile::Add(path) | DiffFile::Delete(path) => Some(path),
            DiffFile::Move { to, .. } => Some(to),
        }
    }

    /// The row that names the file before its diff, as pieces safe to print; empty for a
    /// diff without a name. In a question (`asking`) a delete and a move are in the
    /// `warning` role, as what a "yes" allows: `delete old.rs`, `move a.rs → b.rs`. Under
    /// a call that ran, the words say what happened, muted: `deleted old.rs`.
    pub(crate) fn heading(&self, asking: bool) -> Vec<Piece> {
        let path_tone = if asking { Tone::Plain } else { Tone::Code };
        let marked = |word: &str, done: &str, what: String| {
            if asking {
                vec![(format!("{word} {what}"), Tone::Attention)]
            } else {
                vec![(format!("{done} "), Tone::Dim), (what, path_tone)]
            }
        };
        match self {
            DiffFile::Unknown => Vec::new(),
            DiffFile::Update(path) => vec![(one_line(path), path_tone)],
            DiffFile::Add(path) => {
                vec![("new ".to_owned(), Tone::Dim), (one_line(path), path_tone)]
            }
            DiffFile::Delete(path) => marked("delete", "deleted", one_line(path)),
            DiffFile::Move { from, to } => {
                marked("move", "moved", format!("{}{ARROW}{}", one_line(from), one_line(to)))
            }
        }
    }
}

/// The diff of one file in a diff of several files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileDiff<'a> {
    pub(crate) file: DiffFile,
    pub(crate) parts: DiffParts<'a>,
}

impl FileDiff<'_> {
    /// The diff as text again, without the line that marks a delete or a move: its
    /// headers, its lines and the daemon's line `... N more lines`.
    pub(crate) fn text(&self) -> String {
        let mut text = String::new();
        for line in self.parts.head.iter().chain(&self.parts.body) {
            text.push_str(line);
            text.push('\n');
        }
        if self.parts.cut > 0 {
            text.push_str(&format!("... {} more lines\n", self.parts.cut));
        }
        text
    }
}

/// The line of a preview that marks a delete or a move: `delete <path>`, `move <from>
/// -> <to>`.
fn mark(line: &str) -> Option<DiffFile> {
    if let Some(path) = line.strip_prefix("delete ") {
        return Some(DiffFile::Delete(path.to_owned()));
    }
    let (from, to) = line.strip_prefix("move ")?.split_once(" -> ")?;
    Some(DiffFile::Move { from: from.to_owned(), to: to.to_owned() })
}

/// The lines that a hunk header `@@ -a,b +c,d @@` says follow, old and new; `None`
/// when it gives no counts.
fn hunk_counts(line: &str) -> Option<(u32, u32)> {
    let mut words = line.split(' ').skip(1);
    let count = |word: Option<&str>, sign: char| -> Option<u32> {
        let range = word?.strip_prefix(sign)?;
        match range.split_once(',') {
            Some((_, count)) => count.parse().ok(),
            None => range.parse::<u32>().ok().map(|_| 1),
        }
    };
    Some((count(words.next(), '-')?, count(words.next(), '+')?))
}

/// A line that starts the headers of a file before its first hunk, without naming it.
fn starts_file(line: &str) -> bool {
    line.starts_with("diff --git ")
}

/// A line of the headers of a file other than `---` and `+++`.
fn pre_header(line: &str) -> bool {
    super::changes::is_header(line) && !line.starts_with("--- ") && !line.starts_with("+++ ")
}

/// `diff` cut into the diffs of its files, in order: a file starts at a `diff --git`
/// line, at a `---` line followed by a `+++` line, or at a line that marks a delete
/// or a move, outside a hunk. The lines of a hunk are counted from its header, so a
/// removed line `-- note` (shown as `--- note`) stays a line of its hunk. A diff of one
/// file is one item; an empty diff has none.
pub(crate) fn file_diffs(diff: &str) -> Vec<FileDiff<'_>> {
    let lines: Vec<&str> = diff.lines().collect();
    let mut sections: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    // The old and new lines left in the current hunk: `Some(None)` for a hunk whose
    // header gave no counts.
    let mut hunk: Option<Option<(u32, u32)>> = None;
    for (at, line) in lines.iter().copied().enumerate() {
        let file_header = line.starts_with("--- ")
            && lines.get(at + 1).is_some_and(|next| next.starts_with("+++ "));
        if let Some(left) = &mut hunk {
            if in_hunk(line, left, file_header) {
                current.push(line);
                if matches!(left, Some((0, 0))) {
                    hunk = None;
                }
                continue;
            }
            hunk = None;
        }
        let split = if mark(line).is_some() {
            !current.is_empty()
        } else if starts_file(line) {
            current.iter().any(|line| mark(line).is_none())
        } else if file_header {
            current.iter().any(|line| mark(line).is_none() && !pre_header(line))
        } else {
            false
        };
        if split {
            sections.push(std::mem::take(&mut current));
        }
        if line.starts_with("@@") {
            hunk = Some(hunk_counts(line));
        }
        current.push(line);
    }
    if !current.is_empty() {
        sections.push(current);
    }
    sections.into_iter().map(section).collect()
}

/// True when `line` is a line of the hunk that has `left` lines to go, and counts it.
fn in_hunk(line: &str, left: &mut Option<(u32, u32)>, file_header: bool) -> bool {
    let first = line.as_bytes().first().copied();
    match left {
        Some((old, new)) => {
            let (old_step, new_step) = match first {
                Some(b' ') | None => (1, 1),
                Some(b'-') => (1, 0),
                Some(b'+') => (0, 1),
                Some(b'\\') => (0, 0),
                _ => return false,
            };
            if *old < old_step || *new < new_step {
                return false;
            }
            *old -= old_step;
            *new -= new_step;
            true
        }
        // NOTE: without counts, the headers of the next file end the hunk.
        None => !file_header && matches!(first, Some(b' ' | b'-' | b'+' | b'\\') | None),
    }
}

/// One file's lines as its diff: the line that marks a delete or a move goes, and
/// names the file; else the `---` and `+++` lines name it.
fn section(mut lines: Vec<&str>) -> FileDiff<'_> {
    let marked = lines.first().and_then(|line| mark(line));
    if marked.is_some() {
        lines.remove(0);
    }
    let mut parts = super::changes::parts_of(lines);
    // NOTE: a move without a change has headers and no hunk.
    if parts.head.is_empty() && parts.body.iter().all(|line| super::changes::is_header(line)) {
        parts.head = std::mem::take(&mut parts.body);
    }
    let file = marked.unwrap_or_else(|| named(&parts.head));
    FileDiff { file, parts }
}

/// The file that the headers `head` name.
fn named(head: &[&str]) -> DiffFile {
    let side = |start: &str, prefix: &str| -> Option<Option<String>> {
        let line = head.iter().find_map(|line| line.strip_prefix(start))?;
        let path = line.split('\t').next().unwrap_or(line);
        if path == "/dev/null" {
            return Some(None);
        }
        Some(Some(path.strip_prefix(prefix).unwrap_or(path).to_owned()))
    };
    match (side("--- ", "a/"), side("+++ ", "b/")) {
        (Some(None), Some(Some(path))) => DiffFile::Add(path),
        (Some(Some(path)), Some(None)) => DiffFile::Delete(path),
        (Some(Some(from)), Some(Some(to))) if from != to => DiffFile::Move { from, to },
        (_, Some(Some(path))) | (Some(Some(path)), _) => DiffFile::Update(path),
        _ => DiffFile::Unknown,
    }
}

/// The rows of one file's diff, each after `bar` (muted): at most `limit` lines from
/// its first hunk on, in the `diff.*` roles with the file's syntax colours, then a
/// muted `… N more lines` for the lines left out and the lines that the daemon cut.
/// The file headers stay out; they only give the syntax colours, as `path` does for a
/// diff without them. On a terminal a line wider than the screen goes on in the next
/// row at the same column after a muted `↩`, so nothing of it is cut; elsewhere each
/// line is one row, without colour.
pub(crate) fn rows(
    parts: &DiffParts<'_>,
    path: Option<&str>,
    limit: usize,
    bar: &str,
    options: &RenderOptions,
) -> String {
    let shown = parts.body.len().min(limit);
    let hidden = parts.body.len() - shown + parts.cut;
    let mut source: Vec<String> = parts.head.iter().map(|line| (*line).to_owned()).collect();
    if let Some(path) = path.filter(|_| !parts.head.iter().any(|line| line.starts_with("--- "))) {
        source.push(format!("--- a/{path}"));
        source.push(format!("+++ b/{path}"));
    }
    let head = source.len();
    source.extend(parts.body[..shown].iter().map(|line| (*line).to_owned()));
    let mut text = source.join("\n");
    text.push('\n');
    let room = match columns(options) {
        Some((width, method)) => {
            let taken = text_width(bar, method) + text_width(super::WORD_MARK, method);
            width.saturating_sub(taken).max(MIN_DIFF_WIDTH)
        }
        None => usize::from(options.width()),
    };
    let options = options.clone().with_width(u16::try_from(room).unwrap_or(u16::MAX));
    let bar = paint(bar, Tone::Dim, &options);
    let mut out = String::new();
    for rows in diff_rows(&text, &options).into_iter().skip(head) {
        let last = rows.len().saturating_sub(1);
        for (at, row) in rows.iter().enumerate() {
            out.push_str(&bar);
            out.push_str(row);
            if at < last {
                out.push_str(&paint(super::WORD_MARK, Tone::Dim, &options));
            }
            out.push('\n');
        }
    }
    if hidden > 0 {
        out.push_str(&bar);
        out.push_str(&paint(&more_lines(hidden), Tone::Dim, &options));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
