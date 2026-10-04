//! The block layer: turns the events of a final slice of markdown into painted lines.
//! Containers (quotes and list items) give every line its prefix; paragraphs and
//! headings inside a container are wrapped so their continuation lines keep the
//! indent, while top-level prose and code are never wrapped.
//!
//! A slice is rendered on its own, with a fresh parse. What links one slice to the
//! next is [`Flow`]: whether a blank line is owed before the next block, and how the
//! slice continues a block whose first part was committed earlier.

mod inline;

use std::mem;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::code::{CodeBlock, CodeStyle, source_lines};
use crate::highlight::Assets;
use crate::link::{code_target, link_target};
use crate::options::{ColourMode, MIN_TEXT_WIDTH, RenderOptions};
use crate::outline::parser_options;
use crate::style::{
    BLUE, CYAN, GREEN, Line, MAGENTA, Painter, Span, Style, YELLOW, line_width, sanitize,
};
use crate::table::Table;
use crate::wrap::wrap;

use inline::{Inline, InlineKind};

const QUOTE_BAR: &str = "\u{2502} ";
const BULLETS: [&str; 3] = ["\u{2022}", "\u{25e6}", "\u{25aa}"];
const RULE: &str = "\u{2500}";

/// Everything rendering needs that stays the same for a whole stream.
#[derive(Debug)]
pub(crate) struct Ctx {
    pub(crate) options: RenderOptions,
    pub(crate) painter: Painter,
    pub(crate) code: CodeStyle,
}

impl Ctx {
    pub(crate) fn new(options: RenderOptions, assets: &'static Assets) -> Ctx {
        let painter = Painter::new(options.colour(), options.hyperlinks());
        let code = CodeStyle { colour: options.colour(), theme: options.theme(), assets };
        Ctx { options, painter, code }
    }

    fn no_colour(&self) -> bool {
        self.options.colour() == ColourMode::None
    }
}

/// How the next slice continues the last block of the slice before it.
#[derive(Clone, Debug, Default)]
pub(crate) enum Continuation {
    #[default]
    None,
    /// More lines of a paragraph whose first lines are committed.
    Paragraph,
    /// More items of a list whose first items are committed: the next item's number
    /// (for an ordered list) and whether a blank line separated it from the last one.
    List { next: Option<u64>, blank_before: bool },
    /// More lines of a top-level code block whose first lines are committed. `skip`
    /// counts the rendered lines still in the pending text (an indented block keeps
    /// them; a fenced block drops them and keeps only its opening line).
    Code { block: CodeBlock, skip: usize },
}

/// What carries over from one committed slice to the next.
#[derive(Clone, Debug, Default)]
pub(crate) struct Flow {
    written: bool,
    separator: bool,
    pub(crate) continuation: Continuation,
}

impl Flow {
    /// A new top-level block starts; it owes a blank line if anything came before it.
    pub(crate) fn begin_block(&mut self) {
        if self.written {
            self.separator = true;
        }
    }

    /// Paints one line, after the blank line a new block owes.
    pub(crate) fn write(&mut self, ctx: &Ctx, out: &mut String, line: &[Span]) {
        if mem::take(&mut self.separator) {
            out.push('\n');
        }
        ctx.painter.paint(out, line);
        self.written = true;
    }
}

/// The last top-level list of a slice: the number its next item would get.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ListTail {
    pub(crate) next: Option<u64>,
}

/// Renders `text` as final blocks: nothing in it changes later. Consumes the flow's
/// continuation and returns the painted text and the tail of the last top-level list.
pub(crate) fn render_slice(ctx: &Ctx, flow: &mut Flow, text: &str) -> (String, Option<ListTail>) {
    let continuation = mem::take(&mut flow.continuation);
    let mut writer = Writer {
        ctx,
        continuing: !matches!(continuation, Continuation::None),
        continuation,
        flow,
        source: text,
        out: String::new(),
        depth: 0,
        top_blocks: 0,
        containers: Vec::new(),
        lists: Vec::new(),
        inline: None,
        styles: Vec::new(),
        links: Vec::new(),
        code: None,
        html: None,
        table: None,
        tail: None,
    };
    for (event, range) in Parser::new_ext(text, parser_options()).into_offset_iter() {
        writer.event(event, range);
    }
    writer.flush_inline();
    (writer.out, writer.tail)
}

