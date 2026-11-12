//! A unified diff as one [`DiffLine`] for each line: its kind, its painted sign, its
//! painted content cut into rows, and the fill of its rows. The seam between the two
//! slices of the terminal look that draw a diff: this crate paints the lines (the
//! colours of each depth), the CLI lays them out (the indent, the sign column, the
//! cut, the `… N more lines` row).
//!
//! The first form keeps the colours of [`diff_rows`](crate::diff_rows): the sign bold
//! in its role and the diffed file's syntax colours inside, with the blended tint of
//! a truecolor theme behind the text. Only the place of the sign is new: it is apart
//! from the content, so a caller can put it in a column of its own.

use crate::code::{CodeBlock, CodeStyle, DiffKind, source_lines};
use crate::diff_rows::split;
use crate::highlight::ASSETS;
use crate::options::RenderOptions;
use crate::palette::Role;
use crate::style::{Line, Painter, expand_tabs, sanitize};
use crate::width::{WidthMethod, display_width, text_width};

/// What a line of a unified diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiffLineKind {
    /// A line that starts with `+` inside a hunk.
    Added,
    /// A line that starts with `-` inside a hunk.
    Removed,
    /// A line that starts with a space inside a hunk, or an empty line there.
    Context,
    /// A hunk header, `@@ -a,b +c,d @@` and what follows it.
    Hunk,
    /// Anything else: file headers, `diff --git`, `index`, `\ No newline at end of
    /// file`, and text outside a hunk.
    Other,
}

/// One line of a unified diff, painted.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiffLine {
    /// What the line is.
    pub kind: DiffLineKind,
    /// The sign of an added, removed or context line, painted: `+` and `-` bold in
    /// their roles, a context line's space as it is. Empty for any other kind, whose
    /// whole text is in [`rows`](Self::rows).
    pub sign: String,
    /// The rest of the line, painted and cut into rows of at most the options' width,
    /// never at a space: the rows of a line read back as the line. Each row opens and
    /// closes its own SGR state; a row that a cut ends keeps its trailing spaces. At
    /// least one row, which may be empty.
    pub rows: Vec<String>,
    /// The SGR parameters of the background of the line's rows, for a caller that
    /// fills each row to the edge with `ESC [ K` and resets the background before the
    /// newline: [`RenderOptions::fill`] of `diff.add.bg` or `diff.remove.bg`. `None`
    /// for other kinds, and where the palette sets no background or the colour mode is
    /// not truecolor.
    pub fill: Option<String>,
}

/// The lines of the unified diff `diff`, in order, painted for `options`. When the
/// output is not a terminal, each line is one plain row with its sign apart.
///
/// The calls slice lays these lines out under a file edit (2 columns, the sign, one
/// space, the content, the fill); the render slice gives them the design's colours in
/// each depth. [`diff_rows`](crate::diff_rows) stays for the callers of today.
pub fn diff_lines(diff: &str, options: &RenderOptions) -> Vec<DiffLine> {
    let lines = source_lines(diff);
    if !options.is_terminal() {
        return lines.into_iter().map(plain_line).collect();
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
            let mut spans = block.line(source);
            let kind = kind_of(block.diff_kind());
            let sign = match kind {
                DiffLineKind::Added | DiffLineKind::Removed | DiffLineKind::Context
                    if !spans.is_empty() =>
                {
                    let first = spans.remove(0);
                    let mut out = String::new();
                    painter.paint(&mut out, std::slice::from_ref(&first));
                    out.pop();
                    // NOTE: the painter drops a trailing space, which is all of the sign
                    // of a context line.
                    if out.is_empty() { first.text } else { out }
                }
                _ => String::new(),
            };
            let rows = paint_rows(&painter, &spans, width, method);
            let fill = match kind {
                DiffLineKind::Added => options.fill(Role::DiffAddBg),
                DiffLineKind::Removed => options.fill(Role::DiffRemoveBg),
                _ => None,
            };
            DiffLine { kind, sign, rows, fill }
        })
        .collect()
}

/// A line of a diff for output that is not a terminal: the sign apart, no colour.
fn plain_line(line: &str) -> DiffLine {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let text = sanitize(&expand_tabs(line)).into_owned();
    let kind = if text.starts_with("@@") {
        DiffLineKind::Hunk
    } else if text.starts_with("+++ ") || text.starts_with("--- ") {
        DiffLineKind::Other
    } else {
        match text.as_bytes().first() {
            Some(b'+') => DiffLineKind::Added,
            Some(b'-') => DiffLineKind::Removed,
            Some(b' ') => DiffLineKind::Context,
            _ => DiffLineKind::Other,
        }
    };
    let (sign, rest) = match kind {
        DiffLineKind::Added | DiffLineKind::Removed | DiffLineKind::Context => {
            let (sign, rest) = text.split_at(1);
            (sign.to_owned(), rest.to_owned())
        }
        _ => (String::new(), text),
    };
    DiffLine { kind, sign, rows: vec![rest], fill: None }
}

fn kind_of(kind: Option<DiffKind>) -> DiffLineKind {
    match kind {
        Some(DiffKind::Added) => DiffLineKind::Added,
        Some(DiffKind::Removed) => DiffLineKind::Removed,
        Some(DiffKind::Context) => DiffLineKind::Context,
        Some(DiffKind::HunkHeader) => DiffLineKind::Hunk,
        _ => DiffLineKind::Other,
    }
}

/// `spans` cut into rows of at most `width` columns and painted; a row that a cut ends
/// keeps its trailing spaces.
fn paint_rows(painter: &Painter, spans: &Line, width: usize, method: WidthMethod) -> Vec<String> {
    let rows = split(spans, width, method);
    let last = rows.len().saturating_sub(1);
    rows.iter()
        .enumerate()
        .map(|(at, row)| {
            let mut out = String::new();
            painter.paint(&mut out, row);
            out.pop();
            if at < last {
                let row_width: usize = row.iter().map(|span| text_width(&span.text, method)).sum();
                let shown = display_width(&out, method);
                out.push_str(&" ".repeat(row_width.saturating_sub(shown)));
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests;
