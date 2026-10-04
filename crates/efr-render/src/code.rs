//! Code blocks: a dim language label, then one output line per source line with
//! syntax colours, never wrapped. A block renders one line at a time, so the
//! streaming renderer commits each line as soon as it is complete, and a block
//! streamed in pieces renders exactly as the same block rendered at once.

mod diff;

use crate::highlight::{Assets, Highlight};
use crate::options::{ColourMode, Theme};
use crate::style::{Line, Span, Style, expand_tabs, push_span, sanitize};

use diff::Diff;

/// Fence labels that mark a unified diff.
const DIFF_LABELS: &[&str] = &["diff", "patch", "udiff"];

/// What a code block needs from the options.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CodeStyle {
    pub(crate) colour: ColourMode,
    pub(crate) theme: Theme,
    pub(crate) assets: &'static Assets,
}

#[derive(Clone, Debug)]
pub(crate) struct CodeBlock {
    label: Option<String>,
    body: Body,
    started: bool,
}

#[derive(Clone, Debug)]
enum Body {
    Plain,
    /// No label: the first line decides whether this is a diff.
    Sniff(CodeStyle),
    Highlighted(Highlight),
    Diff(Box<Diff>),
}

impl CodeBlock {
    /// A block for a fence's info string (empty for an indented block). Grammars load
    /// only here, and only for a block with a known label in a colour mode.
    pub(crate) fn new(info: &str, style: CodeStyle) -> CodeBlock {
        let label = info
            .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
            .next()
            .filter(|label| !label.is_empty())
            .map(|label| sanitize(label).into_owned());
        let body = match label.as_deref() {
            Some(label) if DIFF_LABELS.contains(&label.to_ascii_lowercase().as_str()) => {
                Body::Diff(Box::new(Diff::new(style)))
            }
            _ if style.colour == ColourMode::None => Body::Plain,
            Some(label) => style.assets.syntax_for_label(label).map_or(Body::Plain, |syntax| {
                Body::Highlighted(Highlight::new(style.assets, syntax, style.theme))
            }),
            None => Body::Sniff(style),
        };
        CodeBlock { label, body, started: false }
    }

    /// The label line, the first time it is asked for; `None` afterwards, and for a
    /// block without a label.
    pub(crate) fn start(&mut self) -> Option<Line> {
        if self.started {
            return None;
        }
        self.started = true;
        self.label.as_ref().map(|label| vec![Span::new(label.clone(), Style::PLAIN.dim())])
    }

    /// Renders the next source line, given without its newline.
    pub(crate) fn line(&mut self, source: &str) -> Line {
        let source = source.strip_suffix('\r').unwrap_or(source);
        let expanded = expand_tabs(source);
        let text = sanitize(&expanded);
        if let Body::Sniff(style) = self.body {
            self.body = if text.starts_with("diff --git ") {
                Body::Diff(Box::new(Diff::new(style)))
            } else {
                Body::Plain
            };
        }
        match &mut self.body {
            Body::Plain | Body::Sniff(_) => vec![Span::plain(text)],
            Body::Diff(diff) => diff.line(&text),
            Body::Highlighted(highlight) => match highlight.line(&text) {
                Some(tokens) => {
                    let mut line = Vec::new();
                    for (style, piece) in tokens {
                        push_span(&mut line, Span::new(piece, style));
                    }
                    line
                }
                None => {
                    self.body = Body::Plain;
                    vec![Span::plain(text)]
                }
            },
        }
    }
}

/// The lines of a code block's text, without their newlines. A final line without a
/// newline (an unclosed fence at the end of a reply) is still a line.
pub(crate) fn source_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests;
