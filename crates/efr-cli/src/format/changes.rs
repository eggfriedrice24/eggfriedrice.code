//! What a call or a turn changed in the files of its roots, as the daemon's snapshots
//! see them: the muted row under a call, the line at the end of a turn and the list of
//! `efr diff --stat`. After an `auto` turn that `efr undo` can take back, the line of
//! the turn names the roots, the files outside them and the hint, then what no
//! snapshot covers; `efr history` names each undo and, with `--verbose`, each snapshot.
//!
//! ```text
//! changed src/a.rs +3 -1 · Cargo.lock +18 · deleted old.rs (+2 more)
//! 3 files changed  +24 -7  crates/app/src/turn.rs, tests/steer.rs, CHANGELOG.md
//! changed 4 files  +12 -3  in app and 2 in ~ · efr undo reverts this turn
//! not covered: target/ (ignored, large), refs of app (shallow)
//! ```
//!
//! The line of a turn names at most three files after its counts, each after the first
//! without the directories that it shares with the first, then how many more. The
//! count of the files is in `text`, the names and the rest in `muted`. `efr diff
//! --stat` starts with a table of the groups of the files ([`groups`]) when a file is
//! not code.
//!
//! Every path comes from the daemon and passes through [`one_line`]. Added lines are in
//! the `success` role and removed lines in the `error` role.

use std::fmt::Write as _;
use std::path::PathBuf;

use efr_config::DiffGroups;
use efr_protocol::{
    ChangeKind, Event, FileChange, FileChanges, RefChange, RootFiles, SnapshotPhase, TurnId,
    TurnUndoInfo,
};
use efr_render::{Glyphs, RenderOptions, WidthMethod, text_width};

use super::groups::{self, GroupCount};
use super::{Tone, one_line, paint};

/// A file outside the roots that the calls of a turn changed (`home_file_changed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HomeChange {
    pub(crate) path: PathBuf,
    pub(crate) real: Option<PathBuf>,
    /// The copy in efr's phase A backup store, in an event from before the home store.
    pub(crate) backup: Option<PathBuf>,
    pub(crate) floor: bool,
    pub(crate) covered: bool,
}

impl HomeChange {
    /// The change that `event` reports; `None` for an event that is not a
    /// `home_file_changed`.
    pub(crate) fn of(event: &Event) -> Option<HomeChange> {
        match event {
            Event::HomeFileChanged { path, real, backup, floor, covered, .. } => Some(HomeChange {
                path: path.clone(),
                real: real.clone(),
                backup: backup.clone(),
                floor: *floor,
                covered: *covered,
            }),
            _ => None,
        }
    }
}

/// Adds `change` to `changes`, which hold one entry for each path, in the order of the
/// first change: efrd reports a file once for each call that changed it, and the end
/// of a turn names it once. The backup and the real file of the last change count. A
/// file is covered only when each change says so, because undo cannot revert a change
/// that efr did not copy.
pub(crate) fn add_home_change(changes: &mut Vec<HomeChange>, change: HomeChange) {
    match changes.iter_mut().find(|known| known.path == change.path) {
        Some(known) => {
            known.covered &= change.covered;
            known.floor |= change.floor;
            known.backup = change.backup;
            if change.real.is_some() {
                known.real = change.real;
            }
        }
        None => changes.push(change),
    }
}

/// The sign of a count of removed lines: a plain hyphen-minus, as a diff writes it.
pub(crate) const MINUS: char = '-';

/// The files that a call's row names; the rest are counted.
const ROW_FILES: usize = 3;

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
fn path(file: &FileChange, glyphs: &Glyphs) -> String {
    match (&file.from, file.kind) {
        (Some(from), ChangeKind::Renamed) => {
            format!("{} {} {}", one_line(from), glyphs.arrow, one_line(&file.path))
        }
        _ => one_line(&file.path),
    }
}

/// ` · ` with the separator of `glyphs`.
fn join(glyphs: &Glyphs) -> String {
    format!(" {} ", glyphs.separator)
}

/// `+3 -1` as pieces, a part left out when it is 0; nothing when both are.
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

