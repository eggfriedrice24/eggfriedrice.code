//! A bounded unified diff of a text file, so the user sees what a write changes before
//! approving it.
//!
//! Lines are compared whole, with their line ends. The common head and tail are cut
//! off first; the lines between them get a longest-common-subsequence diff when the
//! table fits in [`MAX_CELLS`], and otherwise show as one replacement, which is still
//! a correct diff. Hunks carry [`CONTEXT`] lines of context, and the preview stops
//! after [`MAX_LINES`] lines or [`MAX_BYTES`] bytes with a line that says how much is
//! left out.

use std::fmt::{self, Write as _};
use std::path::Path;

/// Lines of context around each change.
const CONTEXT: usize = 3;

/// The most lines a preview shows below its two header lines.
pub(crate) const MAX_LINES: usize = 200;

/// The most bytes of lines a preview shows below its header lines.
const MAX_BYTES: usize = 16 * 1024;

/// The largest table the line diff builds (old lines times new lines, between the
/// common head and tail); about 4 MiB of `u32`.
const MAX_CELLS: usize = 1 << 20;

/// What the preview says when the content would not change.
pub(crate) const UNCHANGED: &str = "the new content is the same as the current file";

/// What one line of the diff does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Keep,
    Delete,
    Insert,
}

/// One line of the diff with the number of old and new lines before it.
#[derive(Debug, Clone, Copy)]
struct Step<'a> {
    op: Op,
    text: &'a str,
    old_at: usize,
    new_at: usize,
}

/// The unified diff from `old` (`None` for a file that does not exist yet) to `new`
/// for the file at `path`, as an approval shows it: three lines of context around each
/// change, at most 200 lines or 16 KiB below the two header lines, then a line that
/// says how much is left out. The same text as `write_file`'s preview.
pub fn unified_diff(path: &Path, old: Option<&str>, new: &str) -> String {
    if old == Some(new) {
        return UNCHANGED.to_owned();
    }
    let lines = Lines::new(old, new);
    let mut out = header(path, old.is_some());
    let mut bytes = 0;
    for (shown, line) in lines.lines.iter().enumerate() {
        bytes += line.len() + 1;
        if shown == MAX_LINES || bytes > MAX_BYTES {
            let _ = writeln!(out, "[... {} more lines of the diff]", lines.lines.len() - shown);
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// What a write changed in a text file: its unified diff after the write and the
/// number of lines it added and removed.
///
/// `Debug` shows the sizes, not the text: a file can hold secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct WrittenDiff {
    /// The unified diff with its two header lines, at most the asked number of lines
    /// and [`MAX_WRITTEN_BYTES`] bytes below them, then a line `... N more lines`.
    pub text: String,
    /// Lines added.
    pub added: usize,
    /// Lines removed.
    pub removed: usize,
}

impl fmt::Debug for WrittenDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrittenDiff")
            .field("text_bytes", &self.text.len())
            .field("added", &self.added)
            .field("removed", &self.removed)
            .finish()
    }
}

/// The most bytes of lines that a [`WrittenDiff`] holds below its header lines, so an
/// event that carries it stays small.
pub const MAX_WRITTEN_BYTES: usize = 256 * 1024;

/// The diff of a write from `old` (`None` for a new file) to `new` for the file at
/// `path`, with three lines of context, cut after `max_lines` lines or
/// [`MAX_WRITTEN_BYTES`] bytes with a last line `... N more lines`, and the counts of
/// all lines added and removed. `None` when the content does not change.
pub fn written_diff(
    path: &Path,
    old: Option<&str>,
    new: &str,
    max_lines: usize,
) -> Option<WrittenDiff> {
    if old == Some(new) {
        return None;
    }
    change_diff(old.map(|old| (path, old)), Some((path, new)), max_lines).map(|(diff, _)| diff)
}

/// The diff of one file of a call that may also delete or move it: from `old`, its
/// path and text before the call (`None` for a new file), to `new`, its path and text
/// after it (`None` for a deleted file). The header names both paths, `/dev/null` for
/// the side that is missing. The lines are cut as [`written_diff`] cuts them. The
/// second value is the number of diff lines shown below the header. `None` when the
/// path and the text stay the same, or when both sides are missing.
pub(crate) fn change_diff(
    old: Option<(&Path, &str)>,
    new: Option<(&Path, &str)>,
    max_lines: usize,
) -> Option<(WrittenDiff, usize)> {
    if old.is_none() && new.is_none() || old == new {
        return None;
    }
    let from =
        old.map_or_else(|| "/dev/null".to_owned(), |(path, _)| format!("a{}", path.display()));
    let to = new.map_or_else(|| "/dev/null".to_owned(), |(path, _)| format!("b{}", path.display()));
    let lines = Lines::new(old.map(|(_, text)| text), new.map_or("", |(_, text)| text));
    let mut text = format!("--- {from}\n+++ {to}\n");
    let mut bytes = 0;
    let mut shown = lines.lines.len();
    for (at, line) in lines.lines.iter().enumerate() {
        bytes += line.len() + 1;
        if at == max_lines || bytes > MAX_WRITTEN_BYTES {
            let _ = writeln!(text, "... {} more lines", lines.lines.len() - at);
            shown = at;
            break;
        }
        text.push_str(line);
        text.push('\n');
    }
    Some((WrittenDiff { text, added: lines.added, removed: lines.removed }, shown))
}

