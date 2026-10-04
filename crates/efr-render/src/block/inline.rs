//! The inline text of one paragraph, heading or table cell: styled spans, broken into
//! lines at soft and hard breaks in a paragraph (the author's line structure stays;
//! the terminal soft-wraps long lines), with bare URLs made into links.

use crate::link::{encode, find_urls};
use crate::style::{Line, Span, Style, line_text, push_span, sanitize};

use super::link_style;

pub(crate) enum InlineKind {
    Paragraph,
    /// A heading, with the style of its level.
    Heading(Style),
    Cell,
}

/// Adjacent text events in one style, joined before URLs are looked for, because the
/// parser splits text at characters that might have been markup.
struct Run {
    text: String,
    style: Style,
    link: Option<String>,
    autolink: bool,
}

pub(crate) struct Inline {
    kind: InlineKind,
    lines: Vec<Line>,
    line: Line,
    run: Option<Run>,
}

impl Inline {
    pub(crate) fn new(kind: InlineKind) -> Inline {
        Inline { kind, lines: Vec::new(), line: Vec::new(), run: None }
    }

    pub(crate) fn base_style(&self) -> Style {
        match self.kind {
            InlineKind::Heading(style) => style,
            InlineKind::Paragraph | InlineKind::Cell => Style::PLAIN,
        }
    }

    /// Whether a soft break starts a new output line here. Headings and table cells
    /// are one line, so their soft breaks become spaces.
    pub(crate) fn breaks_lines(&self) -> bool {
        matches!(self.kind, InlineKind::Paragraph)
    }

    /// Adds text; `autolink` marks text outside a link, where bare URLs become links.
    pub(crate) fn push_text(
        &mut self,
        text: &str,
        style: Style,
        link: Option<String>,
        autolink: bool,
    ) {
        if let Some(run) = &mut self.run
            && run.style == style
            && run.link == link
            && run.autolink == autolink
        {
            run.text.push_str(text);
            return;
        }
        self.flush_run();
        self.run = Some(Run { text: text.to_owned(), style, link, autolink });
    }

    pub(crate) fn push_span(&mut self, span: Span) {
        self.flush_run();
        push_span(&mut self.line, span);
    }

    pub(crate) fn break_line(&mut self) {
        self.flush_run();
        self.lines.push(std::mem::take(&mut self.line));
    }

    /// Where the next span will go, as (line, span).
    pub(crate) fn position(&mut self) -> (usize, usize) {
        self.flush_run();
        (self.lines.len(), self.line.len())
    }

    /// The text written since `start`, if it is all on the current line.
    pub(crate) fn text_since(&mut self, start: (usize, usize)) -> Option<String> {
        self.flush_run();
        (self.lines.len() == start.0).then(|| line_text(self.line.get(start.1..).unwrap_or(&[])))
    }

    pub(crate) fn finish(mut self) -> Vec<Line> {
        self.flush_run();
        self.lines.push(self.line);
        self.lines
    }

    fn flush_run(&mut self) {
        let Some(run) = self.run.take() else { return };
        let text = sanitize(&run.text);
        if !run.autolink {
            push_span(&mut self.line, Span::linked(text, run.style, run.link));
            return;
        }
        let mut at = 0;
        for url in find_urls(&text) {
            push_span(&mut self.line, Span::new(&text[at..url.start], run.style));
            let target = Some(encode(&text[url.clone()]));
            let style = run.style.patch(link_style());
            push_span(&mut self.line, Span::linked(&text[url.clone()], style, target));
            at = url.end;
        }
        push_span(&mut self.line, Span::new(&text[at..], run.style));
    }
}

#[cfg(test)]
mod tests;
