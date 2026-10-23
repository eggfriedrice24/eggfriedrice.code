//! A bounded unified diff of a text file, so the user sees what a write changes before
//! approving it.
//!
//! Lines are compared whole, with their line ends. The common head and tail are cut
//! off first. The lines between them split at anchors: the lines that occur once on
//! each side, in the longest run that keeps their order on both sides (a patience
//! diff). Each gap between two anchors splits again in the same way. A gap without
//! anchors gets a longest-common-subsequence diff when its table fits in what is left
//! of [`MAX_WORK`], and otherwise shows as one replacement, which is still a correct
//! diff. All gaps share the budget, so the work stays linear in the size of the file:
//! the diff runs on the async executor. Hunks carry [`CONTEXT`] lines of context, and
//! the preview stops after [`MAX_LINES`] lines or [`MAX_BYTES`] bytes with a line that
//! says how much is left out. Only the lines that show become strings.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::{self, Write as _};
use std::ops::Range;
use std::path::Path;

/// Lines of context around each change.
const CONTEXT: usize = 3;

/// The most lines a preview shows below its two header lines.
pub(crate) const MAX_LINES: usize = 200;

/// The most bytes of lines a preview shows below its header lines.
const MAX_BYTES: usize = 16 * 1024;

/// The work that one line diff may do between the common head and tail: the cells of
/// all its tables (old lines times new lines of a gap) and the lines it scans for
/// anchors, together. The largest table is about 4 MiB of `u32`.
const MAX_WORK: usize = 1 << 20;

/// The line that follows a last line without a line end.
const NO_NEWLINE: &str = "\\ No newline at end of file";

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
    for (shown, line) in lines.iter().enumerate() {
        bytes += line.len() + 1;
        if shown == MAX_LINES || bytes > MAX_BYTES {
            let _ = writeln!(out, "[... {} more lines of the diff]", lines.len() - shown);
            break;
        }
        out.push_str(&line);
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
    let total = lines.len();
    let mut shown = total;
    for (at, line) in lines.iter().enumerate() {
        bytes += line.len() + 1;
        if at == max_lines || bytes > MAX_WRITTEN_BYTES {
            let _ = writeln!(text, "... {} more lines", total - at);
            shown = at;
            break;
        }
        text.push_str(&line);
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
/// It makes the text of a line only when [`Lines::iter`] reaches it.
struct Lines<'a> {
    steps: Vec<Step<'a>>,
    hunks: Vec<(usize, usize)>,
    added: usize,
    removed: usize,
}

impl<'a> Lines<'a> {
    fn new(old: Option<&'a str>, new: &'a str) -> Lines<'a> {
        Lines::within(old, new, MAX_WORK)
    }

    /// The diff from `old` to `new` that does at most `budget` work between the common
    /// head and tail (see [`MAX_WORK`]).
    fn within(old: Option<&'a str>, new: &'a str, budget: usize) -> Lines<'a> {
        let old_lines: Vec<&str> =
            old.map(|old| old.split_inclusive('\n').collect()).unwrap_or_default();
        let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
        let steps = steps(&old_lines, &new_lines, budget);
        let added = steps.iter().filter(|step| step.op == Op::Insert).count();
        let removed = steps.iter().filter(|step| step.op == Op::Delete).count();
        let hunks = hunks(&steps);
        Lines { steps, hunks, added, removed }
    }

    /// The number of lines: a header for each hunk, each of its steps, and the note
    /// after a last line without a line end.
    fn len(&self) -> usize {
        self.hunks
            .iter()
            .map(|&(start, end)| {
                let hunk = &self.steps[start..end];
                1 + hunk.len() + hunk.iter().filter(|step| !step.text.ends_with('\n')).count()
            })
            .sum()
    }

    /// The lines in order. Each one becomes a string when the iterator reaches it.
    fn iter(&self) -> impl Iterator<Item = String> + '_ {
        self.hunks.iter().flat_map(|&(start, end)| {
            let hunk = &self.steps[start..end];
            std::iter::once(hunk_header(hunk)).chain(hunk.iter().flat_map(|step| {
                let mark = match step.op {
                    Op::Keep => ' ',
                    Op::Delete => '-',
                    Op::Insert => '+',
                };
                let (text, note) = match step.text.strip_suffix('\n') {
                    Some(text) => (text, None),
                    None => (step.text, Some(NO_NEWLINE.to_owned())),
                };
                std::iter::once(format!("{mark}{text}")).chain(note)
            }))
        })
    }
}

