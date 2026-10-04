//! Keeping a tool's output within what the model should read: the head and the tail,
//! with a marker for the middle. The full output of a command stays in the PTY
//! recording, where `conversation.history` can fetch it.

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
    pub omitted: usize,
}

/// `text` when it fits in `limit` bytes; otherwise its head and its tail, about half
/// of the limit each, with a line `[... N bytes omitted ...]` between them, all
/// within `limit` bytes. Cuts fall on character boundaries, and move to a line end
/// when one is near, so no line is cut in half without need. A limit smaller than the
/// marker gives the marker alone.
pub fn truncate_middle(text: &str, limit: usize) -> Truncated {
    if text.len() <= limit {
        return Truncated { text: text.to_owned(), truncated: false, omitted: 0 };
    }
    // The marker for the whole input is at least as long as the real one.
    let budget = limit.saturating_sub(marker(text.len()).len());
    let head_budget = budget / 2;
    let tail_budget = budget - head_budget;

    let mut head_end = floor_boundary(text, head_budget);
    if let Some(newline) = text[..head_end].rfind('\n')
        && newline + 1 >= head_end - head_end / 4
    {
        head_end = newline + 1;
    }
    let mut tail_start = ceil_boundary(text, text.len() - tail_budget);
    if let Some(newline) = text[tail_start..].find('\n')
        && newline < tail_budget / 4
    {
        tail_start += newline + 1;
    }
    let omitted = tail_start - head_end;
    let text = format!("{}{}{}", &text[..head_end], marker(omitted), &text[tail_start..]);
    Truncated { text, truncated: true, omitted }
}

fn marker(omitted: usize) -> String {
    format!("\n[... {omitted} bytes omitted ...]\n")
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
