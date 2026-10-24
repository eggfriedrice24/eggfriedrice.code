//! The lines of `efr search`: the plain list of the conversations that match, best
//! first, and for the picker of `,resume` the rows and the header that fzf shows and
//! the preview of one conversation.
//!
//! Every text from efrd passes through [`one_line`] before a colour is added, and every
//! range of a match is clamped to its text ([`clamp_ranges`]), so a bad answer can
//! neither drive the terminal nor panic `efr`. The matching parts show in the `accent`
//! role, the colour that the picker gives fzf's own matches.

use std::fmt::Write as _;
use std::ops::Range;

use efr_protocol::{
    ConversationSearchResult, ConversationsSearchResult, SearchIndexState, SearchKind,
    SearchPartial, SearchRow, TextRange,
};
use efr_render::{RenderOptions, WidthMethod, text_width};
use jiff::Timestamp;
use unicode_segmentation::UnicodeSegmentation as _;

use super::resume::{MIN_TITLE, Row, pad, pad_left};
use super::route::age;
use super::{Tone, columns, keys, one_line, paint};

/// The widest title of a row of the picker.
const PICKER_TITLE: usize = 48;

/// The widest scope and holder of a row of the picker.
const PICKER_CELL: usize = 24;

/// The widest snippet of a row of the picker. fzf cuts a row at the edge of its window.
const PICKER_SNIPPET: usize = 120;

/// The most bytes of the text of a title or a snippet of the picker before its
/// colours. A cut by columns alone does not bound the bytes: a character of zero width
/// takes no column.
const TEXT_BYTES: usize = 160;

/// The most bytes of the text of a scope or a holder of the picker.
const CELL_BYTES: usize = 64;

/// The most bytes of a title of the picker with its colours.
const TITLE_BYTES: usize = 200;

/// The most bytes of one row of the picker, with its colours and its line end, so that
/// the header and 50 rows stay within 32 KiB.
pub(crate) const ROW_BYTES: usize = 640;

/// The code points before the first match that a cut line keeps.
const LEAD: usize = 16;

/// The indent of the snippet under a row of the plain list.
const INDENT: &str = "    ";

/// What stands for the text that a cut removed.
const CUT: &str = "\u{2026}";

/// What stands for a conversation without a title.
const NO_TITLE: &str = "(no title)";

/// The keys of the picker, as its header shows them.
const KEYS: [(&str, &str); 4] =
    [("enter", "resume"), ("ctrl-o", "print"), ("ctrl-d", "delete"), ("esc", "close")];

/// The byte ranges in `text` of `ranges`, which count Unicode code points: each one cut
/// to the text, an empty or reversed one dropped, sorted, and those that overlap or
/// touch merged. Text from another efrd may come with any ranges, and none of them can
/// make a slice of `text` panic.
pub(crate) fn clamp_ranges(text: &str, ranges: &[TextRange]) -> Vec<Range<usize>> {
    if ranges.is_empty() {
        return Vec::new();
    }
    let offsets: Vec<usize> =
        text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).collect();
    let chars = offsets.len() - 1;
    let to_chars = |at: u32| usize::try_from(at).map_or(chars, |at| at.min(chars));
    let mut spans: Vec<Range<usize>> = ranges
        .iter()
        .map(|range| to_chars(range.start)..to_chars(range.end))
        .filter(|span| span.start < span.end)
        .collect();
    spans.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start <= last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }
    merged
        .into_iter()
        .filter_map(|span| Some(*offsets.get(span.start)?..*offsets.get(span.end)?))
        .collect()
}

/// A text that is safe to print, and the byte ranges of its matches: sorted, apart,
/// and on character boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Marked {
    text: String,
    hits: Vec<Range<usize>>,
}

impl Marked {
    /// `text` from efrd with its `ranges`. [`one_line`] changes one character for one,
    /// so the code points that the ranges count stay where they were.
    fn new(text: &str, ranges: &[TextRange]) -> Marked {
        let text = one_line(text);
        let hits = clamp_ranges(&text, ranges);
        Marked { text, hits }
    }

