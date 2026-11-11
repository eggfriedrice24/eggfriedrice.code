//! How many columns text takes on a terminal, by code point or by grapheme cluster.
//!
//! A terminal that counts by code point gives each code point its own width: a ZWJ
//! emoji such as a family takes the width of every emoji in it. Ghostty counts by
//! grapheme cluster (mode 2027, on by default): a cluster takes the width of its first
//! code point, a variation selector 16 makes a narrow emoji wide, a variation selector
//! 15 makes a wide one narrow, and a flag (two regional indicators) takes two columns.
//! The caller knows the terminal and picks the [`WidthMethod`]; when the count is
//! wrong, a live-zone redraw erases one row too many or too few.

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthChar as _;

/// Variation selector 15: the text presentation of an emoji.
const TEXT_PRESENTATION: char = '\u{fe0e}';
/// Variation selector 16: the emoji presentation of a character.
const EMOJI_PRESENTATION: char = '\u{fe0f}';

/// How the terminal counts the width of text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WidthMethod {
    /// Each code point has its own width, as most terminals and tmux count. The
    /// default.
    #[default]
    CodePoint,
    /// Each grapheme cluster has one width, as Ghostty counts with mode 2027 on (its
    /// default, `grapheme-width-method = unicode`).
    Grapheme,
}

/// The columns that `text` takes on a terminal that counts by `method`. `text` must
/// hold no escape sequences; [`display_width`] skips them.
pub fn text_width(text: &str, method: WidthMethod) -> usize {
    match method {
        WidthMethod::CodePoint => text.chars().map(char_width).sum(),
        WidthMethod::Grapheme => text.graphemes(true).map(cluster_width).sum(),
    }
}

/// The columns that painted text takes on a terminal that counts by `method`, without
/// its CSI sequences (colours) and OSC sequences (hyperlinks).
pub fn display_width(painted: &str, method: WidthMethod) -> usize {
    if !painted.contains('\x1b') {
        return text_width(painted, method);
    }
    text_width(&visible(painted), method)
}

/// The text of `painted` that the terminal shows: every character outside an escape
/// sequence. A grapheme cluster can continue across a colour change, as it does on the
/// terminal, so the text is joined before it is measured.
fn visible(painted: &str) -> String {
    Visible { chars: painted.char_indices() }.map(|(_, c)| c).collect()
}

/// The characters of painted text that the terminal shows, each with its byte offset in
/// the painted text.
struct Visible<'a> {
    chars: std::str::CharIndices<'a>,
}

