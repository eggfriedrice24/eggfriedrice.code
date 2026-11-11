//! The reasoning of the model on a terminal. While the model thinks, the status row
//! says `thinking`, with the bold title of the reasoning when it has one. A reasoning
//! without a title, such as the reasoning of a Claude model, puts its newest words in
//! that place ([`Reasoning::words`]), by the rules of `newest`: whole words in at most
//! 50 columns, only when the phase has run for 1 s (the summary of a short phase comes
//! in one piece just before the tool call that follows it, so its words would show for
//! a moment and go), and a start that moves a few words at a time, so the words stand
//! still while they grow. No row of the live zone holds the reasoning:
//! a row that comes and goes moves the status row. When a phase of reasoning ends
//! and it took [`ROW_AFTER`] or more, one row stays in the scrollback, at column 0:
//! `▸ <title>  thought 6s`, the bold title of the reasoning in the `text` role and the
//! rest muted. Without a title, the first words of the reasoning stand in its place,
//! muted and in italic, cut with `…` at about [`FIRST_WORDS`] columns; with no words,
//! the row is `▸ thought 3s`. A shorter phase leaves nothing. Bold and italic are no
//! colour, so they stay with `NO_COLOR`.
//!
//! The view only draws the reasoning: it keeps the start and the end of the phase that
//! runs, and nothing of it goes to the model or to the store from here.
//!
//! The times come from the frames, as those of the status row do (`status`): a phase
//! starts at the first frame that draws it and lasts until the newest frame before it
//! ends. The follow loop draws at least once a tick while the turn runs, so the time
//! is off by less than a tick.

use std::time::Duration;

use efr_render::{RenderOptions, Role, WidthMethod, text_width};
use jiff::Timestamp;

use super::newest::{Growing, floor};
use crate::format;

/// The shortest phase of reasoning that leaves a row in the scrollback.
pub(crate) const ROW_AFTER: Duration = Duration::from_secs(2);

/// The columns of the first words of a reasoning without a title, with the `…`.
const FIRST_WORDS: usize = 58;

/// The columns that the title or the first words keep at least; on a narrower screen
/// the row is only the mark and the time.
const MIN_LABEL: usize = 8;

/// The mark at the start of the row.
const MARK: &str = "\u{25b8}";

/// The first bytes of a phase that the row looks at for its first words.
const HEAD: usize = 512;

/// The reasoning of the followed turn.
#[derive(Debug, Default)]
pub(crate) struct Reasoning {
    /// The phase that runs now. In a box, so the view stays small in the futures that
    /// hold it.
    phase: Option<Box<Phase>>,
}

/// One phase of reasoning: from its first draft to the text, the tool call or the end
/// that follows it.
#[derive(Debug, Default)]
struct Phase {
    /// Its text, from the reasoning of the turn that the drafts of the phase brought,
    /// and the frames that drew it.
    text: Growing,
    /// The first bytes of its text, at most [`HEAD`].
    head: String,
    /// The newest bold title, safe to print.
    title: Option<String>,
}

impl Reasoning {
    /// Takes the reasoning `delta` at byte `offset` of the reasoning of the turn, under
    /// `title`, the newest bold title, already safe to print. A phase starts when none
    /// runs. True when what shows changed.
    pub(crate) fn take(&mut self, offset: u64, delta: &str, title: Option<String>) -> bool {
        let started = self.phase.is_none();
        let phase = self.phase.get_or_insert_with(|| {
            Box::new(Phase { text: Growing::from(offset), ..Phase::default() })
        });
        let fresh = phase.text.take(offset, delta);
        let titled = title.is_some() && title != phase.title;
        if title.is_some() {
            phase.title = title;
        }
        if !fresh.is_empty() {
            phase.keep_head(fresh);
        }
        started || titled || !fresh.is_empty()
    }

    /// True while a phase of reasoning runs.
    pub(crate) fn running(&self) -> bool {
        self.phase.is_some()
    }

    /// The frame at `now` draws the phase that runs.
    pub(crate) fn at(&mut self, now: Timestamp) {
        if let Some(phase) = &mut self.phase {
            phase.text.at(now);
        }
    }

    /// The newest words of the phase that runs, for the status row. `None` for a phase
    /// with a title, which the status row shows in their place, before the first words,
    /// and before the phase has run for [`WORDS_AFTER`](super::newest::WORDS_AFTER).
    pub(crate) fn words(&self, method: WidthMethod) -> Option<String> {
        let phase = self.phase.as_ref()?;
        if phase.title.is_some() {
            return None;
        }
        phase.text.words(|tail| plain(tail, true), method)
    }

