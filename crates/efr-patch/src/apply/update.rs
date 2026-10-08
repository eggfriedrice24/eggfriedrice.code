//! The hunks of one update applied to the text of one file.

use std::path::Path;

use crate::seek::{self, Found};
use crate::text::{self, Line};
use crate::{Hunk, HunkLine, PatchError};

/// One hunk placed in the file: from line `start`, its old lines give way to its new
/// lines.
struct Edit<'h> {
    start: usize,
    lines: Vec<&'h HunkLine>,
}

/// Applies `hunks` to `text`, the content of `path`, and returns the new content.
///
/// The hunks apply from the top down: each one is searched after the one before it.
/// Context lines keep the file's own text and ending, so a hunk that matched in a
/// tolerant pass changes only its `-` and `+` lines.
pub(crate) fn update(path: &Path, text: &str, hunks: &[Hunk]) -> Result<String, PatchError> {
    let source = text::split(text);
    let file: Vec<&str> = source.lines.iter().map(|line| line.text).collect();
    let mut edits: Vec<Edit<'_>> = Vec::with_capacity(hunks.len());
    let mut cursor = 0;
    for (index, hunk) in hunks.iter().enumerate() {
        let number = index + 1;
        let anchor = anchor(path, &file, hunk, number, cursor)?;
        let (edit, next) = place(path, &file, hunk, number, anchor, cursor)?;
        edits.push(edit);
        cursor = next;
    }
    // NOTE: a hunk of only `+` lines without an anchor appends to the file and leaves
    // the cursor where it was (as in Codex), so the edits are sorted here. They never
    // overlap: every other edit starts at or after the end of the one before it.
    edits.sort_by_key(|edit| edit.start);
    let mut lines: Vec<Line<'_>> = Vec::with_capacity(source.lines.len());
    let mut next = 0;
    for edit in edits {
        lines.extend_from_slice(source.lines.get(next..edit.start).unwrap_or_default());
        next = edit.start.max(next);
        for line in edit.lines {
            match line {
                HunkLine::Context(_) => {
                    lines.extend(source.lines.get(next));
                    next += 1;
                }
                HunkLine::Remove(_) => next += 1,
                HunkLine::Add(text) => lines.push(Line { text, ending: None }),
            }
        }
    }
    lines.extend_from_slice(source.lines.get(next..).unwrap_or_default());
    Ok(text::join(&lines, source.preferred, source.final_newline))
}

/// The line of the hunk's last anchor. Each anchor is the first line after the one
/// before it (after the previous hunk for the first) that it names
/// ([`seek::find_anchor`]).
fn anchor(
    path: &Path,
    file: &[&str],
    hunk: &Hunk,
    number: usize,
    cursor: usize,
) -> Result<Option<usize>, PatchError> {
    let mut from = cursor;
    let mut found = None;
    for anchor in &hunk.anchors {
        let Some(at) = seek::find_anchor(file, anchor, from) else {
            return Err(PatchError::NoAnchor {
                path: path.to_owned(),
                hunk: number,
                anchor: anchor.clone(),
                nearest: seek::nearest(file, &[anchor.as_str()]),
            });
        };
        found = Some(at);
        from = at + 1;
    }
    Ok(found)
}

/// Where the hunk goes, and the cursor after it.
///
/// - A hunk with no old lines inserts after its anchor, or appends to the file when
///   it has none or ends the file.
/// - With an anchor, the first match from the anchor's line on wins: the anchor says
///   which one the patch means.
/// - Without an anchor, the match after the previous hunk must be the only one.
/// - When nothing matches and the last old line is empty, the hunk is tried again
///   without that line: models often write the file's final newline as an empty
///   context line. A hunk whose only old line is that empty line is not tried again.
fn place<'h>(
    path: &Path,
    file: &[&str],
    hunk: &'h Hunk,
    number: usize,
    anchor: Option<usize>,
    cursor: usize,
) -> Result<(Edit<'h>, usize), PatchError> {
    let mut lines: Vec<&HunkLine> = hunk.lines.iter().collect();
    let mut retried = false;
    loop {
        let old: Vec<&str> = lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(text) | HunkLine::Remove(text) => Some(text.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect();
        if old.is_empty() {
            return Ok(match anchor {
                Some(at) if !hunk.end_of_file => (Edit { start: at + 1, lines }, at + 1),
                _ => (Edit { start: file.len(), lines }, cursor),
            });
        }
        let found = seek::find(file, &old, anchor.unwrap_or(cursor), hunk.end_of_file);
        let start = match found {
            Found::Once(start) => start,
            Found::Many(starts) if anchor.is_some() => starts[0],
            Found::Many(starts) => {
                return Err(PatchError::Ambiguous {
                    path: path.to_owned(),
                    hunk: number,
                    lines: starts.iter().map(|start| start + 1).collect(),
                });
            }
            Found::Nowhere => {
                let last_old = lines.iter().rposition(|line| !matches!(line, HunkLine::Add(_)));
                match last_old {
                    // NOTE: a retry that leaves no old line would turn the hunk into
                    // an insertion at the end of the file, not the edit it asks for.
                    Some(at)
                        if !retried
                            && old.len() > 1
                            && matches!(
                                lines[at],
                                HunkLine::Context(text) | HunkLine::Remove(text) if text.is_empty()
                            ) =>
                    {
                        lines.remove(at);
                        retried = true;
                        continue;
                    }
                    _ => {
                        let old: Vec<&str> = hunk.old_lines();
                        return Err(PatchError::NoMatch {
                            path: path.to_owned(),
                            hunk: number,
                            nearest: seek::nearest(file, &old),
                        });
                    }
                }
            }
        };
        let next = start + old.len();
        return Ok((Edit { start, lines }, next));
    }
}