    /// `text` with no match.
    fn plain(text: &str) -> Marked {
        Marked { text: one_line(text), hits: Vec::new() }
    }

    fn width(&self, method: WidthMethod) -> usize {
        text_width(&self.text, method)
    }

    /// The part that fits in `columns` and `max_bytes`, with `…` at each cut. With
    /// `lead`, a first match that would not show starts the part, after a few code
    /// points of the text before it.
    fn window(&self, columns: usize, max_bytes: usize, lead: bool, method: WidthMethod) -> Marked {
        let text = self.text.as_str();
        let fits = text_width(text, method) <= columns && text.len() <= max_bytes;
        let start = match self.hits.first() {
            Some(first)
                if lead
                    && !fits
                    && (text_width(text.get(..first.end).unwrap_or(text), method) + 1
                        > columns
                        || first.end + CUT.len() > max_bytes) =>
            {
                let before = text.get(..first.start).unwrap_or("");
                let keep = LEAD.min(columns / 4).max(1);
                before.char_indices().rev().nth(keep - 1).map_or(0, |(at, _)| at)
            }
            _ => 0,
        };
        let head = if start > 0 { CUT } else { "" };
        let rest = text.get(start..).unwrap_or("");
        let used = text_width(head, method);
        let end =
            if used + text_width(rest, method) <= columns && head.len() + rest.len() <= max_bytes {
                text.len()
            } else {
                let room = columns.saturating_sub(used + 1);
                let room_bytes = max_bytes.saturating_sub(head.len() + CUT.len());
                let mut width = 0;
                let mut end = start;
                for piece in pieces(rest, method) {
                    let piece_width = text_width(piece, method);
                    if width + piece_width > room || end - start + piece.len() > room_bytes {
                        break;
                    }
                    width += piece_width;
                    end += piece.len();
                }
                end
            };
        let tail = if end < text.len() { CUT } else { "" };
        let shift = |at: usize| at - start + head.len();
        let hits = self
            .hits
            .iter()
            .map(|hit| hit.start.max(start)..hit.end.min(end))
            .filter(|hit| hit.start < hit.end)
            .map(|hit| shift(hit.start)..shift(hit.end))
            .collect();
        Marked { text: format!("{head}{}{tail}", text.get(start..end).unwrap_or("")), hits }
    }

    /// The pieces of the text in order, each with its tone: the matches in `accent`,
    /// the rest in `base`.
    fn spans(&self, base: Tone) -> Vec<(&str, Tone)> {
        let mut spans = Vec::with_capacity(self.hits.len() * 2 + 1);
        let mut at = 0;
        for hit in &self.hits {
            if let (Some(before), Some(matched)) =
                (self.text.get(at..hit.start), self.text.get(hit.clone()))
            {
                spans.push((before, base));
                spans.push((matched, Tone::Accent));
                at = hit.end;
            }
        }
        spans.push((self.text.get(at..).unwrap_or(""), base));
        spans.retain(|(text, _)| !text.is_empty());
        spans
    }

    /// The text painted: the matches in `accent`, the rest in `base`.
    fn paint(&self, base: Tone, options: &RenderOptions) -> String {
        self.spans(base).into_iter().map(|(text, tone)| paint(text, tone, options)).collect()
    }

    /// [`paint`](Self::paint) in at most `budget` bytes, colours included: the text is
    /// cut, with `…` at the cut, where the next piece would not fit.
    fn paint_within(&self, base: Tone, options: &RenderOptions, budget: usize) -> String {
        let whole = self.paint(base, options);
        if whole.len() <= budget {
            return whole;
        }
        let mut out = String::new();
        if budget < CUT.len() {
            return out;
        }
        for (text, tone) in self.spans(base) {
            let painted = paint(text, tone, options);
            if out.len() + painted.len() + CUT.len() <= budget {
                out.push_str(&painted);
                continue;
            }
            let colours = paint("x", tone, options).len().saturating_sub(1);
            let room = budget.saturating_sub(out.len() + colours + CUT.len());
            let cut = floor(text, room);
            if !cut.is_empty() {
                out.push_str(&paint(cut, tone, options));
            }
            out.push_str(CUT);
            break;
        }
        out
    }
}