/// The two header lines of a diff of `path`.
fn header(path: &Path, existed: bool) -> String {
    let from = if existed { format!("a{}", path.display()) } else { "/dev/null".to_owned() };
    format!("--- {from}\n+++ b{}\n", path.display())
}

/// The lines of a diff below its header, with the counts of added and removed lines.
struct Lines {
    lines: Vec<String>,
    added: usize,
    removed: usize,
}

impl Lines {
    fn new(old: Option<&str>, new: &str) -> Lines {
        let old_lines: Vec<&str> =
            old.map(|old| old.split_inclusive('\n').collect()).unwrap_or_default();
        let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
        let steps = steps(&old_lines, &new_lines);
        let added = steps.iter().filter(|step| step.op == Op::Insert).count();
        let removed = steps.iter().filter(|step| step.op == Op::Delete).count();

        let mut lines = Vec::new();
        for (start, end) in hunks(&steps) {
            let hunk = &steps[start..end];
            let old_count = hunk.iter().filter(|step| step.op != Op::Insert).count();
            let new_count = hunk.iter().filter(|step| step.op != Op::Delete).count();
            let first = hunk.first().map_or((0, 0), |step| (step.old_at, step.new_at));
            let old_start = if old_count == 0 { first.0 } else { first.0 + 1 };
            let new_start = if new_count == 0 { first.1 } else { first.1 + 1 };
            lines.push(format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"));
            for step in hunk {
                let mark = match step.op {
                    Op::Keep => ' ',
                    Op::Delete => '-',
                    Op::Insert => '+',
                };
                match step.text.strip_suffix('\n') {
                    Some(text) => lines.push(format!("{mark}{text}")),
                    None => {
                        lines.push(format!("{mark}{}", step.text));
                        lines.push("\\ No newline at end of file".to_owned());
                    }
                }
            }
        }
        Lines { lines, added, removed }
    }
}

/// Every line of `old` and `new` as kept, deleted or inserted, in order.
fn steps<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Step<'a>> {
    let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let tail =
        old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|(a, b)| a == b).count();
    let old_middle = &old[head..old.len() - tail];
    let new_middle = &new[head..new.len() - tail];

    let mut ops = vec![Op::Keep; head];
    ops.extend(middle(old_middle, new_middle));
    ops.extend(std::iter::repeat_n(Op::Keep, tail));

    let (mut old_at, mut new_at) = (0, 0);
    let mut steps = Vec::with_capacity(ops.len());
    for op in ops {
        let text = match op {
            Op::Keep | Op::Delete => old.get(old_at),
            Op::Insert => new.get(new_at),
        };
        steps.push(Step { op, text: text.copied().unwrap_or_default(), old_at, new_at });
        match op {
            Op::Keep => {
                old_at += 1;
                new_at += 1;
            }
            Op::Delete => old_at += 1,
            Op::Insert => new_at += 1,
        }
    }
    steps
}

/// The edit of the lines between the common head and tail: a longest common
/// subsequence when its table is small enough, otherwise every old line deleted and
/// every new line inserted.
fn middle(old: &[&str], new: &[&str]) -> Vec<Op> {
    let (n, m) = (old.len(), new.len());
    let cells = (n + 1).saturating_mul(m + 1);
    if n == 0 || m == 0 || cells > MAX_CELLS {
        let mut ops = vec![Op::Delete; n];
        ops.extend(std::iter::repeat_n(Op::Insert, m));
        return ops;
    }
    // lcs[i * (m + 1) + j] is the length of the longest common subsequence of
    // old[i..] and new[j..].
    let width = m + 1;
    let mut lcs = vec![0_u32; cells];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if old[i] == new[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::with_capacity(n + m);
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push(Op::Keep);
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            ops.push(Op::Delete);
            i += 1;
        } else {
            ops.push(Op::Insert);
            j += 1;
        }
    }
    ops.extend(std::iter::repeat_n(Op::Delete, n - i));
    ops.extend(std::iter::repeat_n(Op::Insert, m - j));
    ops
}

/// The hunks of `steps` as ranges: each change with [`CONTEXT`] kept lines around it,
/// and changes closer than twice that in one hunk.
fn hunks(steps: &[Step<'_>]) -> Vec<(usize, usize)> {
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for (at, step) in steps.iter().enumerate() {
        if step.op == Op::Keep {
            continue;
        }
        let start = at.saturating_sub(CONTEXT);
        let end = (at + CONTEXT + 1).min(steps.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => hunks.push((start, end)),
        }
    }
    hunks
}

#[cfg(test)]
mod tests;
