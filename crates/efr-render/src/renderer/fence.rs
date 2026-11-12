//! An open fenced code block inside the last block of the pending text (a list item
//! or a quote), followed one line at a time.
//!
//! Inside a fenced code block, the parser decides what a line is from that line and
//! the containers around the block only: a line of code changes no state that the
//! next line depends on. So a line is a line of the block's code exactly when it is
//! one directly after the block's opening line. The renderer checks each new line
//! against the short text up to the opening line (the head), not against the block.
//!
//! Each complete line of code commits at once, as a line of a top-level block does:
//! the first one with the rendering of the head, then each one painted alone under
//! the prefix of the block's lines. A committed line leaves the pending text, so the
//! pending text keeps the head and the lines that are not committed, and no work for
//! a line grows with the block. The flow carries a [`Continuation::Nested`], so a
//! later rendering of the pending text writes only what comes after the committed
//! lines. A blank line of code waits for the next line of code that is not blank:
//! at the end of a list item, the parser leaves blank lines out of the block.
//!
//! In a list, the follow goes on after the closing fence through the lines that add
//! nothing, such as blank lines, so the live zone stays empty until the item goes on.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use crate::block::{Continuation, Ctx, Flow, paint_code_line, render_open_code};
use crate::outline::parser_options;
use crate::style::Line;

use super::{Stream, rows_of_text};

pub(crate) struct OpenFence {
    /// Where the fence starts in the pending text.
    at: usize,
    /// The pending text up to the end of the fence's opening line.
    head: usize,
    /// The events of that text alone.
    events: Vec<Event<'static>>,
    /// The fence is in a list, not in a quote, which a blank line ends.
    in_list: bool,
    state: State,
    /// Once the fence closed in a list: the closing line and the lines after it.
    closed: Option<String>,
}

enum State {
    /// No line of code that is not blank yet: an item marker can still be owed.
    Waiting,
    /// The head and the lines of code so far are committed; the next line of code is
    /// painted under this prefix. `held` is the code of the blank lines after the
    /// last committed line: they stay in the pending text until a line of code that
    /// is not blank follows, because at the end of a list item they are not code.
    Committing { prefix: Line, held: Vec<String> },
    /// The rendering of the head does not end with the block's lines: the lines stay
    /// in the pending text and each live zone is rendered in full.
    Plain,
}

impl OpenFence {
    /// Follows the fence whose opening fence starts at `at` in the complete lines of
    /// `stream`, and commits to `out` the lines of code that came after its opening
    /// line already. `None` unless the fence is the last element of the text, inside a
    /// list item (`in_list`) or a quote, and each line after its opening line is code.
    pub(crate) fn new(
        ctx: &Ctx,
        stream: &mut Stream,
        at: usize,
        in_list: bool,
        out: &mut String,
    ) -> Option<OpenFence> {
        let text = &stream.pending[..stream.scanned];
        let head = text[at..].find('\n').map(|end| at + end + 1)?;
        let events = events_of(&text[..head]);
        if !ends_with_nested_fence(&events) {
            return None;
        }
        let mut fence =
            OpenFence { at, head, events, in_list, state: State::Waiting, closed: None };
        // Newest line first: after a closing fence only blank lines stay in the
        // block, so a closed fence fails at its last lines.
        let mut last = None;
        for line in text[head..].split_inclusive('\n').rev() {
            let code = fence.code(&text[..head], line, true)?;
            last.get_or_insert(code);
        }
        if last.is_some_and(|code| !is_blank(&code)) {
            fence.start(ctx, stream, out);
        }
        Some(fence)
    }

    /// Takes the complete line that starts at `start` and ends the scanned text of
    /// `stream`, and commits it to `out` when it is code. False when the line can
    /// change what commits or how the block renders: it is not code of the open
    /// block, and it adds more than nothing after the closed one.
    pub(crate) fn take_line(
        &mut self,
        ctx: &Ctx,
        stream: &mut Stream,
        start: usize,
        out: &mut String,
    ) -> bool {
        let head = &stream.pending[..self.head];
        let line = &stream.pending[start..stream.scanned];
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
        let blank = is_blank(&code);
        if matches!(self.state, State::Waiting) {
            if !blank {
                self.start(ctx, stream, out);
            }
            return true;
        }
        let State::Committing { prefix, held } = &mut self.state else { return true };
        if blank {
            held.push(code);
            return true;
        }
        let Continuation::Nested { block, .. } = &mut stream.flow.continuation else {
            // NOTE: nothing else sets the flow while the fence is followed.
            self.state = State::Plain;
            return true;
        };
        for code in held.drain(..).chain([code]) {
            let source = code.strip_suffix('\n').unwrap_or(&code);
            out.push_str(&paint_code_line(ctx, prefix, block, source));
        }
        stream.pending.truncate(self.head);
        stream.scanned = self.head;
        true
    }

    /// Commits the head and the lines of code after it, the complete lines of
    /// `stream`, as [`crate::block::render_slice`] renders them, and keeps only the
    /// head in the pending text.
    fn start(&mut self, ctx: &Ctx, stream: &mut Stream, out: &mut String) {
        let mut flow = stream.flow.clone();
        let before = match flow.continuation.clone() {
            Continuation::Nested { before, .. } => *before,
            other => other,
        };
        let Some(open) = render_open_code(ctx, &mut flow, &stream.pending[..stream.scanned]) else {
            self.state = State::Plain;
            return;
        };
        out.push_str(&open.out);
        flow.continuation =
            Continuation::Nested { before: Box::new(before), at: self.at, block: open.block };
        stream.flow = flow;
        stream.pending.truncate(self.head);
        stream.scanned = self.head;
        self.state = State::Committing { prefix: open.prefix, held: Vec::new() };
    }

    /// The live zone of `pending`, whose complete lines end at `scanned`, and its
    /// rows: what [`crate::block::render_slice`] gives for all of `pending` after
    /// the committed lines, with `flow`. `None` when the caller must render it.
    pub(crate) fn live(
        &self,
        ctx: &Ctx,
        flow: &Flow,
        pending: &str,
        scanned: usize,
    ) -> Option<(String, usize)> {
        let State::Committing { prefix, held } = &self.state else { return None };
        let Continuation::Nested { block, .. } = &flow.continuation else { return None };
        let head = &pending[..self.head];
        let partial = &pending[scanned..];
        if partial.is_empty() && held.is_empty() {
            return Some((String::new(), 0));
        }
        let painted = match &self.closed {
            Some(after)
                if held.is_empty() && self.adds_nothing(head, &format!("{after}{partial}")) =>
            {
                String::new()
            }
            Some(_) => return None,
            // NOTE: blank lines show only with code that is not blank after them.
            None => match self.code(head, partial, false)? {
                code if is_blank(&code) => return None,
                code => {
                    let mut block = block.clone();
                    let mut painted = String::new();
                    for line in held {
                        let source = line.strip_suffix('\n').unwrap_or(line);
                        painted.push_str(&paint_code_line(ctx, prefix, &mut block, source));
                    }
                    painted.push_str(&paint_code_line(ctx, prefix, &mut block, &code));
                    painted
                }
            },
        };
        let rows = rows_of_text(&painted, ctx);
        Some((painted, rows))
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

/// True for a line, or the code of one, with nothing but spaces and tabs.
fn is_blank(line: &str) -> bool {
    line.trim_end_matches(['\n', '\r']).chars().all(|c| c == ' ' || c == '\t')
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