/// The pieces that a terminal that counts by `method` gives a width each: grapheme
/// clusters, or code points.
fn pieces(text: &str, method: WidthMethod) -> Vec<&str> {
    match method {
        WidthMethod::Grapheme => text.graphemes(true).collect(),
        _ => text.split_inclusive(|_| true).collect(),
    }
}

/// The longest start of `text` of at most `bytes` bytes that ends on a character.
fn floor(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or("")
}

/// The note on how far the index is and why the search stopped early, such as
/// `indexing: 40%`; `None` when the index is complete and the search saw every match.
fn note(partial: Option<SearchPartial>, index: Option<SearchIndexState>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(index) = index {
        parts.push(format!("indexing: {}%", index.percent.min(100)));
    }
    if let Some(partial) = partial {
        parts.push(
            match partial {
                SearchPartial::RowCap => "partial: row cap",
                SearchPartial::TimeCap => "partial: time cap",
                _ => "partial",
            }
            .to_owned(),
        );
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// The first line of each list that fzf shows, which `--header-lines=1` keeps as the
/// header: an empty id, then the keys and, when there is one, a muted `note`, such as
/// `indexing: 40%` or why the search failed.
pub(crate) fn picker_header(note: Option<&str>, options: &RenderOptions) -> String {
    let mut line = format!("\t{}", keys(&KEYS, options));
    if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
        let note = Marked::plain(note.trim()).window(
            PICKER_SNIPPET,
            TEXT_BYTES,
            false,
            options.width_method(),
        );
        line.push_str("   ");
        line.push_str(&note.paint(Tone::Dim, options));
    }
    line.push('\n');
    line
}

/// The cells of one row of a search, before the columns are padded.
struct Cells {
    title: Marked,
    scope: Marked,
    age: String,
    turns: String,
    holder: Marked,
    short_id: String,
    snippet: Option<Marked>,
}

impl Cells {
    fn of(row: &SearchRow, now: Timestamp) -> Cells {
        let Row { scope, age, turns, holder, .. } = Row::of(&row.conversation, now);
        let title = match &row.conversation.summary.title {
            Some(title) => Marked::new(title, &row.title_ranges),
            None => Marked::plain(NO_TITLE),
        };
        let snippet = row
            .hit
            .as_ref()
            .and_then(|hit| hit.lines.first())
            .map(|line| Marked::new(&line.text, &line.ranges))
            .filter(|line| !line.text.trim().is_empty());
        Cells {
            title,
            scope: Marked::plain(&scope),
            age,
            turns,
            holder: Marked::plain(&holder),
            short_id: one_line(&row.short_id),
            snippet,
        }
    }

    /// The cells cut for a row of the picker.
    fn for_picker(self, method: WidthMethod) -> Cells {
        let cell = |marked: Marked, columns| marked.window(columns, CELL_BYTES, false, method);
        Cells {
            title: self.title.window(PICKER_TITLE, TEXT_BYTES, false, method),
            scope: cell(self.scope, PICKER_CELL),
            holder: cell(self.holder, PICKER_CELL),
            snippet: self.snippet.map(|line| line.window(PICKER_SNIPPET, TEXT_BYTES, true, method)),
            ..self
        }
    }
}

/// The widest cell of `cells` that `pick` chooses.
fn widest(cells: &[Cells], pick: impl Fn(&Cells) -> usize) -> usize {
    cells.iter().map(pick).max().unwrap_or(0)
}

/// What `efr search --picker` prints for fzf: the header, then one row per
/// conversation, best first: its id, a tab, then the title, the scope, the age, the
/// turns, the terminal that holds it and the snippet of the best match, in columns.
/// fzf shows the part after the tab. A row takes at most [`ROW_BYTES`].
pub(crate) fn picker_rows(
    result: &ConversationsSearchResult,
    now: Timestamp,
    options: &RenderOptions,
) -> String {
    let mut out = picker_header(note(result.partial, result.index).as_deref(), options);
    let method = options.width_method();
    let cells: Vec<Cells> =
        result.rows.iter().map(|row| Cells::of(row, now).for_picker(method)).collect();
    let title_w = widest(&cells, |cells| cells.title.width(method));
    let scope_w = widest(&cells, |cells| cells.scope.width(method));
    let age_w = widest(&cells, |cells| text_width(&cells.age, method));
    let turns_w = widest(&cells, |cells| text_width(&cells.turns, method));
    let holder_w = widest(&cells, |cells| cells.holder.width(method));
    for (row, cells) in result.rows.iter().zip(&cells) {
        let mut line = format!("{}\t", row.conversation.summary.id);
        line.push_str(&cells.title.paint_within(Tone::Plain, options, TITLE_BYTES));
        line.push_str(&" ".repeat(title_w.saturating_sub(cells.title.width(method))));
        let _ = write!(
            line,
            "  {}  {}  {}",
            pad(&cells.scope.text, scope_w, method),
            pad(&cells.age, age_w, method),
            pad_left(&cells.turns, turns_w, method),
        );
        if holder_w > 0 {
            line.push_str("  ");
            line.push_str(&paint(&cells.holder.text, Tone::Dim, options));
            line.push_str(&" ".repeat(holder_w.saturating_sub(cells.holder.width(method))));
        }
        match &cells.snippet {
            Some(snippet) => {
                line.push_str("  ");
                let budget = ROW_BYTES.saturating_sub(line.len() + 1);
                line.push_str(&snippet.paint_within(Tone::Dim, options, budget));
            }
            None => line.truncate(line.trim_end().len()),
        }
        line.push('\n');
        out.push_str(&line);
    }
    out
}

/// The plain list of `efr search`: one row per conversation, best first, with the
/// cells of the list of `efr resume` and the start of its id that `,resume` takes, and
/// under it a muted line with the best match; then the note on the index, and the line
/// that says how to resume one: `,resume <id>` from the zsh plugin (`plugin`), else
/// `efr resume <id>`. On a terminal the titles and the matches are cut so a row fits;
/// in a pipe nothing is cut and nothing is coloured.
pub(crate) fn list(
    result: &ConversationsSearchResult,
    now: Timestamp,
    plugin: bool,
    options: &RenderOptions,
) -> String {
    let mut out = String::new();
    let shown = columns(options);
    let method = shown.map_or(WidthMethod::CodePoint, |(_, method)| method);
    if result.rows.is_empty() {
        out.push_str("no conversation matches\n");
    }
    let cells: Vec<Cells> = result.rows.iter().map(|row| Cells::of(row, now)).collect();
    let scope_w = widest(&cells, |cells| cells.scope.width(method));
    let age_w = widest(&cells, |cells| text_width(&cells.age, method));
    let turns_w = widest(&cells, |cells| text_width(&cells.turns, method));
    let holder_w = widest(&cells, |cells| cells.holder.width(method));
    let id_w = widest(&cells, |cells| text_width(&cells.short_id, method));
    // The gaps between the columns, and the holder after three blanks.
    let fixed = 2 + scope_w + 2 + age_w + 2 + turns_w + holder_w.min(1) * (3 + holder_w) + 3 + id_w;
    let longest = widest(&cells, |cells| cells.title.width(method));
    let title_w = match shown {
        Some((cols, _)) => longest.min(cols.saturating_sub(fixed).max(MIN_TITLE)),
        None => longest,
    };
    for cells in &cells {
        let title = match shown {
            Some(_) => cells.title.window(title_w, usize::MAX, false, method),
            None => cells.title.clone(),
        };
        let mut line = title.paint(Tone::Plain, options);
        line.push_str(&" ".repeat(title_w.saturating_sub(title.width(method))));
        let _ = write!(
            line,
            "  {}  {}  {}",
            pad(&cells.scope.text, scope_w, method),
            pad(&cells.age, age_w, method),
            pad_left(&cells.turns, turns_w, method),
        );
        if holder_w > 0 {
            line.push_str("   ");
            line.push_str(&paint(&cells.holder.text, Tone::Dim, options));
            line.push_str(&" ".repeat(holder_w.saturating_sub(cells.holder.width(method))));
        }
        let _ = writeln!(line, "   {}", cells.short_id);
        out.push_str(&line);
        if let Some(snippet) = &cells.snippet {
            let snippet = match shown {
                Some((cols, _)) => {
                    snippet.window(cols.saturating_sub(INDENT.len()), usize::MAX, true, method)
                }
                None => snippet.clone(),
            };
            let _ = writeln!(out, "{INDENT}{}", snippet.paint(Tone::Dim, options));
        }
    }
    if let Some(note) = note(result.partial, result.index) {
        let _ = writeln!(out, "{}", paint(&note, Tone::Dim, options));
    }
    if !result.rows.is_empty() {
        let how = if plugin { ",resume <id>" } else { "efr resume <id>" };
        let line = format!("{how} continues one here");
        let _ = writeln!(out, "{}", paint(&line, Tone::Dim, options));
    }
    out
}

/// The word for what the text of a match is.
fn kind_word(kind: SearchKind) -> &'static str {
    match kind {
        SearchKind::Prompt => "prompt",
        SearchKind::Answer => "answer",
        SearchKind::Command => "command",
        SearchKind::Title => "title",
        _ => "text",
    }
}

