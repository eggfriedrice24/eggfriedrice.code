//! An open fenced code block inside the last block of the pending text (a list item
//! or a quote), followed one line at a time.
//!
//! Inside a fenced code block, the parser decides what a line is from that line and
//! the containers around the block only: a line of code changes no state that the
//! next line depends on. So a line is a line of the block's code exactly when it is
//! one directly after the block's opening line. The renderer checks each new line
//! against the short text up to the opening line, not against the whole block. While
//! each line is code, nothing can commit (an open list item or quote commits only
//! when a later block starts), and the live zone grows by one painted line.
//!
//! In a list, the follow goes on after the closing fence through the lines that add
//! nothing, such as blank lines. When the next item starts, the item commits as the
//! live zone shows it, so the block is not rendered again.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use crate::block::{Ctx, Flow, ListTail, OpenCode, paint_code_line, render_open_code};
use crate::outline::parser_options;

use super::rows_of_text;

pub(crate) struct OpenFence {
    /// The pending text up to the end of the fence's opening line.
    head: usize,
    /// The events of that text alone.
    events: Vec<Event<'static>>,
    /// The fence is in a list, not in a quote, which a blank line ends.
    in_list: bool,
    /// The lines of code after the opening line.
    lines: usize,
    /// Once the fence closed in a list: the closing line and the lines after it.
    closed: Option<String>,
    live: Live,
}

/// The live zone of the pending text without its partial line, made once and then
/// grown by each line of code.
enum Live {
    Unbuilt,
    Built {
        open: OpenCode,
        flow: Flow,
        rows: usize,
    },
    /// The rendering does not end with the block's lines; each live zone is rendered
    /// in full.
    Unavailable,
}

/// The rendering of the pending text up to `end`, kept from a follow that ended at
/// a line which starts there: the output, the tail of its last list and the flow
/// after it, as [`crate::block::render_slice`] gives them for that text.
pub(crate) struct Ended {
    pub(crate) end: usize,
    pub(crate) out: String,
    pub(crate) tail: Option<ListTail>,
    pub(crate) flow: Flow,
}

impl OpenFence {
    /// The fence whose opening fence starts at `start` in `text`, which ends at a
    /// line end. `None` unless the fence is the last element of the text, inside a
    /// list item (`in_list`) or a quote, and each line after its opening line is
    /// code.
    pub(crate) fn new(text: &str, start: usize, in_list: bool) -> Option<OpenFence> {
        let head = text[start..].find('\n').map(|at| start + at + 1)?;
        let events = events_of(&text[..head]);
        if !ends_with_nested_fence(&events) {
            return None;
        }
        let fence =
            OpenFence { head, events, in_list, lines: 0, closed: None, live: Live::Unbuilt };
        // Newest line first: after a closing fence only blank lines stay in the
        // block, so a closed fence fails at its last lines.
        let mut lines = 0;
        for line in text[head..].split_inclusive('\n').rev() {
            fence.code(&text[..head], line, true)?;
            lines += 1;
        }
        Some(OpenFence { lines, ..fence })
    }

    /// The end of the fence's opening line in the pending text.
    pub(crate) fn head(&self) -> usize {
        self.head
    }

    /// Takes the next complete line of the pending text, after `head`. False when
    /// the line can change what commits or how the block renders: it is not code of
    /// the open block, and it adds more than nothing after the closed one.
    pub(crate) fn take_line(&mut self, ctx: &Ctx, head: &str, line: &str) -> bool {
        if let Some(after) = &self.closed {
            let text = format!("{after}{line}");
            if !self.adds_nothing(head, &text) {
                return false;
            }
            self.closed = Some(text);
            return true;
        }
        let Some(code) = self.code(head, line, true) else {
            if self.in_list && self.adds_nothing(head, line) {
                self.closed = Some(line.to_owned());
                return true;
            }
            return false;
        };
        self.lines += 1;
        if let Live::Built { open, rows, .. } = &mut self.live {
            let source = code.strip_suffix('\n').unwrap_or(&code);
            let painted = paint_code_line(ctx, &open.prefix, &mut open.block, source);
            *rows += rows_of_text(&painted, ctx);
            open.out.push_str(&painted);
        }
        true
    }