impl Iterator for Visible<'_> {
    type Item = (usize, char);

    fn next(&mut self) -> Option<(usize, char)> {
        loop {
            let (at, c) = self.chars.next()?;
            if c != '\x1b' {
                return Some((at, c));
            }
            match self.chars.next() {
                // A CSI sequence ends with its final byte, 0x40 to 0x7e.
                Some((_, '[')) => {
                    for (_, c) in self.chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // An OSC sequence ends with BEL or with ST (ESC \).
                Some((_, ']')) => {
                    while let Some((_, c)) = self.chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            self.chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// The rows that one painted line, without its newline, takes on a terminal `width`
/// columns wide that counts by `method`: at least one. A character that does not fit in
/// what is left of a row starts the next row, as the terminal wraps it, so a wide
/// character that would take the last column and one more leaves the last column
/// empty.
pub fn display_rows(painted: &str, width: usize, method: WidthMethod) -> usize {
    // The offsets of the rows do not count here, so the escapes go first.
    let text = if painted.contains('\x1b') {
        Cow::Owned(visible(painted))
    } else {
        Cow::Borrowed(painted)
    };
    let mut rows = 1;
    row_starts(&text, width, method, |_| rows += 1);
    rows
}

/// The last `rows` rows of one painted line, without its newline, on a terminal `width`
/// columns wide that counts by `method`: the colours and the hyperlink that are open
/// where those rows start, then the line from there. The whole line when it takes no
/// more rows, and nothing for no rows. The terminal wraps the result into the same rows
/// as the end of the whole line, because it starts where the terminal starts a row.
pub fn last_rows(painted: &str, width: usize, rows: usize, method: WidthMethod) -> Cow<'_, str> {
    if rows == 0 {
        return Cow::Borrowed("");
    }
    let mut starts = Vec::new();
    row_starts(painted, width, method, |at| starts.push(at));
    // `starts` holds the start of each row after the first.
    let Some(skip) = starts.len().checked_sub(rows) else {
        return Cow::Borrowed(painted);
    };
    let cut = starts[skip];
    let mut out = open_state(&painted[..cut]);
    out.push_str(&painted[cut..]);
    Cow::Owned(out)
}

/// Calls `start` with the byte offset of the first character of each row after the
/// first, as the terminal wraps one painted line `width` columns wide.
fn row_starts(painted: &str, width: usize, method: WidthMethod, mut start: impl FnMut(usize)) {
    let width = width.max(1);
    let mut column = 0;
    let mut place = |at: usize, columns: usize| {
        if columns == 0 {
            return;
        }
        if column > 0 && column + columns > width {
            start(at);
            column = 0;
        }
        column += columns;
    };
    if !painted.contains('\x1b') {
        match method {
            WidthMethod::CodePoint => {
                painted.char_indices().for_each(|(at, c)| place(at, char_width(c)));
            }
            WidthMethod::Grapheme => {
                painted.grapheme_indices(true).for_each(|(at, g)| place(at, cluster_width(g)));
            }
        }
        return;
    }
    match method {
        WidthMethod::CodePoint => {
            Visible { chars: painted.char_indices() }.for_each(|(at, c)| place(at, char_width(c)));
        }
        WidthMethod::Grapheme => {
            // A cluster can go on across a colour change: join the text, then map each
            // cluster back to the offset of its first character.
            let mut text = String::with_capacity(painted.len());
            let mut offsets = Vec::with_capacity(painted.len());
            for (at, c) in (Visible { chars: painted.char_indices() }) {
                offsets.extend(std::iter::repeat_n(at, c.len_utf8()));
                text.push(c);
            }
            for (at, cluster) in text.grapheme_indices(true) {
                place(offsets[at], cluster_width(cluster));
            }
        }
    }
}

/// The colours and the hyperlink that are open at the end of `painted`, as the escape
/// sequences that open them again. Colours go back to the last reset.
fn open_state(painted: &str) -> String {
    let mut colours = String::new();
    let mut link = "";
    let mut rest = painted;
    while let Some(at) = rest.find('\x1b') {
        let sequence = &rest[at..];
        let len = sequence_len(sequence);
        let whole = &sequence[..len];
        if let Some(params) = whole.strip_prefix("\x1b[").and_then(|s| s.strip_suffix('m')) {
            if params.is_empty() || params == "0" {
                colours.clear();
            } else {
                colours.push_str(whole);
            }
        } else if let Some(body) = whole.strip_prefix("\x1b]8;") {
            let body = body.trim_end_matches(['\x07', '\\']).trim_end_matches('\x1b');
            let target = body.split_once(';').map_or("", |(_, target)| target);
            link = if target.is_empty() { "" } else { whole };
        }
        rest = &sequence[len.max(1)..];
    }
    colours.push_str(link);
    colours
}

/// The length in bytes of the escape sequence at the start of `text`, as [`Visible`]
/// skips it.
fn sequence_len(text: &str) -> usize {
    let mut chars = text.char_indices().skip(1);
    match chars.next() {
        Some((_, '[')) => {
            for (at, c) in chars {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    return at + c.len_utf8();
                }
            }
        }
        Some((_, ']')) => {
            while let Some((at, c)) = chars.next() {
                if c == '\x07' {
                    return at + 1;
                }
                if c == '\x1b' {
                    return chars.next().map_or(at + 1, |(at, c)| at + c.len_utf8());
                }
            }
        }
        Some((at, c)) => return at + c.len_utf8(),
        None => {}
    }
    text.len()
}

/// `text` cut to at most `width` columns with an ellipsis at the end, at a code point
/// or a grapheme cluster as the terminal counts; `text` itself when it fits.
pub(crate) fn cut(text: &str, width: usize, method: WidthMethod) -> String {
    const ELLIPSIS: char = '\u{2026}';
    if text_width(text, method) <= width {
        return text.to_owned();
    }
    let pieces: Vec<&str> = match method {
        WidthMethod::CodePoint => text.split_inclusive(|_| true).collect(),
        WidthMethod::Grapheme => text.graphemes(true).collect(),
    };
    let mut out = String::new();
    let mut used = 0;
    for piece in pieces {
        let w = text_width(piece, method);
        if used + w + 1 > width {
            break;
        }
        out.push_str(piece);
        used += w;
    }
    out.push(ELLIPSIS);
    out
}

/// The width of one code point; control characters take none.
pub(crate) fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// The width of one grapheme cluster as Ghostty counts it.
fn cluster_width(cluster: &str) -> usize {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else { return 0 };
    if is_regional_indicator(first) {
        return 2;
    }
    let base = char_width(first);
    if base == 1 && cluster.contains(EMOJI_PRESENTATION) {
        return 2;
    }
    if base == 2 && cluster.contains(TEXT_PRESENTATION) {
        return 1;
    }
    base
}

fn is_regional_indicator(c: char) -> bool {
    ('\u{1f1e6}'..='\u{1f1ff}').contains(&c)
}

#[cfg(test)]
mod tests;