/// The row of changes under a call, such as `changed src/a.rs +3 -1 · Cargo.lock +18 ·
/// deleted old.rs (+2 more)`: the first files, grouped by their kind in the order the
/// kinds first come, each after ` · `, then how many more files changed. Empty when
/// nothing changed.
pub(crate) fn call_row(changes: &FileChanges, options: &RenderOptions) -> Vec<Piece> {
    let glyphs = options.glyphs();
    let join = join(glyphs);
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
            pieces.push((join.clone(), Tone::Dim));
        }
        pieces.push((format!("{} ", word(*kind)), Tone::Dim));
        for (number, file) in shown.iter().filter(|file| file.kind == *kind).enumerate() {
            if number > 0 {
                pieces.push((join.clone(), Tone::Dim));
            }
            pieces.push((path(file, glyphs), Tone::Dim));
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

/// The files that the line of a turn names; the rest are counted.
const LINE_FILES: usize = 3;

/// The count of the files and the lines, as the start of the line of a turn: `3 files
/// changed` in `text`, two spaces, `+24 -7` in colour.
fn total_pieces(count: &str, added: u32, removed: u32) -> Vec<Piece> {
    let mut pieces = vec![(count.to_owned(), Tone::Text)];
    let counts = counts(added, removed);
    if !counts.is_empty() {
        pieces.push(("  ".to_owned(), Tone::Plain));
        pieces.extend(counts);
    }
    pieces
}

/// `path` without the directories at its start that it shares with `first`:
/// `crates/app/tests/steer.rs` after `crates/app/src/turn.rs` is `tests/steer.rs`.
pub(crate) fn short_path<'a>(path: &'a str, first: &str) -> &'a str {
    let mut shared = 0;
    for ((at, a), b) in path.char_indices().zip(first.chars()) {
        if a != b {
            break;
        }
        if a == '/' {
            shared = at + 1;
        }
    }
    &path[shared..]
}

/// The names of `paths` for a line: at most `shown`, the first whole and each later one
/// without the directories it shares with the first, joined by `, `, then ` (+N more)`
/// for the rest (`more` names `N more` in the words of the line).
fn names(paths: &[String], hidden: usize, more: impl Fn(usize) -> String) -> String {
    let first = paths.first().map(String::as_str).unwrap_or_default();
    let shown: Vec<&str> = paths
        .iter()
        .take(LINE_FILES)
        .enumerate()
        .map(|(at, path)| if at == 0 { path.as_str() } else { short_path(path, first) })
        .collect();
    let rest = paths.len().saturating_sub(shown.len()) + hidden;
    let mut text = shown.join(", ");
    if rest > 0 && !shown.is_empty() {
        let _ = write!(text, " ({})", more(rest));
    }
    text
}

/// The line at the end of a turn that changed files, as pieces: `3 files changed  +24
/// -7  crates/app/src/turn.rs, tests/steer.rs, CHANGELOG.md (+1 more)`, the count in
/// `text`, the counts in colour and the names muted (TM-Legend 34); empty when it
/// changed none.
pub(crate) fn turn_line(changes: &FileChanges, options: &RenderOptions) -> Vec<Piece> {
    let count = file_count(changes);
    if count == 0 {
        return Vec::new();
    }
    let glyphs = options.glyphs();
    let mut pieces = total_pieces(&files_changed(count), changes.added, changes.removed);
    let paths: Vec<String> = changes.files.iter().map(|file| path(file, glyphs)).collect();
    let more = usize::try_from(changes.more).unwrap_or(usize::MAX);
    let named = names(&paths, more, |rest| format!("+{rest} more"));
    if !named.is_empty() {
        pieces.push(("  ".to_owned(), Tone::Plain));
        pieces.push((named, Tone::Dim));
    }
    pieces
}

/// [`turn_line`] without the names of the files: `1 file changed  +5 -3`, for
/// `efr history`, whose rows of the calls name the files above it (TM-Commands 4).
pub(crate) fn count_line(changes: &FileChanges) -> Vec<Piece> {
    let count = file_count(changes);
    if count == 0 {
        return Vec::new();
    }
    total_pieces(&files_changed(count), changes.added, changes.removed)
}

/// [`turn_line`] as plain text, for a caller that prints it as one line.
pub(crate) fn plain(pieces: &[Piece]) -> String {
    pieces.iter().map(|(text, _)| text.as_str()).collect()
}

