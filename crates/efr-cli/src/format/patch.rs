//! The `apply_patch` tool as the CLI shows it: the line of a call with each file of
//! its patch and their counts, a diff of several files cut into one block per file,
//! and the rows of a diff under a call ([`layout`]).
//!
//! ```text
//! ✓ patch 3 files  +5 -1
//!   src/a.rs  +1 -1
//!     @@ 1 fn main
//!     -     old();
//!     +     new();
//!   new notes.md  +2
//!     + # Notes
//!     +
//!   delete old.rs
//!     deleted; its lines are in efr diff
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

use efr_render::{DiffLineKind, Glyphs, RenderOptions, Role, WidthMethod, diff_lines, text_width};
use serde_json::Value;

use super::changes::{DiffParts, MINUS, Piece, more_lines};
use super::{Tone, columns, one_line, paint};

/// The arrow between the two paths of a move (`Glyphs::arrow`), in a call's line.
fn arrow() -> String {
    format!(" {} ", Glyphs::UNICODE.arrow)
}

/// The name of the tool.
pub(crate) const TOOL: &str = "apply_patch";

/// The member that holds the patch in the function form of the tool, the same name as
/// `efr_tools::FREEFORM_INPUT`.
const INPUT: &str = "input";

/// What joins the files of a call's line.
const JOIN: &str = ", ";

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

/// The files of a patch as the line of its call, such as `src/a.rs +3 -1, new
/// notes.md +2, delete old.rs, move a.rs → b.rs +1 -1`. Empty for a patch without a
/// file.
pub(crate) fn summary(files: &[PatchFile]) -> String {
    entries(files).join(JOIN)
}

