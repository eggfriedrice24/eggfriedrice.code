//! Keeping a tool's output within what the model should read: the head and the tail,
//! with a marker for the middle. The full output of a command stays in the PTY
//! recording, where `conversation.history` can fetch it.

use efr_shell::OutputGap;
use efr_stdx::text::omitted_marker;

/// The size a tool's output is cut to unless the tool says otherwise.
pub const DEFAULT_OUTPUT_LIMIT: usize = 32 * 1024;

/// Text cut to a limit by [`truncate_middle`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Truncated {
    /// The text, whole or as head, marker and tail.
    pub text: String,
    /// True when the middle was cut out.
    pub truncated: bool,
    /// How many bytes of the input the marker stands for.
    pub omitted: u64,
}

/// `text` when it fits in `limit` bytes; otherwise its head and its tail, about half
/// of the limit each, with a line `[... N bytes omitted ...]` between them, all
/// within `limit` bytes. Cuts fall on character boundaries, and move to a line end
/// when one is near, so no line is cut in half without need. A limit smaller than the
/// marker gives the marker alone.
pub fn truncate_middle(text: &str, limit: usize) -> Truncated {
    truncate_around(text, limit, None)
}

/// [`truncate_middle`] for a text that has a marker line already at `gap`, where the
/// shell left bytes out: the head comes from the text before that marker and the tail
/// from the text after it, and the one marker of the result counts both cuts. A gap
/// that does not point at its marker counts as no gap.
pub(crate) fn truncate_around(text: &str, limit: usize, gap: Option<OutputGap>) -> Truncated {
    if text.len() <= limit {
        return Truncated { text: text.to_owned(), truncated: false, omitted: 0 };
    }
    let Some((head, dropped, tail)) = gap.and_then(|gap| split_at(text, gap)) else {
        let (head_end, tail_start) = ends(text, text, limit, text.len() as u64);
        return joined(text, head_end, (tail_start - head_end) as u64, text, tail_start);
    };
    let whole = head.len() as u64 + dropped + tail.len() as u64;
    let (head_end, tail_start) = ends(head, tail, limit, whole);
    let omitted = (head.len() - head_end) as u64 + dropped + tail_start as u64;
    joined(head, head_end, omitted, tail, tail_start)
}

/// The text before the marker of `gap`, the bytes that the marker stands for, and the
/// text after it.
fn split_at(text: &str, gap: OutputGap) -> Option<(&str, u64, &str)> {
    let (head, rest) = text.split_at_checked(gap.at)?;
    let tail = rest.strip_prefix(omitted_marker(gap.dropped).as_str())?;
    Some((head, gap.dropped, tail))
}

/// How much to keep within `limit` bytes: the end of the prefix of `head` and the
/// start of the suffix of `tail`, about half of the room each; a side shorter than
/// its half gives the rest to the other. `whole` is the most bytes that the marker can
/// stand for, so the room keeps space for the longest marker.
fn ends(head: &str, tail: &str, limit: usize, whole: u64) -> (usize, usize) {
    let budget = limit.saturating_sub(omitted_marker(whole).len());
    let tail_budget = (budget - budget / 2).min(tail.len());
    let head_budget = (budget - tail_budget).min(head.len());
    let tail_budget = (budget - head_budget).min(tail.len());

    let mut head_end = floor_boundary(head, head_budget);
    if let Some(newline) = head[..head_end].rfind('\n')
        && newline + 1 >= head_end - head_end / 4
    {
        head_end = newline + 1;
    }
    let mut tail_start = ceil_boundary(tail, tail.len() - tail_budget);
    if let Some(newline) = tail[tail_start..].find('\n')
        && newline < tail_budget / 4
    {
        tail_start += newline + 1;
    }
    (head_end, tail_start)
}

/// `head` up to `head_end`, the marker of `omitted` bytes, and `tail` from
/// `tail_start`.
fn joined(head: &str, head_end: usize, omitted: u64, tail: &str, tail_start: usize) -> Truncated {
    let text = format!("{}{}{}", &head[..head_end], omitted_marker(omitted), &tail[tail_start..]);
    Truncated { text, truncated: true, omitted }
}

/// The largest character boundary at or below `at`.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The smallest character boundary at or above `at`.
fn ceil_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests;