enum Container {
    Quote { blocks: usize },
    Item(Item),
}

struct Item {
    number: Option<u64>,
    level: usize,
    task: Option<bool>,
    /// Set once the item's first line, the one with the marker, is written.
    marker_width: Option<usize>,
}

impl Item {
    fn marker(&self) -> Line {
        let mut line = Vec::new();
        if let Some(number) = self.number {
            line.push(Span::plain(format!("{number}. ")));
        }
        match self.task {
            Some(true) => line.extend([Span::new("[x]", Style::fg(GREEN)), Span::plain(" ")]),
            Some(false) => line.push(Span::plain("[ ] ")),
            None if self.number.is_none() => {
                line.push(Span::plain(format!("{} ", BULLETS[self.level % BULLETS.len()])));
            }
            None => {}
        }
        line
    }
}

struct ListState {
    next: Option<u64>,
    blank_before_next: bool,
}

struct LinkState {
    target: Option<String>,
    destination: String,
    /// Where the link text starts in the inline: (line, span).
    start: (usize, usize),
    /// For an image: its alt text, collected instead of shown as it arrives.
    alt: Option<String>,
}

struct Writer<'a> {
    ctx: &'a Ctx,
    flow: &'a mut Flow,
    source: &'a str,
    out: String,
    /// The slice continues a block of the previous one, so its first block owes no
    /// blank line.
    continuing: bool,
    /// Data the first block takes from the previous slice: list numbering or the
    /// code block state.
    continuation: Continuation,
    depth: usize,
    top_blocks: usize,
    containers: Vec<Container>,
    lists: Vec<ListState>,
    inline: Option<Inline>,
    styles: Vec<Style>,
    links: Vec<LinkState>,
    /// The code block being collected: its state, its text so far, and how many of
    /// its lines a previous slice rendered.
    code: Option<(CodeBlock, String, usize)>,
    html: Option<String>,
    table: Option<Table>,
    tail: Option<ListTail>,
}