/// Each file of a patch as its entry in the line of its call, such as `src/a.rs +3
/// -1` or `delete old.rs`, safe to print.
pub(crate) fn entries(files: &[PatchFile]) -> Vec<String> {
    files
        .iter()
        .map(|file| {
            let path = one_line(&file.path);
            let mut part = match &file.op {
                Op::Update => path,
                Op::Add => format!("new {path}"),
                Op::Delete => return format!("delete {path}"),
                Op::Move(to) => format!("move {path}{}{}", arrow(), one_line(to)),
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
                marked("move", "moved", format!("{}{}{}", one_line(from), arrow(), one_line(to)))
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

/// The lines that `diff` adds and removes, counted from its lines, outside its headers.
pub(crate) fn count(diff: &str) -> (u32, u32) {
    file_diffs(diff).iter().fold((0, 0), |(added, removed), file| {
        let (more, fewer) = count_parts(&file.parts);
        (added.saturating_add(more), removed.saturating_add(fewer))
    })
}

/// The lines that one file's diff adds and removes, counted from its lines.
pub(crate) fn count_parts(parts: &DiffParts<'_>) -> (u32, u32) {
    let mut counts = (0_u32, 0_u32);
    for line in &parts.body {
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => counts.0 = counts.0.saturating_add(1),
            Some(b'-') => counts.1 = counts.1.saturating_add(1),
            _ => {}
        }
    }
    counts
}

/// The rows of one file's diff, each after `indent`: at most `limit` lines from its
/// first hunk on, then a muted `… N more lines` for the lines left out and the lines
/// that the daemon cut, with ` · efr diff` after it when `hint`. The file headers stay
/// out; they only give the syntax colours, as `path` does for a diff without them.
///
/// - A hunk header is short, in the `diff.hunk` role: `@@ 88 impl Turn`, the first line
///   of the new side and the function that git names, without its `{`.
/// - A line of the diff is its sign (`efr_render::diff_lines` paints it), one space and
///   its text, on the fill of its kind to the end of the row where the palette gives
///   one (`ESC [ K`); a line that stays has two spaces in place of its sign.
/// - On a terminal a line wider than the screen goes on in the next row after a muted
///   `↩`, 2 columns before the code of the line, so nothing of it is cut. Elsewhere
///   each line is one row, without colour.
pub(crate) fn layout(
    parts: &DiffParts<'_>,
    path: Option<&str>,
    limit: usize,
    indent: &str,
    hint: bool,
    options: &RenderOptions,
) -> String {
    // NOTE: the hunk of a new file shows no row, and neither does a first hunk with no
    // function context of git, whose place the row of the file gives (TM-Diffs 1: the
    // diff of `CHANGELOG.md`). So it takes none of the limit.
    let quiet_first = |line: &str| new_file_hunk(line) || no_context(line);
    let quiet = usize::from(parts.body.first().is_some_and(|line| quiet_first(line)));
    let shown = parts.body.len().min(limit.saturating_add(quiet));
    let hidden = parts.body.len() - shown + parts.cut;
    let mut source: Vec<String> = parts.head.iter().map(|line| (*line).to_owned()).collect();
    if let Some(path) = path.filter(|_| !parts.head.iter().any(|line| line.starts_with("--- "))) {
        source.push(format!("--- a/{path}"));
        source.push(format!("+++ b/{path}"));
    }
    let head = source.len();
    let body = &parts.body[..shown];
    source.extend(body.iter().map(|line| (*line).to_owned()));
    let mut text = source.join("\n");
    text.push('\n');
    // NOTE: one row for each line; this function cuts the rows itself, at the column
    // that each row starts at.
    let whole = options.clone().with_width(u16::MAX);
    let glyphs = options.glyphs();
    let method = options.width_method();
    let indent_width = text_width(indent, method);
    let painted_indent = if indent.trim().is_empty() {
        indent.to_owned()
    } else {
        paint(indent, Tone::Dim, options)
    };
    let mut out = String::new();
    for (at, (line, source)) in
        diff_lines(&text, &whole).into_iter().skip(head).zip(body).enumerate()
    {
        match line.kind {
            // NOTE: the hunk of a new file starts at its first line and says nothing.
            DiffLineKind::Hunk if new_file_hunk(source) => {}
            // NOTE: a later hunk keeps its header, so two hunks never read as one.
            DiffLineKind::Hunk if at == 0 && no_context(source) => {}
            DiffLineKind::Hunk => {
                out.push_str(&painted_indent);
                out.push_str(&options.paint(Role::DiffHunk, &short_hunk(source, glyphs)));
                out.push('\n');
            }
            DiffLineKind::Added | DiffLineKind::Removed | DiffLineKind::Context => {
                // NOTE: the sign is the first column of the line, however the painter
                // joined it with the text after it.
                let whole = format!("{}{}", line.sign, line.rows.concat());
                let mut parts = split_painted(&whole, 1, usize::MAX, method).into_iter();
                let sign = parts.next().filter(|sign| !bare(sign).is_empty());
                let sign = sign.unwrap_or_else(|| " ".to_owned());
                let content = parts.next().unwrap_or_default();
                let lead = format!("{painted_indent}{sign} ");
                let column = indent_width + 2;
                let rows = match columns(options) {
                    Some((width, method)) => cut_rows(&content, column, width, method),
                    None => vec![(0, content)],
                };
                let count = rows.len();
                for (at, (start, row)) in rows.into_iter().enumerate() {
                    let mut body = if at == 0 { lead.clone() } else { " ".repeat(start) };
                    body.push_str(&row);
                    if at + 1 < count {
                        body.push_str(&paint(glyphs.word_cut, Tone::Dim, options));
                    }
                    let body = if line.fill.is_none() { body.trim_end().to_owned() } else { body };
                    push_filled(&mut out, &body, line.fill.as_deref());
                }
            }
            _ => {
                out.push_str(&painted_indent);
                out.push_str(&line.rows.concat());
                out.push('\n');
            }
        }
    }
    if hidden > 0 {
        let mut more = more_lines(hidden, glyphs);
        if hint {
            more.push_str(&format!(" {} efr diff", glyphs.separator));
        }
        out.push_str(&painted_indent);
        out.push_str(&paint(&more, Tone::Dim, options));
        out.push('\n');
    }
    out
}

/// True for the hunk header of a new file, `@@ -0,0 +1,12 @@`, which says nothing that
/// the row of the call does not.
fn new_file_hunk(line: &str) -> bool {
    line.starts_with("@@ -0,0 ")
}

/// True for a hunk header with no function context, such as `@@ -3,1 +3,1 @@`.
fn no_context(line: &str) -> bool {
    line.strip_prefix("@@ ")
        .and_then(|rest| rest.split_once(" @@"))
        .is_some_and(|(_, context)| context.trim().is_empty())
}

/// A hunk header `@@ -88,7 +88,11 @@ impl Turn {` in its short form, `@@ 88 impl Turn`:
/// the first line of the new side and git's function context without its `{`. A header
/// of another form stays as it is.
fn short_hunk(line: &str, glyphs: &Glyphs) -> String {
    let short = || -> Option<String> {
        let rest = line.strip_prefix("@@ ")?;
        let (ranges, context) = rest.split_once(" @@")?;
        let new = ranges.split(' ').find_map(|range| range.strip_prefix('+'))?;
        let start = new.split(',').next()?;
        start.parse::<u64>().ok()?;
        let context = context.trim();
        let context = context.strip_suffix('{').unwrap_or(context).trim_end();
        let mut short = format!("{} {start}", glyphs.hunk);
        if !context.is_empty() {
            short.push(' ');
            short.push_str(&one_line(context));
        }
        Some(short)
    };
    short().unwrap_or_else(|| one_line(line))
}

/// `painted`, the text of a line of a diff after its sign, as rows of a screen `width`
/// wide whose first row starts at `column`: the first row and each row that goes on
/// keep one column for the mark of the cut. A row that goes on starts 2 columns before
/// the code of the line (after its leading spaces), and never before 2 columns after
/// `column`. Gives each row with the column it starts at; the paint of each row is
/// whole (`ESC [ 0 m` at its end, the paint that runs on again at the start of the
/// next).
fn cut_rows(
    painted: &str,
    column: usize,
    width: usize,
    method: WidthMethod,
) -> Vec<(usize, String)> {
    let plain = bare(painted);
    if column + text_width(&plain, method) <= width {
        return vec![(column, painted.to_owned())];
    }
    let leading = plain.chars().take_while(|c| *c == ' ').count();
    let goes_on = (column + leading).saturating_sub(2).max(column + 2);
    let first = width.saturating_sub(column + 1).max(MIN_DIFF_WIDTH);
    let rest = width.saturating_sub(goes_on + 1).max(MIN_DIFF_WIDTH);
    split_painted(painted, first, rest, method)
        .into_iter()
        .enumerate()
        .map(|(at, row)| (if at == 0 { column } else { goes_on }, row))
        .collect()
}

/// `text` without its escape sequences.
fn bare(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The reset of SGR.
const RESET: &str = "\x1b[0m";

/// `painted` as rows of `first` columns, then of `rest`: each row starts with the SGR
/// that runs at its start and ends with a reset when any runs at its end. Only SGR and
/// other CSI sequences may be in it; they take no column.
fn split_painted(painted: &str, first: usize, rest: usize, method: WidthMethod) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    // The SGR that runs now, the sequences since the last character, and whether the
    // row has paint open at its end.
    let mut active: Vec<String> = Vec::new();
    let mut pending = String::new();
    let mut open = false;
    let mut used = 0;
    let mut room = first.max(1);
    let mut chars = painted.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            let mut sequence = String::from("\x1b");
            for c in chars.by_ref() {
                sequence.push(c);
                if ('\x40'..='\x7e').contains(&c) && c != '[' {
                    break;
                }
            }
            if sequence.ends_with('m') {
                if sequence == RESET || sequence == "\x1b[m" {
                    active.clear();
                } else {
                    active.push(sequence.clone());
                }
            }
            pending.push_str(&sequence);
            continue;
        }
        let width = text_width(c.encode_utf8(&mut [0; 4]), method);
        if used > 0 && used + width > room {
            // NOTE: the sequences since the last character belong to the next row,
            // whose start sets the paint that runs.
            if open {
                row.push_str(RESET);
            }
            rows.push(std::mem::take(&mut row));
            row.push_str(&active.concat());
            pending.clear();
            used = 0;
            room = rest.max(1);
        }
        row.push_str(&std::mem::take(&mut pending));
        open = !active.is_empty();
        row.push(c);
        used += width;
    }
    row.push_str(&pending);
    rows.push(row);
    rows
}

/// Adds `body` as one row with its newline: on `fill`, the SGR parameters of the
/// background of its kind, from its first column to the end of the row (`ESC [ K`), with
/// the background set again after each reset in it and reset before the newline, so the
/// next row starts clean.
fn push_filled(out: &mut String, body: &str, fill: Option<&str>) {
    match fill {
        Some(fill) => {
            let on = format!("\x1b[{fill}m");
            out.push_str(&on);
            out.push_str(&body.replace(RESET, &format!("{RESET}{on}")));
            out.push_str("\x1b[K");
            out.push_str(RESET);
        }
        None => out.push_str(body),
    }
    out.push('\n');
}

#[cfg(test)]
mod tests;

/// `diff` with the whole paths of its `---` and `+++` lines as the user knows them
/// ([`super::call::short_path`]): `~/.config/hypr/hyprland.conf` for the preview of a
/// file tool, which names the whole path (TM-Questions 2).
pub(crate) fn short_paths(
    diff: &str,
    cwd: Option<&std::path::Path>,
    home: Option<&std::path::Path>,
) -> String {
    let mut out = String::with_capacity(diff.len());
    for line in diff.split_inclusive('\n') {
        let header = ["--- ", "+++ "].into_iter().find(|mark| line.starts_with(mark));
        match header {
            Some(mark) => {
                let rest = &line[mark.len()..];
                let path = rest.trim_end_matches(['\n', '\r']);
                out.push_str(mark);
                out.push_str(&super::call::short_path(path, cwd, home));
                out.push_str(&rest[path.len()..]);
            }
            None => out.push_str(line),
        }
    }
    out
}