    /// The live zone of `pending`, whose complete lines end at `scanned`, and its
    /// rows: what [`crate::block::render_slice`] gives for all of `pending`. When the
    /// partial last line is not code of the block, a `long` pending text shows the
    /// live zone without it. `None` when the caller must render it in full.
    pub(crate) fn live(
        &mut self,
        ctx: &Ctx,
        flow: &Flow,
        pending: &str,
        scanned: usize,
        long: bool,
    ) -> Option<(String, usize)> {
        if matches!(self.live, Live::Unbuilt) {
            // NOTE: with no line of code yet, an item marker can still be owed.
            if self.lines == 0 {
                return None;
            }
            let mut flow = flow.clone();
            self.live = match render_open_code(ctx, &mut flow, &pending[..scanned]) {
                Some(open) => {
                    let rows = rows_of_text(&open.out, ctx);
                    Live::Built { open, flow, rows }
                }
                None => Live::Unavailable,
            };
        }
        let Live::Built { open, rows, .. } = &self.live else { return None };
        let head = &pending[..self.head];
        let partial = &pending[scanned..];
        let mut live = String::with_capacity(open.out.len() + 2 * partial.len() + 64);
        live.push_str(&open.out);
        let mut rows = *rows;
        let shown = match &self.closed {
            _ if partial.is_empty() => true,
            Some(after) => self.adds_nothing(head, &format!("{after}{partial}")),
            None => match self.code(head, partial, false) {
                Some(code) if !code.is_empty() => {
                    let mut block = open.block.clone();
                    let painted = paint_code_line(ctx, &open.prefix, &mut block, &code);
                    rows += rows_of_text(&painted, ctx);
                    live.push_str(&painted);
                    true
                }
                Some(_) => true,
                None => false,
            },
        };
        (shown || long).then_some((live, rows))
    }

    /// The kept rendering, when there is one, for the pending text up to `end`.
    pub(crate) fn into_ended(self, end: usize) -> Option<Ended> {
        match self.live {
            Live::Built { open, flow, .. } => {
                Some(Ended { end, out: open.out, tail: open.tail, flow })
            }
            Live::Unbuilt | Live::Unavailable => None,
        }
    }

    /// The code that `line` adds to the block when it follows `head`, the text up to
    /// the opening line. `None` when the line is not code of this block: the events
    /// of `head` and `line` together, without the block's text, must be the events of
    /// `head` alone. A `complete` line ends with its newline and must stay in the
    /// block; the partial last line may also close the fence, which adds no code.
    fn code(&self, head: &str, line: &str, complete: bool) -> Option<String> {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = if complete { body.strip_suffix('\r').unwrap_or(body) } else { body };
        if complete != line.ends_with('\n') || body.contains(['\r', '\n']) {
            // NOTE: a lone carriage return also ends a line in markdown.
            return None;
        }
        let text = format!("{head}{line}");
        let events: Vec<Event<'_>> = Parser::new_ext(&text, parser_options()).collect();
        let start =
            events.iter().rposition(|event| matches!(event, Event::Start(Tag::CodeBlock(_))))?;
        let mut code = String::new();
        let mut rest = Vec::with_capacity(events.len());
        let mut inside = false;
        for (index, event) in events.into_iter().enumerate() {
            inside |= index == start;
            match event {
                Event::Text(piece) if inside => {
                    code.push_str(&piece);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => inside = false,
                _ => {}
            }
            rest.push(event);
        }
        if rest[..] != self.events[..] {
            return None;
        }
        let newlines = code.matches('\n').count();
        let fits = if complete { newlines == 1 && code.ends_with('\n') } else { newlines == 0 };
        fits.then_some(code)
    }

    /// True when `after`, the text after the block's code, adds no event to `head`:
    /// it closes the fence, then has only lines such as blank ones. The code itself
    /// changes no state of the parser, so it is left out.
    fn adds_nothing(&self, head: &str, after: &str) -> bool {
        let text = format!("{head}{after}");
        Parser::new_ext(&text, parser_options()).eq(self.events.iter().cloned())
    }
}

fn events_of(text: &str) -> Vec<Event<'static>> {
    Parser::new_ext(text, parser_options()).map(Event::into_static).collect()
}

/// True when the last element that starts in `events` is a fenced code block inside
/// another block, with no code yet: only ends follow it.
fn ends_with_nested_fence(events: &[Event<'_>]) -> bool {
    let Some(start) = events.iter().rposition(|event| matches!(event, Event::Start(_))) else {
        return false;
    };
    let depth = events[..start].iter().fold(0_isize, |depth, event| match event {
        Event::Start(_) => depth + 1,
        Event::End(_) => depth - 1,
        _ => depth,
    });
    matches!(events[start], Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_))))
        && depth > 0
        && events[start + 1..].iter().all(|event| matches!(event, Event::End(_)))
}