impl Writer<'_> {
    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => {
                let top = self.depth == 0;
                self.depth += 1;
                self.start(tag, range, top);
            }
            Event::End(tag) => {
                self.depth = self.depth.saturating_sub(1);
                self.end(tag);
            }
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.inline_code(&code),
            Event::Html(html) => match &mut self.html {
                Some(block) => block.push_str(&html),
                None => self.text(&html),
            },
            Event::InlineHtml(html) | Event::InlineMath(html) | Event::DisplayMath(html) => {
                self.text(&html);
            }
            Event::FootnoteReference(label) => self.text(&format!("[^{label}]")),
            Event::SoftBreak | Event::HardBreak => self.line_break(),
            Event::Rule => {
                self.block_start(self.depth == 0);
                let width = self.available_width().max(3);
                self.emit(&[Span::new(RULE.repeat(width), Style::PLAIN.dim())], false);
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::Item(item)) =
                    self.containers.iter_mut().rev().find(|c| matches!(c, Container::Item(_)))
                {
                    item.task = Some(done);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>, top: bool) {
        match tag {
            Tag::Paragraph => {
                self.block_start(top);
                self.inline = Some(Inline::new(InlineKind::Paragraph));
            }
            Tag::Heading { level, .. } => {
                self.block_start(top);
                self.inline = Some(Inline::new(InlineKind::Heading(heading_style(level))));
            }
            Tag::BlockQuote(_) => {
                self.block_start(top);
                self.containers.push(Container::Quote { blocks: 0 });
            }
            Tag::CodeBlock(kind) => {
                self.block_start(top);
                let continued = if top && self.top_blocks == 1 {
                    match mem::take(&mut self.continuation) {
                        Continuation::Code { block, skip } => Some((block, skip)),
                        other => {
                            self.continuation = other;
                            None
                        }
                    }
                } else {
                    None
                };
                let (block, skip) = continued.unwrap_or_else(|| {
                    let info = match &kind {
                        CodeBlockKind::Fenced(info) => info.as_ref(),
                        CodeBlockKind::Indented => "",
                    };
                    (CodeBlock::new(info, self.ctx.code), 0)
                });
                self.code = Some((block, String::new(), skip));
            }
            Tag::HtmlBlock => {
                self.block_start(top);
                self.html = Some(String::new());
            }
            Tag::List(start) => {
                self.block_start(top);
                let mut state = ListState { next: start, blank_before_next: false };
                if top
                    && self.top_blocks == 1
                    && let Continuation::List { next, blank_before } =
                        mem::take(&mut self.continuation)
                {
                    state.next = start.and(next.or(start));
                    state.blank_before_next = blank_before;
                }
                self.lists.push(state);
            }
            Tag::Item => self.start_item(range),
            Tag::Table(aligns) => {
                self.block_start(top);
                self.table = Some(Table::new(aligns));
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.start_row();
                }
            }
            Tag::TableCell => self.inline = Some(Inline::new(InlineKind::Cell)),
            Tag::Emphasis => self.styles.push(Style::PLAIN.italic()),
            Tag::Strong => self.styles.push(Style::PLAIN.bold()),
            Tag::Strikethrough => self.styles.push(Style::PLAIN.strike()),
            Tag::Link { dest_url, .. } => {
                let start = self.inline_mut().position();
                let destination = dest_url.to_string();
                self.links.push(LinkState {
                    target: link_target(&destination),
                    destination,
                    start,
                    alt: None,
                });
                self.styles.push(link_style());
            }
            Tag::Image { dest_url, .. } => {
                let start = self.inline_mut().position();
                let destination = dest_url.to_string();
                self.links.push(LinkState {
                    target: link_target(&destination),
                    destination,
                    start,
                    alt: Some(String::new()),
                });
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.flush_inline(),
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.containers.pop();
            }
            TagEnd::CodeBlock => {
                if let Some((mut block, text, skip)) = self.code.take() {
                    if let Some(label) = block.start() {
                        self.emit(&label, false);
                    }
                    for line in source_lines(&text).into_iter().skip(skip) {
                        let rendered = block.line(line);
                        self.emit(&rendered, false);
                    }
                }
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    for line in source_lines(&html) {
                        let text = sanitize(line.strip_suffix('\r').unwrap_or(line)).into_owned();
                        self.emit(&[Span::new(text, Style::PLAIN.dim())], false);
                    }
                }
            }
            TagEnd::List(_) => {
                self.flush_inline();
                if let Some(list) = self.lists.pop()
                    && self.depth == 0
                {
                    self.tail = Some(ListTail { next: list.next });
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                let needs_marker = matches!(
                    self.containers.last(),
                    Some(Container::Item(Item { marker_width: None, .. }))
                );
                if needs_marker {
                    self.emit(&[], false);
                }
                self.containers.pop();
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    for line in table.layout(self.available_width()) {
                        if line.is_empty() {
                            self.blank_line();
                        } else {
                            self.emit(&line, false);
                        }
                    }
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    if tag == TagEnd::TableHead {
                        table.end_head();
                    } else {
                        table.end_row();
                    }
                }
            }
            TagEnd::TableCell => {
                let cell = self.inline.take().map(Inline::finish).unwrap_or_default();
                if let Some(table) = &mut self.table {
                    table.push_cell(cell.into_iter().flatten().collect());
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                self.end_link();
            }
            TagEnd::Image => self.end_image(),
            _ => {}
        }
    }

    fn start_item(&mut self, range: Range<usize>) {
        self.flush_inline();
        let ends_blank = ends_with_blank_line(self.source.get(range).unwrap_or_default());
        let (number, blank_before) = match self.lists.last_mut() {
            Some(list) => {
                let number = list.next;
                list.next = number.map(|n| n.saturating_add(1));
                (number, mem::replace(&mut list.blank_before_next, ends_blank))
            }
            None => (None, false),
        };
        if blank_before {
            self.blank_line();
        }
        let level = self.lists.len().saturating_sub(1);
        self.containers.push(Container::Item(Item {
            number,
            level,
            task: None,
            marker_width: None,
        }));
    }

    fn end_link(&mut self) {
        let Some(link) = self.links.pop() else { return };
        let shown = self.ctx.options.hyperlinks() && link.target.is_some();
        if shown || link.destination.is_empty() {
            return;
        }
        let inline = self.inline_mut();
        let text = inline.text_since(link.start);
        let redundant = text.as_deref().is_some_and(|text| {
            text == link.destination || link.destination.strip_prefix("mailto:") == Some(text)
        });
        if !redundant {
            let destination = sanitize(&link.destination).into_owned();
            inline.push_span(Span::new(format!(" ({destination})"), Style::PLAIN.dim()));
        }
    }

    fn end_image(&mut self) {
        let Some(image) = self.links.pop() else { return };
        let alt = image.alt.unwrap_or_default();
        let text = if alt.trim().is_empty() {
            "[image]".to_owned()
        } else {
            format!("[image: {}]", sanitize(alt.trim()))
        };
        let shown = self.ctx.options.hyperlinks() && image.target.is_some();
        let style = self.style().patch(Style::PLAIN.dim());
        let inline = self.inline_mut();
        inline.push_span(Span::linked(text, style, image.target));
        if !shown && !image.destination.is_empty() {
            let destination = sanitize(&image.destination).into_owned();
            inline.push_span(Span::new(format!(" ({destination})"), Style::PLAIN.dim()));
        }
    }

    fn text(&mut self, text: &str) {
        if let Some((_, buffer, _)) = &mut self.code {
            buffer.push_str(text);
            return;
        }
        if let Some(alt) = self.links.last_mut().and_then(|link| link.alt.as_mut()) {
            alt.push_str(text);
            return;
        }
        let style = self.style();
        let link = self.link();
        let autolink = self.links.is_empty();
        self.inline_mut().push_text(text, style, link, autolink);
    }

    fn inline_code(&mut self, code: &str) {
        if let Some(alt) = self.links.last_mut().and_then(|link| link.alt.as_mut()) {
            alt.push_str(code);
            return;
        }
        let clean = sanitize(code);
        let text = if self.ctx.no_colour() { format!("`{clean}`") } else { clean.into_owned() };
        let style = self.style().patch(Style::fg(YELLOW));
        let link = self.link().or_else(|| code_target(code));
        self.inline_mut().push_span(Span::linked(text, style, link));
    }

    fn line_break(&mut self) {
        if let Some(alt) = self.links.last_mut().and_then(|link| link.alt.as_mut()) {
            alt.push(' ');
            return;
        }
        let style = self.style();
        let link = self.link();
        let inline = self.inline_mut();
        if inline.breaks_lines() {
            inline.break_line();
        } else {
            inline.push_text(" ", style, link, false);
        }
    }

    /// The style for inline text here: the inline's base style with every open
    /// emphasis, strong, strikethrough and link style on top.
    fn style(&self) -> Style {
        let base = self.inline.as_ref().map_or(Style::PLAIN, Inline::base_style);
        self.styles.iter().fold(base, |style, over| style.patch(*over))
    }

    fn link(&self) -> Option<String> {
        self.links.last().and_then(|link| link.target.clone())
    }

    /// The current inline, starting one for text that arrives without a paragraph
    /// around it, as the text of a tight list item does.
    fn inline_mut(&mut self) -> &mut Inline {
        self.inline.get_or_insert_with(|| Inline::new(InlineKind::Paragraph))
    }

    /// Writes out the paragraph or heading in progress.
    fn flush_inline(&mut self) {
        let Some(inline) = self.inline.take() else { return };
        for line in inline.finish() {
            self.emit(&line, true);
        }
    }

    /// A block starts: at the top level it owes a blank line after what came before
    /// (unless it continues the previous slice's last block); inside a quote it is
    /// separated from the quote's previous block by a quoted blank line. Inside a list
    /// item, blocks follow each other without blank lines.
    fn block_start(&mut self, top: bool) {
        self.flush_inline();
        if top {
            self.top_blocks += 1;
            if !(self.continuing && self.top_blocks == 1) {
                self.flow.begin_block();
            }
        } else if let Some(Container::Quote { blocks }) = self.containers.last_mut() {
            *blocks += 1;
            if *blocks > 1 {
                self.blank_line();
            }
        }
    }

    /// Columns left for content after the container prefixes.
    fn available_width(&self) -> usize {
        let prefix: usize = self
            .containers
            .iter()
            .map(|container| match container {
                Container::Quote { .. } => QUOTE_BAR.chars().count(),
                Container::Item(item) => {
                    item.marker_width.unwrap_or_else(|| line_width(&item.marker()))
                }
            })
            .sum();
        self.ctx.options.columns().saturating_sub(prefix)
    }

    /// Writes `content` as one or more lines under the current containers, wrapping
    /// it when `wrap_text` is set and the containers indent it.
    fn emit(&mut self, content: &[Span], wrap_text: bool) {
        let (first, rest) = self.prefixes();
        let prefix_width = line_width(&first);
        let lines = if wrap_text && prefix_width > 0 {
            let available =
                self.ctx.options.columns().saturating_sub(prefix_width).max(MIN_TEXT_WIDTH);
            wrap(content, available)
        } else {
            vec![content.to_vec()]
        };
        for (index, line) in lines.into_iter().enumerate() {
            let mut full = if index == 0 { first.clone() } else { rest.clone() };
            full.extend(line);
            self.flow.write(self.ctx, &mut self.out, &full);
        }
    }

    /// The prefixes for the next line and for its continuation lines. The first line
    /// of a list item carries the item's marker; every later line an indent as wide.
    fn prefixes(&mut self) -> (Line, Line) {
        let mut first = Vec::new();
        let mut rest = Vec::new();
        for container in &mut self.containers {
            match container {
                Container::Quote { .. } => {
                    first.push(Span::new(QUOTE_BAR, Style::PLAIN.dim()));
                    rest.push(Span::new(QUOTE_BAR, Style::PLAIN.dim()));
                }
                Container::Item(item) => {
                    if let Some(width) = item.marker_width {
                        first.push(Span::plain(" ".repeat(width)));
                    } else {
                        let marker = item.marker();
                        let width = line_width(&marker);
                        item.marker_width = Some(width);
                        first.extend(marker);
                    }
                    rest.push(Span::plain(" ".repeat(item.marker_width.unwrap_or(0))));
                }
            }
        }
        (first, rest)
    }

    /// A blank line inside the current containers: quote bars stay, indents go.
    fn blank_line(&mut self) {
        let mut line = Vec::new();
        for container in &self.containers {
            match container {
                Container::Quote { .. } => line.push(Span::new(QUOTE_BAR, Style::PLAIN.dim())),
                Container::Item(item) => {
                    line.push(Span::plain(" ".repeat(item.marker_width.unwrap_or(0))));
                }
            }
        }
        self.flow.write(self.ctx, &mut self.out, &line);
    }
}

fn heading_style(level: HeadingLevel) -> Style {
    let colour = match level {
        HeadingLevel::H1 => MAGENTA,
        HeadingLevel::H2 => BLUE,
        _ => CYAN,
    };
    Style::fg(colour).bold()
}

fn link_style() -> Style {
    Style::fg(BLUE).underline()
}

/// True when the last line of `text` (before its final newline) is blank, as the
/// source of a list item followed by a blank line ends.
pub(crate) fn ends_with_blank_line(text: &str) -> bool {
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.rsplit_once('\n').is_some_and(|(_, last)| last.trim().is_empty())
}

#[cfg(test)]
mod tests;