/// The hint at the end of the line of an `auto` turn that `efr undo` can take back.
pub(crate) const UNDO_HINT: &str = "efr undo reverts this turn";

/// The most names of the `not covered:` line; the rest are counted.
const NOT_COVERED_NAMES: usize = 5;

/// `1 file` or `3 files`.
fn files(count: usize) -> String {
    if count == 1 { "1 file".to_owned() } else { format!("{count} files") }
}

/// The line at the end of an `auto` turn that `efr undo` can take back, as pieces:
/// `changed 4 files  +12 -3  in app and 2 in ~ · efr undo reverts this turn`
/// (TM-Endings 17): the count in `text`, the counts in colour, then muted the roots
/// that hold the files and the files outside the roots that efr copied first, and the
/// hint. For a turn that only moved refs: `moved 1 ref  in app · efr undo reverts this
/// turn`. Empty when the turn changed no file or ref that undo covers, or undo cannot
/// take the turn back now.
pub(crate) fn undo_line(
    changes: Option<&FileChanges>,
    undo: &TurnUndoInfo,
    options: &RenderOptions,
) -> Vec<Piece> {
    if !undo.undoable {
        return Vec::new();
    }
    let separator = options.glyphs().separator;
    let count = changes.map_or(0, file_count);
    let home = usize::try_from(undo.home_files).unwrap_or(usize::MAX);
    let roots: Vec<String> =
        undo.roots.iter().filter(|root| root.files > 0).map(|root| one_line(&root.shown)).collect();
    let (mut pieces, mut place) = match (count, home) {
        (0, 0) => {
            // NOTE: a turn that only moved refs, such as `git reset --soft HEAD~1`.
            let moved: Vec<&RootFiles> = undo.roots.iter().filter(|root| root.refs > 0).collect();
            let refs: u32 = moved.iter().map(|root| root.refs).sum();
            if refs == 0 {
                return Vec::new();
            }
            let names: Vec<String> = moved.iter().map(|root| one_line(&root.shown)).collect();
            let unit = if refs == 1 { "ref" } else { "refs" };
            (vec![(format!("moved {refs} {unit}"), Tone::Text)], format!("in {}", joined(&names)))
        }
        (0, home) => (vec![(format!("changed {}", files(home)), Tone::Text)], "in ~".to_owned()),
        (count, home) => {
            let (added, removed) =
                changes.map_or((0, 0), |changes| (changes.added, changes.removed));
            let pieces = total_pieces(&format!("changed {}", files(count)), added, removed);
            let place = match (roots.as_slice(), home) {
                ([], 0) => String::new(),
                ([], home) => format!("and {home} in ~"),
                (roots, 0) => format!("in {}", joined(roots)),
                (roots, home) => format!("in {} and {home} in ~", roots.join(", ")),
            };
            (pieces, place)
        }
    };
    if !place.is_empty() {
        place.push(' ');
    }
    let _ = write!(place, "{separator} {UNDO_HINT}");
    pieces.push(("  ".to_owned(), Tone::Plain));
    pieces.push((place, Tone::Dim));
    pieces
}

