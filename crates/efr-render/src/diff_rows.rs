//! A unified diff as rows of a fixed width, for a caller that lays a diff out itself,
//! such as the CLI under a file write: the same colours as a diff in a code block, but
//! each line cut into rows that fit, so the caller can put its own indent and marks
//! around them and no row wraps on the terminal.

use unicode_segmentation::UnicodeSegmentation as _;

use crate::code::{CodeBlock, CodeStyle, source_lines};
use crate::highlight::ASSETS;
use crate::options::RenderOptions;
use crate::palette::Role;
use crate::style::{Line, Painter, Span, expand_tabs, push_span, sanitize};
use crate::width::{WidthMethod, display_width, text_width};

/// The rows of each line of the unified diff `diff`, at most the options' width each:
/// one entry per line, in order. Added lines are in the `diff.add` role, removed lines
/// in `diff.remove`, hunk headers in `diff.hunk`, with the diffed file's syntax colours
/// inside, as [`render`](crate::render) shows a fenced `diff` block. A line wider than
/// the width is cut into rows at the width, never at a space, because a diff shows
/// code: the rows of a line read back as the line. Every row opens and closes its own
/// SGR state, and a row that a cut ends keeps its trailing spaces. When the output is
/// not a terminal, each line is one row, without colour.
pub fn diff_rows(diff: &str, options: &RenderOptions) -> Vec<Vec<String>> {
    let lines = source_lines(diff);
    if !options.is_terminal() {
        return lines
            .into_iter()
            .map(|line| {
                let line = line.strip_suffix('\r').unwrap_or(line);
                vec![sanitize(&expand_tabs(line)).into_owned()]
            })
            .collect();
    }
    let style = CodeStyle::new(options, &ASSETS);
    let mut block = CodeBlock::new("diff", &style);
    let painter = Painter::new(options.colour(), options.hyperlinks())
        .with_text(options.palette().colour(Role::Text));
    let width = options.columns().max(1);
    let method = options.width_method();
    lines
        .into_iter()
        .map(|source| {
            let rows = split(&block.line(source), width, method);
            let last = rows.len().saturating_sub(1);
            rows.iter()
                .enumerate()
                .map(|(at, row)| {
                    let mut out = String::new();
                    painter.paint(&mut out, row);
                    out.pop();
                    if at < last {
                        // NOTE: the painter drops spaces at the end of a line, which
                        // show nothing there; before a cut they are part of the line.
                        let row_width: usize =
                            row.iter().map(|span| text_width(&span.text, method)).sum();
                        let shown = display_width(&out, method);
                        out.push_str(&" ".repeat(row_width.saturating_sub(shown)));
                    }
                    out
                })
                .collect()
        })
        .collect()
}

/// `line` cut into rows of at most `width` columns as a terminal that counts by
/// `method` shows them; a row takes one piece at least.
fn split(line: &Line, width: usize, method: WidthMethod) -> Vec<Line> {
    let mut rows: Vec<Line> = vec![Vec::new()];
    let mut used = 0;
    for span in line {
        let pieces: Vec<&str> = match method {
            WidthMethod::Grapheme => span.text.graphemes(true).collect(),
            WidthMethod::CodePoint => span.text.split_inclusive(|_| true).collect(),
        };
        for piece in pieces {
            let piece_width = text_width(piece, method);
            if used > 0 && used + piece_width > width {
                rows.push(Vec::new());
                used = 0;
            }
            if let Some(row) = rows.last_mut() {
                push_span(row, Span::linked(piece, span.style, span.link.clone()));
            }
            used += piece_width;
        }
    }
    rows
}

#[cfg(test)]
mod tests;
