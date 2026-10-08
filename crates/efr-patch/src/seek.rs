//! Where a run of lines occurs in a file: exact first, then with tolerant passes.

use std::collections::HashMap;

use crate::NearLine;

/// How two lines are compared, from strict to tolerant. A pass runs only when every
/// stricter pass found nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Byte for byte.
    Exact,
    /// Trailing whitespace ignored.
    TrailingSpace,
    /// Whitespace at both ends ignored.
    Space,
    /// Whitespace at both ends ignored, and Unicode quotes, dashes and spaces read as
    /// their ASCII form.
    Punctuation,
}

const PASSES: [Pass; 4] = [Pass::Exact, Pass::TrailingSpace, Pass::Space, Pass::Punctuation];

impl Pass {
    fn same(self, file: &str, patch: &str) -> bool {
        match self {
            Pass::Exact => file == patch,
            Pass::TrailingSpace => file.trim_end() == patch.trim_end(),
            Pass::Space => file.trim() == patch.trim(),
            Pass::Punctuation => file.trim().chars().map(fold).eq(patch.trim().chars().map(fold)),
        }
    }
}

/// The ASCII form of a typographic character that models rarely write but files
/// often hold: dashes, curly quotes and special spaces.
fn fold(c: char) -> char {
    match c {
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2018}'..='\u{201B}' => '\'',
        '\u{201C}'..='\u{201F}' => '"',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        other => other,
    }
}

/// The places a run of lines matches, in the first pass that finds any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    /// No pass finds the run.
    Nowhere,
    /// The first pass that finds it finds it at this index only.
    Once(usize),
    /// The first pass that finds it finds it at each of these indexes, in order.
    Many(Vec<usize>),
}

/// Where `pattern` occurs in `lines` at an index from `from` on. With `at_end`, the
/// run must end the file, so the only candidate is the last place.
///
/// The passes run from strict to tolerant, and the first pass that finds the run
/// decides: a stricter match always wins over a tolerant one, and a tolerant pass can
/// never make an exact match ambiguous.
pub(crate) fn find(lines: &[&str], pattern: &[&str], from: usize, at_end: bool) -> Found {
    let Some(last) = lines.len().checked_sub(pattern.len()) else {
        return Found::Nowhere;
    };
    if from > last {
        return Found::Nowhere;
    }
    let first = if at_end { last } else { from };
    for pass in PASSES {
        let starts: Vec<usize> = (first..=last)
            .filter(|&start| {
                pattern.iter().zip(&lines[start..]).all(|(want, have)| pass.same(have, want))
            })
            .collect();
        match starts.as_slice() {
            [] => {}
            [start] => return Found::Once(*start),
            _ => return Found::Many(starts),
        }
    }
    Found::Nowhere
}

/// The first line from `from` on that an `@@` anchor names: the first line that the
/// whole anchor matches in the first pass that finds one, else the first line that
/// starts with the anchor (whitespace at both ends ignored, punctuation read as
/// ASCII). The last pass lets `@@ fn run()` name the line `fn run() {`, and
/// `@@ class Base` the line `class Base:`, as the format's own examples do.
pub(crate) fn find_anchor(lines: &[&str], anchor: &str, from: usize) -> Option<usize> {
    match find(lines, &[anchor], from, false) {
        Found::Once(at) => return Some(at),
        Found::Many(at) => return at.first().copied(),
        Found::Nowhere => {}
    }
    let anchor: Vec<char> = anchor.trim().chars().map(fold).collect();
    if anchor.is_empty() {
        return None;
    }
    let starts = |line: &str| {
        let mut chars = line.trim().chars().map(fold);
        anchor.iter().all(|want| chars.next() == Some(*want))
    };
    lines.iter().enumerate().skip(from).find(|(_, line)| starts(line)).map(|(at, _)| at)
}

/// The most lines a near-miss shows.
const NEAR_MAX: usize = 12;

/// The longest prefix of a line that the fuzzy comparison reads, so a huge line
/// cannot make an error slow.
const FUZZY_CHARS: usize = 400;

/// The lines of `lines` nearest to `pattern`, with one line around them, for an
/// error that helps the model correct its patch.
///
/// The place where most lines of the pattern occur (each compared in the most
/// tolerant pass) wins. When no line occurs at all, the line most like the longest
/// line of the pattern (by shared pairs of characters) marks the place. When nothing
/// is alike, the result is empty.
pub(crate) fn nearest(lines: &[&str], pattern: &[&str]) -> Vec<NearLine> {
    if lines.is_empty() || pattern.iter().all(|line| line.trim().is_empty()) {
        return Vec::new();
    }
    let start = by_shared_lines(lines, pattern).or_else(|| by_alike_line(lines, pattern));
    let Some(start) = start else {
        return Vec::new();
    };
    let first = start.saturating_sub(1);
    let end = (start + pattern.len() + 1).min(lines.len()).min(first + NEAR_MAX);
    (first..end)
        .map(|index| NearLine { number: index + 1, text: lines[index].to_owned() })
        .collect()
}

fn key(line: &str) -> String {
    line.trim().chars().map(fold).collect()
}

/// The start of the window of `pattern.len()` lines that holds the most lines of the
/// pattern at their place in the pattern. Blank lines do not count: they occur
/// everywhere.
fn by_shared_lines(lines: &[&str], pattern: &[&str]) -> Option<usize> {
    let mut offsets: HashMap<String, Vec<usize>> = HashMap::new();
    for (offset, line) in pattern.iter().enumerate() {
        let key = key(line);
        if !key.is_empty() {
            offsets.entry(key).or_default().push(offset);
        }
    }
    // NOTE: a window may start before the file, so `score[start + pattern.len()]` holds
    // the window that starts at `start` (which may be below zero).
    let mut score = vec![0_u32; lines.len() + pattern.len()];
    for (index, line) in lines.iter().enumerate() {
        if let Some(at) = offsets.get(&key(line)) {
            for offset in at {
                score[index + pattern.len() - offset] += 1;
            }
        }
    }
    let (best, &most) = score.iter().enumerate().rev().max_by_key(|&(_, count)| *count)?;
    (most > 0).then(|| best.saturating_sub(pattern.len()).min(lines.len() - 1))
}

/// The start of the window around the line most like the longest line of `pattern`.
fn by_alike_line(lines: &[&str], pattern: &[&str]) -> Option<usize> {
    let (offset, longest) =
        pattern.iter().enumerate().max_by_key(|(_, line)| line.trim().chars().count())?;
    let want = pairs(longest);
    if want.is_empty() {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        let shared = shared(&want, &pairs(line));
        if shared > 0 && best.is_none_or(|(_, most)| shared > most) {
            best = Some((index, shared));
        }
    }
    best.map(|(index, _)| index.saturating_sub(offset))
}

/// The sorted pairs of neighbouring characters of a line, read as for the
/// punctuation pass and in lower case.
fn pairs(line: &str) -> Vec<(char, char)> {
    let chars: Vec<char> =
        line.trim().chars().take(FUZZY_CHARS).map(|c| fold(c).to_ascii_lowercase()).collect();
    let mut pairs: Vec<(char, char)> = chars.windows(2).map(|pair| (pair[0], pair[1])).collect();
    pairs.sort_unstable();
    pairs
}

/// How many pairs two sorted lists of pairs share, each pair counted as often as it
/// occurs in both.
fn shared(a: &[(char, char)], b: &[(char, char)]) -> usize {
    let (mut i, mut j, mut count) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                count += 1;
                i += 1;
                j += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests;