/// `a`, `a and b`, `a, b and c`.
fn joined(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The line of what `efr undo` cannot bring back, such as `not covered: target/
/// (ignored, large), refs of app (shallow)`: at most five names, then how many more.
/// `None` when nothing is left out.
pub(crate) fn not_covered_line(names: &[String], more: u32) -> Option<String> {
    if names.is_empty() && more == 0 {
        return None;
    }
    let shown: Vec<String> =
        names.iter().take(NOT_COVERED_NAMES).map(|name| one_line(name)).collect();
    let rest = names.len().saturating_sub(shown.len()) + usize::try_from(more).unwrap_or(0);
    let mut line = format!("not covered: {}", shown.join(", "));
    if rest > 0 {
        let _ = write!(line, "{}and {rest} more", if shown.is_empty() { "" } else { " " });
    }
    Some(line)
}

/// The line of an undo in `efr history`, such as `undid turn <id>: 2 files restored,
/// 1 moved to the undo trash`.
pub(crate) fn undo_applied_line(
    turn_id: TurnId,
    redo: bool,
    counts: UndoCounts,
    refs: &[RefChange],
) -> String {
    let mut parts = Vec::new();
    if counts.restored > 0 {
        parts.push(format!("{} restored", files(counts.restored)));
    }
    if counts.trashed > 0 {
        parts.push(format!("{} moved to the undo trash", counts.trashed));
    }
    if counts.deleted > 0 {
        let unit = if counts.deleted == 1 { "link" } else { "links" };
        parts.push(format!("{} {unit} removed", counts.deleted));
    }
    let moved = refs.iter().filter(|change| change.applied == Some(true)).count();
    if moved > 0 {
        let unit = if moved == 1 { "ref" } else { "refs" };
        parts.push(format!("{moved} {unit} moved"));
    }
    if counts.conflicts > 0 {
        parts.push(format!("{} skipped (changed after the turn)", counts.conflicts));
    }
    if counts.not_covered > 0 {
        parts.push(format!("{} not covered", counts.not_covered));
    }
    let verb = if redo { "redid the undo of turn" } else { "undid turn" };
    if counts.stopped {
        parts.push("stopped at a write that failed; efr undo finishes it".to_owned());
    }
    if parts.is_empty() {
        return format!("{verb} {turn_id}: nothing changed");
    }
    format!("{verb} {turn_id}: {}", parts.join(", "))
}

/// The counts of an `undo_applied` event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct UndoCounts {
    pub(crate) restored: usize,
    pub(crate) trashed: usize,
    pub(crate) deleted: usize,
    pub(crate) conflicts: usize,
    pub(crate) not_covered: usize,
    /// True when the undo stopped at a write that failed.
    pub(crate) stopped: bool,
}

/// The line of a snapshot in `efr history --verbose`, such as `snapshot before the
/// turn of app: 120 files, 3 left out, 45 ms (1a2b3c4)`.
pub(crate) fn snapshot_line(
    root: &str,
    phase: SnapshotPhase,
    commit: &str,
    files_in: u32,
    skipped: u32,
    ms: u64,
) -> String {
    let when = match phase {
        SnapshotPhase::Pre => "before",
        _ => "after",
    };
    let short: String = one_line(commit).chars().take(7).collect();
    let mut line = format!(
        "snapshot {when} the turn of {}: {}",
        one_line(root),
        files(usize::try_from(files_in).unwrap_or(usize::MAX))
    );
    if skipped > 0 {
        let _ = write!(line, ", {skipped} left out");
    }
    let _ = write!(line, ", {ms} ms ({short})");
    line
}

/// The label of the row of the files that a list leaves out.
const NOT_LISTED: &str = "not listed";

/// `efr diff --stat`: the table of the groups when a file is not code
/// ([`group_table`]), then one line per file (its kind, its path and its counts, or
/// `(binary)`), the files left out, then the totals. The paths start in one column.
pub(crate) fn stat(changes: &FileChanges, groups: &DiffGroups, options: &RenderOptions) -> String {
    let method = options.width_method();
    let glyphs = options.glyphs();
    let paths: Vec<String> = changes.files.iter().map(|file| path(file, glyphs)).collect();
    let widest = paths.iter().map(|path| text_width(path, method)).max().unwrap_or(0);
    let mut out = group_table(changes, groups, options);
    if !out.is_empty() {
        out.push('\n');
    }
    for (file, path) in changes.files.iter().zip(&paths) {
        let word = word(file.kind);
        out.push_str(&paint(word, Tone::Dim, options));
        out.push_str(&pad(word, WORD_COLUMN, method));
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
        let more = format!("{} {} more {unit}", glyphs.more, changes.more);
        let _ = writeln!(out, "{}", paint(&more, Tone::Dim, options));
    }
    let count = file_count(changes);
    if count > 0 {
        let pieces = total_pieces(&files_changed(count), changes.added, changes.removed);
        for (text, tone) in &pieces {
            out.push_str(&paint(text, *tone, options));
        }
        out.push('\n');
    }
    out
}

/// The column where the paths of `efr diff --stat` start: after the longest word of a
/// kind (`changed`, `deleted`, `renamed`) and two spaces (TM-Commands 5).
const WORD_COLUMN: usize = 9;