/// What `efr search --preview` prints for fzf's preview window, `width` columns wide:
/// the title, then each match, newest first, under a muted line with what it is and
/// its age, each line cut to the width. `query` is the query of the search: without
/// it, the newest prompts and the start of the newest answer show with no match.
pub(crate) fn preview(
    result: &ConversationSearchResult,
    query: &str,
    now: Timestamp,
    width: usize,
    options: &RenderOptions,
) -> String {
    let method = options.width_method();
    let mut out = match &result.title {
        Some(title) => Marked::new(&title.text, &title.ranges)
            .window(width, usize::MAX, true, method)
            .paint(Tone::Bold, options),
        None => paint(NO_TITLE, Tone::Dim, options),
    };
    out.push('\n');
    for hit in &result.hits {
        let head = format!("{} \u{b7} {}", kind_word(hit.kind), age(hit.at, now));
        let _ = write!(out, "\n{}\n", paint(&head, Tone::Dim, options));
        for line in &hit.lines {
            let line =
                Marked::new(&line.text, &line.ranges).window(width, usize::MAX, true, method);
            let _ = writeln!(out, "{}", line.paint(Tone::Plain, options));
        }
    }
    if result.hits.is_empty() && !query.trim().is_empty() {
        let line = "no prompt, answer or command matches";
        let _ = write!(out, "\n{}\n", paint(line, Tone::Dim, options));
    }
    if let Some(note) = note(result.partial, None) {
        let _ = write!(out, "\n{}\n", paint(&note, Tone::Dim, options));
    }
    out
}

/// What the preview shows when the search failed: `message` as one muted line.
pub(crate) fn preview_failure(message: &str, width: usize, options: &RenderOptions) -> String {
    let line =
        Marked::plain(message.trim()).window(width, usize::MAX, false, options.width_method());
    format!("{}\n", line.paint(Tone::Dim, options))
}

#[cfg(test)]
mod tests;