/// The `@@ -a,b +c,d @@` line of `hunk`.
fn hunk_header(hunk: &[Step<'_>]) -> String {
    let old_count = hunk.iter().filter(|step| step.op != Op::Insert).count();
    let new_count = hunk.iter().filter(|step| step.op != Op::Delete).count();
    let first = hunk.first().map_or((0, 0), |step| (step.old_at, step.new_at));
    let old_start = if old_count == 0 { first.0 } else { first.0 + 1 };
    let new_start = if new_count == 0 { first.1 } else { first.1 + 1 };
    format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@")
}

/// Every line of `old` and `new` as kept, deleted or inserted, in order, with at most
/// `budget` work (see [`MAX_WORK`]).
fn steps<'a>(old: &[&'a str], new: &[&'a str], budget: usize) -> Vec<Step<'a>> {
    let ops = ops(old, new, budget);
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

/// A part of a diff that is still to do.
enum Task {
    /// Diff these old lines against these new lines.
    Gap { old: Range<usize>, new: Range<usize> },
    /// Keep this many lines: the common tail of a gap, or an anchor.
    Keep(usize),
}

/// The edit from `old` to `new`. It splits the lines at anchors again and again
/// ([`gap`]). A stack of tasks, the next one on top, holds the parts that are left,
/// not the call stack: a gap can split once for each of its lines.
fn ops(old: &[&str], new: &[&str], mut budget: usize) -> Vec<Op> {
    let mut ops = Vec::with_capacity(old.len() + new.len());
    let mut todo = vec![Task::Gap { old: 0..old.len(), new: 0..new.len() }];
    while let Some(task) = todo.pop() {
        match task {
            Task::Keep(count) => ops.extend(std::iter::repeat_n(Op::Keep, count)),
            Task::Gap { old: old_range, new: new_range } => {
                gap(old, new, old_range, new_range, &mut budget, &mut ops, &mut todo);
            }
        }
    }
    ops
}

/// Diffs `old[old_range]` against `new[new_range]`. It keeps their common head, puts
/// their common tail on `todo`, and puts the lines between on `todo` too, split at
/// their anchors. Without anchors, or when `budget` is spent, it diffs those lines in
/// one table.
fn gap(
    old: &[&str],
    new: &[&str],
    old_range: Range<usize>,
    new_range: Range<usize>,
    budget: &mut usize,
    ops: &mut Vec<Op>,
    todo: &mut Vec<Task>,
) {
    let (old, new) = (&old[old_range.clone()], &new[new_range.clone()]);
    let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let tail =
        old[head..].iter().rev().zip(new[head..].iter().rev()).take_while(|(a, b)| a == b).count();
    ops.extend(std::iter::repeat_n(Op::Keep, head));
    todo.push(Task::Keep(tail));
    let (old_start, new_start) = (old_range.start + head, new_range.start + head);
    let old = &old[head..old.len() - tail];
    let new = &new[head..new.len() - tail];
    if !old.is_empty() && !new.is_empty() && *budget > 0 {
        // NOTE: a line is scanned again at each depth that its gap splits, so the scans
        // spend the budget too, and the diff stays linear.
        *budget = budget.saturating_sub(old.len() + new.len());
        let anchors = anchors(old, new);
        if !anchors.is_empty() {
            let mut end = (old.len(), new.len());
            for &(at_old, at_new) in anchors.iter().rev() {
                todo.push(Task::Gap {
                    old: old_start + at_old + 1..old_start + end.0,
                    new: new_start + at_new + 1..new_start + end.1,
                });
                todo.push(Task::Keep(1));
                end = (at_old, at_new);
            }
            todo.push(Task::Gap {
                old: old_start..old_start + end.0,
                new: new_start..new_start + end.1,
            });
            return;
        }
    }
    table(old, new, budget, ops);
}

/// The positions in `old` and `new` of the lines that occur once on each side, in the
/// longest run whose positions grow on both sides.
fn anchors(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    // The count (at most 2) and the position of each line on the old side, then the
    // same on the new side.
    let mut seen: HashMap<&str, (u8, usize, u8, usize)> = HashMap::with_capacity(old.len());
    for (at, line) in old.iter().enumerate() {
        match seen.entry(line) {
            Entry::Occupied(mut entry) => entry.get_mut().0 = 2,
            Entry::Vacant(entry) => {
                entry.insert((1, at, 0, 0));
            }
        }
    }
    for (at, line) in new.iter().enumerate() {
        if let Some(entry) = seen.get_mut(line) {
            entry.2 = (entry.2 + 1).min(2);
            entry.3 = at;
        }
    }
    let mut pairs: Vec<(usize, usize)> = seen
        .into_values()
        .filter(|&(old_count, _, new_count, _)| old_count == 1 && new_count == 1)
        .map(|(_, at_old, _, at_new)| (at_old, at_new))
        .collect();
    pairs.sort_unstable();
    longest_increasing(&pairs)
}

/// The longest run of `pairs` (sorted by their first value) whose second values grow
/// too, by patience sorting.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // tails[k] is the pair that ends a run of length k + 1 with the smallest second
    // value so far; before[i] is the pair before pair i in its run.
    let mut tails: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = Vec::with_capacity(pairs.len());
    for (at, &(_, second)) in pairs.iter().enumerate() {
        let length = tails.partition_point(|&tail| pairs[tail].1 < second);
        before.push(length.checked_sub(1).map(|shorter| tails[shorter]));
        if length == tails.len() {
            tails.push(at);
        } else {
            tails[length] = at;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut next = tails.last().copied();
    while let Some(at) = next {
        run.push(pairs[at]);
        next = before[at];
    }
    run.reverse();
    run
}

/// The edit from `old` to `new` as a longest common subsequence when its table fits in
/// `budget`, which it then spends; otherwise every old line deleted and every new line
/// inserted.
fn table(old: &[&str], new: &[&str], budget: &mut usize, ops: &mut Vec<Op>) {
    let (n, m) = (old.len(), new.len());
    let cells = (n + 1).saturating_mul(m + 1);
    if n == 0 || m == 0 || cells > *budget {
        ops.extend(std::iter::repeat_n(Op::Delete, n));
        ops.extend(std::iter::repeat_n(Op::Insert, m));
        return;
    }
    *budget -= cells;
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