    /// Ends the phase that runs. The row it leaves, with its newline, when it took
    /// [`ROW_AFTER`] or more.
    pub(crate) fn end(&mut self, options: &RenderOptions) -> Option<String> {
        let phase = self.phase.take()?;
        let took = phase.text.ran()?;
        (took >= ROW_AFTER).then(|| phase.row(took, options))
    }
}

impl Phase {
    /// Adds `text` to the start that the phase keeps, while it is short.
    fn keep_head(&mut self, text: &str) {
        if self.head.len() < HEAD {
            let room = floor(text, HEAD - self.head.len());
            self.head.push_str(&text[..room]);
        }
    }

    /// The row of a phase that took `took`, at the width of `options`, with its newline.
    fn row(&self, took: Duration, options: &RenderOptions) -> String {
        let method = options.width_method();
        let columns = usize::from(options.width());
        let time = format!("thought {}", format::elapsed(took));
        // The mark and a space, the label, two spaces and the time.
        let room = columns.saturating_sub(2 + 2 + text_width(&time, method));
        let label = if room < MIN_LABEL {
            None
        } else if let Some(title) = &self.title {
            Some(styled(Role::Text, BOLD, &cut(title, room, method), options))
        } else {
            let words = first_words(&plain(&self.head, false), method);
            (!words.is_empty())
                .then(|| styled(Role::Muted, ITALIC, &cut(&words, room, method), options))
        };
        let mut row = match label {
            Some(label) => {
                let mut row = options.paint(Role::Muted, &format!("{MARK} "));
                row.push_str(&label);
                row.push_str(&options.paint(Role::Muted, &format!("  {time}")));
                row
            }
            None => {
                let row = format::cut(&format!("{MARK} {time}"), columns.max(1), method);
                options.paint(Role::Muted, &row)
            }
        };
        row.push('\n');
        row
    }
}

/// The SGR parameter of bold.
const BOLD: &str = "1";

/// The SGR parameter of italic.
const ITALIC: &str = "3";

/// `text` in `role` and with the attribute `extra` (an SGR parameter), then a reset.
/// The attribute stays without colour, as `NO_COLOR` asks.
fn styled(role: Role, extra: &str, text: &str, options: &RenderOptions) -> String {
    if !options.is_terminal() || text.is_empty() {
        return text.to_owned();
    }
    let sgr = options.sgr(role).map_or_else(|| extra.to_owned(), |sgr| format!("{sgr};{extra}"));
    format!("\x1b[{sgr}m{text}\x1b[0m")
}

/// The words of reasoning `text` on one line and safe to print, without the lines of
/// its bold titles (the status row shows the newest one) and without the marks of
/// Markdown. With `live`, a title that is still open on the last line is left out
/// too, so it does not show until the status row takes it.
fn plain(text: &str, live: bool) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        let open = live && lines.peek().is_none() && line.starts_with("**");
        if line.is_empty() || is_title(line) || (open && !line[2..].contains("**")) {
            continue;
        }
        for word in unmarked(line).split_whitespace() {
            let word = word.replace("__", "").replace(['*', '`'], "");
            if word.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format::one_line(&word));
        }
    }
    out
}

/// True for a line that is all bold, such as `**Reading the logs**`: a title.
fn is_title(line: &str) -> bool {
    line.len() > 4 && line.starts_with("**") && line.ends_with("**")
}

/// `line` without the mark of a heading, a quote or a list item at its start.
fn unmarked(line: &str) -> &str {
    let Some((mark, rest)) = line.split_once(char::is_whitespace) else {
        return line;
    };
    let heading = !mark.is_empty() && mark.len() <= 6 && mark.chars().all(|c| c == '#');
    let numbered = mark
        .strip_suffix('.')
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
    if heading || numbered || matches!(mark, ">" | "-" | "+" | "*") {
        rest.trim_start()
    } else {
        line
    }
}

/// The first words of `text` in [`FIRST_WORDS`] columns: whole words before a `…`
/// when they go on, or the text cut at the width when its first word is longer.
fn first_words(text: &str, method: WidthMethod) -> String {
    if text_width(text, method) <= FIRST_WORDS {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for word in text.split(' ') {
        let more = usize::from(!out.is_empty()) + text_width(word, method);
        if used + more + 1 > FIRST_WORDS {
            break;
        }
        used += more;
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        return format::cut(text, FIRST_WORDS, method);
    }
    out.push('\u{2026}');
    out
}

/// `text` cut to `columns` with `…` at the cut, as `format::cut` does, without a space
/// before the `…`.
fn cut(text: &str, columns: usize, method: WidthMethod) -> String {
    let cut = format::cut(text, columns, method);
    match cut.strip_suffix('\u{2026}') {
        Some(kept) if kept.ends_with(char::is_whitespace) => format!("{}\u{2026}", kept.trim_end()),
        _ => cut,
    }
}
