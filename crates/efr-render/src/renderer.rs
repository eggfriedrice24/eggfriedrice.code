//! The streaming renderer: markdown arrives in pieces, complete blocks are committed
//! once (to be written to scrollback and never rewritten), and the block still
//! arriving is the live zone, which the caller redraws in place.
//!
//! What gets committed is decided once per complete source line, from the text up to
//! that line and nothing after it. The sequence of complete lines does not depend on
//! how the text was split into pieces, so neither do the commits: a reply streamed in
//! any chunks commits exactly what [`render`] produces for the whole reply.

use std::fmt;

use crate::block::{Continuation, Ctx, Flow, ListTail, ends_with_blank_line, render_slice};
use crate::code::{CodeBlock, source_lines};
use crate::highlight::{ASSETS, Assets};
use crate::options::RenderOptions;
use crate::outline::{CodeText, Kind, line_start, outline};
use crate::width::{WidthMethod, display_width};

/// Renders a whole markdown document at once. The result equals everything a
/// [`Renderer`] commits for the same text pushed in any pieces, followed by its
/// [`finish`](Renderer::finish).
pub fn render(markdown: &str, options: &RenderOptions) -> String {
    let mut renderer = Renderer::new(options.clone());
    let mut out = renderer.push(markdown).into_committed();
    out.push_str(&renderer.finish());
    out
}

/// What one [`Renderer::push`] produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Update {
    committed: String,
    live: String,
    live_rows: usize,
    width: usize,
    method: WidthMethod,
}

impl Update {
    /// Newly committed output: write it once, below what was committed before and in
    /// place of the old live zone. It is never rewritten.
    pub fn committed(&self) -> &str {
        &self.committed
    }

    /// The live zone as it stands now: the block still arriving. The caller erases the
    /// previous live zone and writes this one after the committed output, inside
    /// synchronized output (mode 2026) so the redraw does not flicker. Every line
    /// opens and closes its own SGR state and hyperlinks.
    pub fn live(&self) -> &str {
        &self.live
    }

    /// How many terminal rows the live zone takes at the renderer's width, counting
    /// the rows long lines wrap onto; the caller moves up this many to erase it.
    pub fn live_rows(&self) -> usize {
        self.live_rows
    }

    /// The last lines of the live zone that fit in `max_rows` rows, for a caller that
    /// must keep the live zone smaller than the screen. Whole lines only, which is
    /// safe because every line carries its own state: when even the last line is
    /// taller than `max_rows`, the result is empty.
    pub fn live_tail(&self, max_rows: usize) -> &str {
        let mut rows = 0;
        let mut start = self.live.len();
        for line in self.live.split_inclusive('\n').rev() {
            rows += rows_of(line, self.width, self.method);
            if rows > max_rows {
                break;
            }
            start -= line.len();
        }
        &self.live[start..]
    }

    /// The committed output, without the live zone.
    pub fn into_committed(self) -> String {
        self.committed
    }
}

fn rows_of(line: &str, width: usize, method: WidthMethod) -> usize {
    let line = line.strip_suffix('\n').unwrap_or(line);
    display_width(line, method).div_ceil(width.max(1)).max(1)
}

/// A streaming markdown renderer for one reply.
///
/// [`push`](Renderer::push) the reply's text as it arrives; each push returns the
/// output it committed and the current live zone. [`finish`](Renderer::finish)
/// commits whatever is left. When the options say the output is not a terminal, the
/// markdown passes through unchanged and there is no live zone.
pub struct Renderer {
    ctx: Ctx,
    /// Text not committed yet. It starts at a block boundary, or where a block whose
    /// first part was committed continues.
    pending: String,
    /// How much of `pending` is complete lines already looked at.
    scanned: usize,
    flow: Flow,
}

