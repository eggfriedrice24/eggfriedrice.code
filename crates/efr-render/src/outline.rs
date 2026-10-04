//! The top-level shape of the complete lines received so far: which blocks there are,
//! where they start, and which of them can no longer change. The streaming renderer
//! decides what to commit from this; `block` does the rendering.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

/// An open paragraph keeps at most this many complete lines in the live zone; past
/// that, its earlier lines commit.
pub(crate) const PARAGRAPH_LIVE_LINES: usize = 3;

/// The markdown dialect: CommonMark plus GitHub's tables, strikethrough and task
/// lists. Every parse in the crate uses it, so the outline and the rendering agree.
pub(crate) fn parser_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Paragraph,
    Heading,
    Rule,
    Quote,
    List,
    Code,
    Table,
    Other,
}

/// A code block's info string and text, as far as it has arrived.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CodeText {
    pub(crate) info: String,
    pub(crate) text: String,
    /// Fenced, as opposed to indented: only its opening line decides where it ends.
    pub(crate) fenced: bool,
}

#[derive(Debug)]
pub(crate) struct TopBlock {
    pub(crate) kind: Kind,
    pub(crate) range: Range<usize>,
    /// Where each item starts, for a list.
    pub(crate) items: Vec<usize>,
    /// The info string and text, for a code block.
    pub(crate) code: Option<CodeText>,
}

/// The top-level blocks of `text`, in order.
pub(crate) fn outline(text: &str) -> Vec<TopBlock> {
    let mut blocks: Vec<TopBlock> = Vec::new();
    let mut depth = 0_usize;
    for (event, range) in Parser::new_ext(text, parser_options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    blocks.push(top_block(&tag, range));
                } else if depth == 1
                    && matches!(tag, Tag::Item)
                    && let Some(list) = blocks.last_mut()
                {
                    list.items.push(range.start);
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Rule if depth == 0 => {
                blocks.push(TopBlock { kind: Kind::Rule, range, items: Vec::new(), code: None });
            }
            Event::Text(piece) if depth == 1 => {
                if let Some(code) = blocks.last_mut().and_then(|block| block.code.as_mut()) {
                    code.text.push_str(&piece);
                }
            }
            _ => {}
        }
    }
    blocks
}

fn top_block(tag: &Tag<'_>, range: Range<usize>) -> TopBlock {
    let mut code = None;
    let kind = match tag {
        Tag::Paragraph => Kind::Paragraph,
        Tag::Heading { .. } => Kind::Heading,
        Tag::BlockQuote(_) => Kind::Quote,
        Tag::List(_) => Kind::List,
        Tag::Table(_) => Kind::Table,
        Tag::CodeBlock(kind) => {
            let (info, fenced) = match kind {
                CodeBlockKind::Fenced(info) => (info.to_string(), true),
                CodeBlockKind::Indented => (String::new(), false),
            };
            code = Some(CodeText { info, text: String::new(), fenced });
            Kind::Code
        }
        _ => Kind::Other,
    };
    TopBlock { kind, range, items: Vec::new(), code }
}

impl TopBlock {
    /// True when no further text can change this block, given that it is the last
    /// block of `text` (which ends at a line end). A heading or a rule is closed by
    /// its own line; a paragraph, a table or a quote by a blank line after it. Lists
    /// and code blocks go on across blank lines, so they close only when another
    /// block starts.
    pub(crate) fn is_closed(&self, text: &str) -> bool {
        match self.kind {
            Kind::Heading | Kind::Rule => true,
            Kind::Paragraph | Kind::Table | Kind::Quote => {
                let rest = text.get(self.range.end..).unwrap_or_default();
                rest.split_inclusive('\n')
                    .next()
                    .is_some_and(|line| line.ends_with('\n') && line.trim().is_empty())
            }
            Kind::List | Kind::Code | Kind::Other => false,
        }
    }

    /// For an open paragraph with more complete lines than the live zone keeps, the
    /// offset of the latest line that can start a paragraph of its own: everything
    /// before it commits now, and it and the lines after it stay live. The lines that
    /// stay are at least the last one, so a table delimiter row or a setext underline
    /// arriving next still finds the line it applies to.
    pub(crate) fn paragraph_cut(&self, text: &str) -> Option<usize> {
        if self.kind != Kind::Paragraph {
            return None;
        }
        let start = line_start(text, self.range.start);
        let mut lines = Vec::new();
        let mut offset = start;
        for line in text[start..].split_inclusive('\n') {
            lines.push((offset, line));
            offset += line.len();
        }
        if lines.len() <= PARAGRAPH_LIVE_LINES {
            return None;
        }
        lines[1..].iter().rev().find(|(_, line)| starts_paragraph(line)).map(|(offset, _)| *offset)
    }
}

/// True for a line that, read on its own, starts a plain paragraph exactly as it
/// continued one: at most three spaces, then a letter. No block construct (list,
/// quote, fence, heading, table row, HTML, link definition) starts with a letter.
fn starts_paragraph(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    line.len() - trimmed.len() <= 3 && trimmed.chars().next().is_some_and(char::is_alphabetic)
}

/// The start of the line that holds `offset`. Blocks are cut at line starts, because
/// a block's own range can begin after its indentation.
pub(crate) fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |newline| newline + 1)
}

#[cfg(test)]
mod tests;
