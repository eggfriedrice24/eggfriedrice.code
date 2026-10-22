//! The highlight markers of FTS5 `highlight()` to ranges of code points (the plan's
//! DQ17).
//!
//! The preview asks `highlight()` to put [`OPEN`] (U+0002) before each match and
//! [`CLOSE`] (U+0003) after it. The text rules of efrd remove every control character
//! before a text goes into the index, so the only such characters in the output are
//! the markers. [`strip`] takes them out and returns the matched ranges as half-open
//! ranges of code points into the text without them: sorted, with no overlap, and
//! adjacent ranges merged. [`lines`] splits a text with its ranges into lines.
//!
//! A pure module: no IO, no runtime, nothing of the crate outside it (`cargo xtask
//! tidy`).

use std::ops::Range;

/// The marker before a match.
pub const OPEN: char = '\u{2}';

/// The marker after a match.
pub const CLOSE: char = '\u{3}';

/// The text without its markers, and the ranges that they marked. An open marker
/// with no close marks to the end of the text; a close with no open marks nothing; an
/// open inside a match changes nothing.
pub fn strip(marked: &str) -> (String, Vec<Range<u32>>) {
    let mut text = String::with_capacity(marked.len());
    let mut ranges: Vec<Range<u32>> = Vec::new();
    let mut at: u32 = 0;
    let mut open: Option<u32> = None;
    for c in marked.chars() {
        match c {
            OPEN => {
                open.get_or_insert(at);
            }
            CLOSE => {
                if let Some(start) = open.take() {
                    push(&mut ranges, start..at);
                }
            }
            _ => {
                text.push(c);
                at = at.saturating_add(1);
            }
        }
    }
    if let Some(start) = open {
        push(&mut ranges, start..at);
    }
    (text, ranges)
}

/// Adds `range` to sorted `ranges`: an empty one is dropped, and one that touches or
/// overlaps the last is merged into it.
fn push(ranges: &mut Vec<Range<u32>>, range: Range<u32>) {
    if range.start >= range.end {
        return;
    }
    match ranges.last_mut() {
        Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
        _ => ranges.push(range),
    }
}

/// The lines of `text` (split at `\n`) with the parts of `ranges` that fall in each,
/// counted from the start of the line. A range across a line end is cut in two.
pub fn lines(text: &str, ranges: &[Range<u32>]) -> Vec<(String, Vec<Range<u32>>)> {
    let mut out = Vec::new();
    let mut start: u32 = 0;
    for line in text.split('\n') {
        let len = u32::try_from(line.chars().count()).unwrap_or(u32::MAX);
        let end = start.saturating_add(len);
        let inside: Vec<Range<u32>> = ranges
            .iter()
            .filter(|range| range.start < end && range.end > start)
            .map(|range| range.start.max(start) - start..range.end.min(end) - start)
            .filter(|range| range.start < range.end)
            .collect();
        out.push((line.to_owned(), inside));
        start = end.saturating_add(1);
    }
    out
}

#[cfg(test)]
mod tests;