impl fmt::Debug for Renderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The pending text can be long; its length is enough to debug the stream.
        f.debug_struct("Renderer")
            .field("options", &self.ctx.options)
            .field("pending_bytes", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl Renderer {
    /// A renderer for one reply.
    pub fn new(options: RenderOptions) -> Renderer {
        Renderer::with_assets(options, &ASSETS)
    }

    pub(crate) fn with_assets(options: RenderOptions, assets: &'static Assets) -> Renderer {
        Renderer {
            ctx: Ctx::new(options, assets),
            pending: String::new(),
            scanned: 0,
            flow: Flow::default(),
        }
    }

    /// The options this renderer was made with.
    pub fn options(&self) -> &RenderOptions {
        &self.ctx.options
    }

    /// Takes the next piece of the reply. Pieces may split the text anywhere, even
    /// inside `**` or a code fence.
    pub fn push(&mut self, markdown: &str) -> Update {
        let width = self.ctx.options.columns();
        let method = self.ctx.options.width_method();
        if !self.ctx.options.is_terminal() {
            let committed = markdown.to_owned();
            return Update { committed, live: String::new(), live_rows: 0, width, method };
        }
        self.pending.push_str(markdown);
        let mut committed = String::new();
        while let Some(newline) = self.pending[self.scanned..].find('\n') {
            self.scanned += newline + 1;
            self.step(&mut committed);
        }
        let live = self.live();
        let live_rows = live.split_inclusive('\n').map(|line| rows_of(line, width, method)).sum();
        Update { committed, live, live_rows, width, method }
    }

    /// Commits everything still pending, as if the reply ended here. Returns the
    /// output to write in place of the live zone.
    pub fn finish(mut self) -> String {
        if !self.ctx.options.is_terminal() || self.pending.is_empty() {
            return String::new();
        }
        let pending = std::mem::take(&mut self.pending);
        render_slice(&self.ctx, &mut self.flow, &pending).0
    }

    /// The live zone: what finishing now would commit, rendered on a copy of the state.
    fn live(&self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        let mut flow = self.flow.clone();
        render_slice(&self.ctx, &mut flow, &self.pending).0
    }

    /// Decides what one more complete line lets commit. Blocks that another block
    /// follows are final. The last block is final when its own syntax closes it;
    /// otherwise a long paragraph commits its earlier lines, a list its earlier items
    /// and a code block each complete line.
    fn step(&mut self, out: &mut String) {
        loop {
            let text = &self.pending[..self.scanned];
            let blocks = outline(text);
            let Some(last) = blocks.last() else { return };
            let last_start = line_start(text, last.range.start);
            if blocks.len() > 1 && last_start > 0 {
                out.push_str(&self.commit(last_start).0);
                continue;
            }
            if last.is_closed(text) {
                out.push_str(&self.commit(self.scanned).0);
                return;
            }
            match last.kind {
                Kind::Paragraph => {
                    if let Some(cut) = last.paragraph_cut(text) {
                        out.push_str(&self.commit(cut).0);
                        self.flow.continuation = Continuation::Paragraph;
                    }
                }
                Kind::List if last.items.len() > 1 => {
                    let cut = last.items.last().map_or(0, |item| line_start(text, *item));
                    if cut > 0 {
                        let blank_before = ends_with_blank_line(&text[..cut]);
                        let (rendered, tail) = self.commit(cut);
                        out.push_str(&rendered);
                        let next = tail.and_then(|tail| tail.next);
                        self.flow.continuation = Continuation::List { next, blank_before };
                    }
                }
                Kind::Code => {
                    if let Some(code) = &last.code {
                        self.advance_code(last.range.start, code, out);
                    }
                }
                _ => {}
            }
            return;
        }
    }

    /// Renders and drops the first `cut` bytes of pending text as final.
    fn commit(&mut self, cut: usize) -> (String, Option<ListTail>) {
        let slice: String = self.pending.drain(..cut).collect();
        self.scanned -= cut;
        render_slice(&self.ctx, &mut self.flow, &slice)
    }

    /// Commits the label and every complete line of the open code block that starts at
    /// `start` in the pending text and is not committed yet. A fenced block's rendered
    /// lines are dropped from the pending text, keeping its opening line, which alone
    /// decides where the block ends; so a long block is not parsed again for every
    /// line. An indented block keeps its lines, because a blank line in it belongs to
    /// it only once more code follows.
    fn advance_code(&mut self, start: usize, code: &CodeText, out: &mut String) {
        let (mut block, skip) = match std::mem::take(&mut self.flow.continuation) {
            Continuation::Code { block, skip } => (block, skip),
            _ => {
                self.flow.begin_block();
                (CodeBlock::new(&code.info, &self.ctx.code), 0)
            }
        };
        if let Some(label) = block.start() {
            self.flow.write(&self.ctx, out, &label);
        }
        let lines = source_lines(&code.text);
        for line in lines.iter().skip(skip) {
            let rendered = block.line(line);
            self.flow.write(&self.ctx, out, &rendered);
        }
        let skip = if code.fenced {
            // Each line of a top-level fenced block is one source line after the
            // opening line; a closing fence after them stays.
            let opening_end =
                self.pending[start..].find('\n').map_or(self.scanned, |at| start + at + 1);
            let mut end = opening_end;
            for _ in 0..lines.len() {
                match self.pending[end..self.scanned].find('\n') {
                    Some(at) => end += at + 1,
                    None => break,
                }
            }
            self.pending.drain(opening_end..end);
            self.scanned -= end - opening_end;
            0
        } else {
            lines.len()
        };
        self.flow.continuation = Continuation::Code { block, skip };
    }
}

#[cfg(test)]
mod tests;
