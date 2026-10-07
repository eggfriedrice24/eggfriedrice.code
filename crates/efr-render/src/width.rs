//! How many columns text takes on a terminal, by code point or by grapheme cluster.
//!
//! A terminal that counts by code point gives each code point its own width: a ZWJ
//! emoji such as a family takes the width of every emoji in it. Ghostty counts by
//! grapheme cluster (mode 2027, on by default): a cluster takes the width of its first
//! code point, a variation selector 16 makes a narrow emoji wide, a variation selector
//! 15 makes a wide one narrow, and a flag (two regional indicators) takes two columns.
//! The caller knows the terminal and picks the [`WidthMethod`]; when the count is
//! wrong, a live-zone redraw erases one row too many or too few.

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
    let mut out = String::with_capacity(painted.len());
    let mut chars = painted.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // A CSI sequence ends with its final byte, 0x40 to 0x7e.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // An OSC sequence ends with BEL or with ST (ESC \).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
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
