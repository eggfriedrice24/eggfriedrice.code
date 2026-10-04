//! Tool call traces and reasoning: dim, one line each, cut to the width so a trace
//! never wraps.

use unicode_width::UnicodeWidthChar as _;

use crate::options::RenderOptions;
use crate::style::{Painter, Span, Style, sanitize};

const ELLIPSIS: char = '\u{2026}';

/// Renders one trace line (a tool call, a reasoning summary): whitespace and newlines
/// collapse to single spaces, the text is cut to the width with an ellipsis, and the
/// line is dim. When the output is not a terminal, the collapsed text is written
/// plain.
pub fn render_trace(text: &str, options: &RenderOptions) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !options.is_terminal() {
        return format!("{collapsed}\n");
    }
    let clean = sanitize(&collapsed);
    let line = truncate(&clean, options.columns());
    let mut out = String::new();
    Painter::new(options.colour(), options.hyperlinks())
        .paint(&mut out, &[Span::new(line, Style::PLAIN.dim())]);
    out
}

fn truncate(text: &str, width: usize) -> String {
    let total: usize = text.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push(ELLIPSIS);
    out
}

#[cfg(test)]
mod tests;