/// The table of the groups at the top of `efr diff --stat`: one row per group with
/// changed lines, the largest first, then the files that the list leaves out and the
/// total. A muted label, then `+N` in the `success` role and `-N` in the `error` role,
/// each column aligned; a count of 0 is left out. A row without changed lines, such as
/// a group of binary files, is left out, but the total always shows. Empty when every
/// group with changed lines is code, as the line of the turn names no group then.
pub(crate) fn group_table(
    changes: &FileChanges,
    groups: &DiffGroups,
    options: &RenderOptions,
) -> String {
    let all = groups::breakdown(&changes.files, groups);
    let mut rows: Vec<GroupCount> = all.iter().filter(|row| row.lines() > 0).cloned().collect();
    if groups::only_code(&rows) {
        return String::new();
    }
    if changes.more > 0 {
        let added = all.iter().fold(changes.added, |left, row| left.saturating_sub(row.added));
        let removed =
            all.iter().fold(changes.removed, |left, row| left.saturating_sub(row.removed));
        let files = usize::try_from(changes.more).unwrap_or(usize::MAX);
        let more = GroupCount { name: NOT_LISTED.to_owned(), files, added, removed };
        if more.lines() > 0 {
            rows.push(more);
        }
    }
    rows.push(GroupCount {
        name: "total".to_owned(),
        files: file_count(changes),
        added: changes.added,
        removed: changes.removed,
    });
    let method = options.width_method();
    let cells: Vec<(String, String, String)> = rows
        .iter()
        .map(|row| {
            let added = if row.added > 0 { format!("+{}", row.added) } else { String::new() };
            let removed =
                if row.removed > 0 { format!("{MINUS}{}", row.removed) } else { String::new() };
            (one_line(&row.name), added, removed)
        })
        .collect();
    let widest = |column: fn(&(String, String, String)) -> &String| {
        cells.iter().map(|cell| text_width(column(cell), method)).max().unwrap_or(0)
    };
    let label_width = widest(|cell| &cell.0);
    let added_width = widest(|cell| &cell.1);
    let removed_width = widest(|cell| &cell.2);
    let mut out = String::new();
    for (label, added, removed) in &cells {
        out.push_str(&paint(label, Tone::Dim, options));
        if !added.is_empty() || !removed.is_empty() {
            out.push_str(&pad(label, label_width, method));
            out.push_str("  ");
            out.push_str(&" ".repeat(added_width.saturating_sub(text_width(added, method))));
            out.push_str(&paint(added, Tone::Success, options));
        }
        if !removed.is_empty() {
            out.push_str("  ");
            out.push_str(&" ".repeat(removed_width.saturating_sub(text_width(removed, method))));
            out.push_str(&paint(removed, Tone::Failure, options));
        }
        out.push('\n');
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
    parts_of(diff.lines().collect())
}

/// The `lines` of one file's diff split into its file headers, its lines and the lines
/// that the daemon cut, as [`diff_parts`] splits a diff.
pub(crate) fn parts_of(mut lines: Vec<&str>) -> DiffParts<'_> {
    let mut cut = 0;
    if let Some(last) = lines.iter().rposition(|line| !line.is_empty())
        && let Some(count) = cut_lines(lines[last])
    {
        lines.truncate(last);
        cut = count;
    }
    let head = match lines.iter().position(|line| line.starts_with("@@")) {
        Some(at) if lines[..at].iter().all(|line| is_header(line)) => at,
        _ => 0,
    };
    let body = lines.split_off(head);
    DiffParts { head: lines, body, cut }
}

/// True for a line of a file header of a diff, such as `--- a/src/a.rs`.
pub(crate) fn is_header(line: &str) -> bool {
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
pub(crate) fn more_lines(count: usize, glyphs: &Glyphs) -> String {
    let unit = if count == 1 { "line" } else { "lines" };
    format!("{} {count} more {unit}", glyphs.more)
}

/// The spaces that bring `text` to `width` columns.
fn pad(text: &str, width: usize, method: WidthMethod) -> String {
    " ".repeat(width.saturating_sub(text_width(text, method)))
}

#[cfg(test)]
mod tests;
