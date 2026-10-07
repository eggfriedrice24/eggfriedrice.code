//! Tool call traces and reasoning: muted, one line each, cut to the width so a trace
//! never wraps.

use crate::options::RenderOptions;
use crate::palette::Role;
use crate::style::{Painter, Span, sanitize};
use crate::width::cut;

/// Renders one trace line (a tool call, a reasoning summary): whitespace and newlines
/// collapse to single spaces, the text is cut to the width with an ellipsis, and the
/// line is in the `muted` role. When the output is not a terminal, the collapsed text
/// is written plain.
pub fn render_trace(text: &str, options: &RenderOptions) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !options.is_terminal() {
        return format!("{collapsed}\n");
    }
    let clean = sanitize(&collapsed);
    let line = cut(&clean, options.columns(), options.width_method());
    let mut out = String::new();
    Painter::new(options.colour(), options.hyperlinks())
        .with_text(options.palette().colour(Role::Text))
        .paint(&mut out, &[Span::new(line, options.role_style(Role::Muted))]);
    out
}

#[cfg(test)]
mod tests;
