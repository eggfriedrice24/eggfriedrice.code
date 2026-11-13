//! Text that the CLI writes itself: its own styled lines, one-line summaries of tool
//! calls, and the `status` and `history` listings. Markdown goes through `efr-render`;
//! everything here is plain lines.
//!
//! Text that comes from the daemon or the model (titles, summaries, error messages,
//! prompts) goes through [`one_line`] or [`lines`] first, so an escape sequence in it
//! cannot drive the terminal.

use std::borrow::Cow;
use std::time::Duration;

use efr_protocol::{ApprovalDecision, Origin};
use efr_render::{Attrs, ColourMode, RenderOptions, Role, WidthMethod, text_width};
use efr_stdx::text::is_format;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use unicode_segmentation::{Graphemes, UnicodeSegmentation as _};

pub(crate) mod always;
mod call;
pub(crate) mod card;
pub(crate) mod changes;
pub(crate) mod context;
pub(crate) mod context_report;
pub(crate) mod delete;
pub(crate) mod ending;
pub(crate) mod groups;
pub(crate) mod hidden_shell;
pub(crate) mod jobs;
pub(crate) mod patch;
pub(crate) mod resume;
pub(crate) mod route;
pub(crate) mod sandbox;
pub(crate) mod search;
pub(crate) mod status;
pub(crate) mod web;

pub(crate) use call::{
    CallText, CommandRow, EDIT_TOOL, RowEnd, call_text, columns, command_line, command_lines,
    numbered_lines, refused, run_heading, tool_result, wrap_command, wrap_spans,
};
pub(crate) use ending::{CACHE_SHOWN_FROM, usage_line};
pub(crate) use status::{
    StatusView, config_error, config_error_place, conversations, reloaded, status,
};
// NOTE: only tests name these from outside their files.
#[cfg(test)]
pub(crate) use call::{WORD_MARK, WRAP_MARK};
#[cfg(test)]
use call::{push_span, runs};

/// How a piece of one of the CLI's own lines looks. Every tone but `Plain` and `Bold`
/// is a colour role of `efr-render` with the attributes it adds
/// ([`Tone::look`]), so the palette decides its colour; [`paint`] paints it through
/// `RenderOptions::paint_with`. The tones after `Accent` are the ones of the design's
/// legend (`TM-Legend`): a slice paints a mark, a badge or a value with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// As it is: plain fact lines.
    Plain,
    /// Bold (SGR 1): the user's own prompt, the keys of a question.
    Bold,
    /// The `muted` role: notes about the turn.
    Dim,
    /// The `warning` role: something waits for the user, or needs their care.
    Attention,
    /// The `error` role: a failure, such as a failed exit code.
    Failure,
    /// The `success` role: a call that went well, an allowed question.
    Success,
    /// The `code` role: what a call runs.
    Code,
    /// The `accent` role: the mark at the start of a call.
    Accent,
    /// The `text` role: a value next to a muted label, the command of a failed call, the
    /// path of an edit, the sentence of an event line.
    Text,
    /// The `text` role in bold: the title of a conversation, a steer after the model
    /// read it.
    TextBold,
    /// The `text` role in italic: the first words of a thought with no title.
    TextItalic,
    /// The `error` role in bold: the mark of a failed call, `✗ failed`, `efr:` before
    /// an error.
    FailureBold,
    /// The `warning` role in bold: the title of a question and its keys, the `?` of a
    /// command that waits for input.
    AttentionBold,
    /// The `link` role: a page of web search, underlined also without colour.
    Link,
    /// The badge of a question, ` ? `: the `warning` role in reverse video and bold.
    /// Without colour it is bold alone (TM-Questions frame 10: "the badge becomes bold
    /// ?").
    Badge,
}

impl Tone {
    /// The colour role of the tone and the attributes it adds to it, when it has a
    /// role.
    fn look(self) -> Option<(Role, Attrs)> {
        match self {
            Tone::Plain | Tone::Bold => None,
            Tone::Dim => Some((Role::Muted, Attrs::NONE)),
            Tone::Attention => Some((Role::Warning, Attrs::NONE)),
            Tone::Failure => Some((Role::Error, Attrs::NONE)),
            Tone::Success => Some((Role::Success, Attrs::NONE)),
            Tone::Code => Some((Role::Code, Attrs::NONE)),
            Tone::Accent => Some((Role::Accent, Attrs::NONE)),
            Tone::Text => Some((Role::Text, Attrs::NONE)),
            Tone::TextBold => Some((Role::Text, Attrs::BOLD)),
            Tone::TextItalic => Some((Role::Text, Attrs::ITALIC)),
            Tone::FailureBold => Some((Role::Error, Attrs::BOLD)),
            Tone::AttentionBold => Some((Role::Warning, Attrs::BOLD)),
            Tone::Link => Some((Role::Link, Attrs::NONE)),
            Tone::Badge => Some((Role::Warning, Attrs::BADGE)),
        }
    }
}

/// The kinds of blocks a reply or a transcript is made of.
///
/// The kinds up to `Answer` are today's; the last three are the design's
/// (`DECISIONS.md` and the Calls, LongTurn and Endings boards). A slice moves its blocks
/// to the design's kinds when it builds their new look: a call row, an answer line, a
/// notice, a thought row and an event line become `Row`, a steer in scrollback becomes
/// `Steer`, the stop reason, the changes line and the outcome line become `End`, and an
/// assistant message is `Message` in both rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Block {
    /// The user's prompt.
    Prompt,
    /// An assistant message: prose.
    Message,
    /// A dim note: an answer line, a steer, the end of a turn.
    Note,
    /// A tool call: its command, its output's tail and its result.
    Call,
    /// A question: an approval or the quarantine question.
    Question,
    /// The answer that takes the place of a question about a call of this turn, such as
    /// "allowed" or "denied": the rows of the call follow it at once.
    Settled,
    /// Any other answer that takes a question's place.
    Answer,
    /// One row of the design: a call row with what stands under it, an answer line, a
    /// notice, a thought row or an event line. No blank line stands between two rows,
    /// and one between rows and prose.
    Row,
    /// The user's steer after the model read it, `› text`: one blank line above it and
    /// one below.
    Steer,
    /// A line of the end of a turn: the stop reason, the changes line and the outcome
    /// line. One blank line stands before the first, none between them.
    End,
}

impl Block {
    /// True for the kinds of the design's rules.
    fn designed(self) -> bool {
        matches!(self, Block::Row | Block::Steer | Block::End)
    }
}

/// Separates blocks by a blank line, except consecutive notes, which read as one
/// group, the answer right after the question it answers, the call right after the
/// answer that settled it, and the line of what an `a` answer saved right after that
/// answer.
///
/// When either block is of a kind of the design (`Row`, `Steer`, `End`), the design's
/// rules decide instead ([`Spacing::designed`]): no blank line between two rows (today's
/// `Note`, `Call`, `Settled` and `Answer` count as rows there), one between rows and
/// prose, one above and one below a steer, one before the end lines and none between
/// them, none after the prompt line, and one above a question card.
#[derive(Debug, Default)]
pub(crate) struct Spacing {
    last: Option<Block>,
    /// The last block was a row that sticks to the message or the call after it.
    stuck: bool,
}

impl Spacing {
    /// After a row that sticks to the message or the tool call that follows it, such as
    /// the row of a phase of reasoning: that block comes with no blank line, and any
    /// other block with one, as after a message.
    pub(crate) fn stuck() -> Self {
        Spacing { last: Some(Block::Message), stuck: true }
    }

    /// What to write before a block of `kind`.
    pub(crate) fn before(&mut self, kind: Block) -> &'static str {
        let separator = self.peek(kind);
        self.last = Some(kind);
        self.stuck = false;
        separator
    }

    /// True when the last block is a row of the design (a call row with what stands
    /// under it, an answer line, a notice) that sticks to nothing: prose after it gets a
    /// blank line.
    pub(crate) fn after_row(&self) -> bool {
        !self.stuck
            && matches!(
                self.last,
                Some(Block::Note | Block::Call | Block::Settled | Block::Answer | Block::Row)
            )
    }

    /// What [`before`](Self::before) would write before a block of `kind`, for a block
    /// that shows in the live zone before it is written.
    pub(crate) fn peek(&self, kind: Block) -> &'static str {
        if self.stuck && matches!(kind, Block::Message | Block::Call | Block::Row) {
            return "";
        }
        if kind.designed() || self.last.is_some_and(Block::designed) {
            return Spacing::designed(self.last, kind);
        }
        match (self.last, kind) {
            (None, _)
            | (Some(Block::Note), Block::Note)
            | (Some(Block::Question), Block::Settled | Block::Answer)
            | (Some(Block::Settled), Block::Call | Block::Settled) => "",
            _ => "\n",
        }
    }

    /// The design's rule for a block of `kind` after one of `last`.
    fn designed(last: Option<Block>, kind: Block) -> &'static str {
        /// What a block is for the design's rules.
        #[derive(PartialEq)]
        enum Class {
            Prompt,
            Prose,
            Row,
            Steer,
            End,
            Question,
        }
        let class = |block: Block| match block {
            Block::Prompt => Class::Prompt,
            Block::Message => Class::Prose,
            Block::Note | Block::Call | Block::Settled | Block::Answer | Block::Row => Class::Row,
            Block::Steer => Class::Steer,
            Block::End => Class::End,
            Block::Question => Class::Question,
        };
        let Some(last) = last else { return "" };
        match (class(last), class(kind)) {
            (Class::Prompt, _) => "",
            (_, Class::Steer) | (Class::Steer, _) => "\n",
            (Class::End, Class::End) => "",
            (_, Class::End) => "\n",
            (Class::Row, Class::Row) => "",
            // NOTE: the answer takes the place of the card, so it stands where the card
            // stood, after the empty row above it.
            (Class::Question, Class::Row) => "",
            _ => "\n",
        }
    }
}

/// `text` styled as `tone` when the output is a terminal, plain otherwise. The text is
/// written as it is: callers pass text that is already safe.
pub(crate) fn paint(text: &str, tone: Tone, options: &RenderOptions) -> String {
    if !options.is_terminal() || text.is_empty() {
        return text.to_owned();
    }
    match tone.look() {
        // NOTE: reverse video of nothing but the terminal's own colours is a block;
        // without colour the badge is a bold `?`, as the design draws it.
        Some((role, _)) if tone == Tone::Badge && options.colour() == ColourMode::None => {
            options.paint_with(role, Attrs::BOLD, text)
        }
        Some((role, attrs)) => options.paint_with(role, attrs, text),
        // NOTE: bold with no role, so a palette that sets `text` does not colour the
        // user's own prompt.
        None if tone == Tone::Bold => format!("\x1b[1m{text}\x1b[0m"),
        None => text.to_owned(),
    }
}

/// `text` on one line and safe to print: newlines and tabs become spaces, other control
/// characters visible stand-ins.
pub(crate) fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            c => visible(c),
        })
        .collect()
}

/// What starts the line of an approval summary that names the parts of a command line
/// that ask, as the daemon writes it.
const ASKS_FOR: &str = "\nasks for: ";

/// What starts the first line of the summary of a question that full mode still asks,
/// such as a delete of `~`, as the daemon writes it.
pub(crate) const FULL_ASKS: &str = "full mode still asks for this: ";

/// The first line of `summary` when it says what full mode still asks for, and the
/// rest of the summary after it; `None` for any other summary.
pub(crate) fn full_guard(summary: &str) -> Option<(&str, &str)> {
    if !summary.starts_with(FULL_ASKS) {
        return None;
    }
    Some(summary.split_once('\n').unwrap_or((summary, "")))
}

/// An approval summary as one line for the question and, when the daemon named them,
/// the parts of a command line that ask, as a line `asks for: hostnamectl, ...`. Both
/// are safe to print.
///
/// NOTE: the daemon escapes every other newline of a summary, and names a part only
/// with letters, digits, spaces and `._/:@%+,-`. A line with anything else is not
/// split off, so a file name with a newline in an older summary cannot pass for it.
pub(crate) fn approval_summary(summary: &str) -> (String, Option<String>) {
    let (first, asking) = split_summary(summary);
    (one_line(first), asking)
}

/// The summary without the line of the parts that ask, and that line.
fn split_summary(summary: &str) -> (&str, Option<String>) {
    let named = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, ' ' | '.' | '_' | '/' | ':' | '@' | '%' | '+' | ',' | '-')
    };
    match summary.rsplit_once(ASKS_FOR) {
        Some((first, parts)) if !parts.is_empty() && parts.chars().all(named) => {
            (first, Some(format!("asks for: {parts}")))
        }
        _ => (summary, None),
    }
}

/// What an approval question shows of the daemon's summary, as [`approval_summary`],
/// but with the lines of a command of several lines each on its own: `call` is the
/// tool and the command of the call, from its `tool_call_started`.
///
/// NOTE: the daemon writes the line as a quoted Rust string (`run "cd src\nls"`). Only
/// a summary with exactly that quote of the call's command as one of its parts gets
/// its lines; what the summary says besides the quote follows on a line of its own
/// after `also:`. The daemon puts the quote first, but an older one put a path first,
/// and `efr history` shows its summaries too. Any other summary shows as before.
pub(crate) fn approval_heading(
    summary: &str,
    call: Option<(&str, &str)>,
) -> (Vec<String>, Option<String>) {
    let (first, asking) = split_summary(summary);
    if let Some((tool, command)) = call
        && command_lines(command).len() > 1
        && let Some(subjects) = first.strip_prefix(&format!("{tool}: "))
        && let Some((before, after)) = around_quote(subjects, &format!("run {command:?}"))
    {
        let mut lines = run_heading(tool, command);
        let rest: Vec<&str> = [before.trim_end_matches("; ").trim(), after.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        if !rest.is_empty() {
            lines.push(format!("also: {}", one_line(&rest.join("; "))));
        }
        return (lines, asking);
    }
    (vec![one_line(first)], asking)
}

/// What `subjects` says before and after `quote`, when `quote` is one of its parts:
/// at the start or after `; `, and at the end or before `;`.
fn around_quote<'a>(subjects: &'a str, quote: &str) -> Option<(&'a str, &'a str)> {
    subjects.match_indices(quote).find_map(|(at, _)| {
        let before = subjects.get(..at)?;
        let after = subjects.get(at + quote.len()..)?;
        let starts = before.is_empty() || before.ends_with("; ");
        let ends = after.is_empty() || after.starts_with(';');
        (starts && ends).then(|| (before, after.trim_start_matches(';')))
    })
}

/// The command of a shell call's input, if it has one.
pub(crate) fn command_of(input: &Value) -> Option<&str> {
    input.get("command").and_then(Value::as_str)
}

/// `text` safe to print as lines: newlines stay, tabs become a space, other control
/// characters and format characters visible stand-ins.
pub(crate) fn lines(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| (c.is_control() && c != '\n') || is_format(c)) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|c| match c {
                '\n' => '\n',
                '\t' => ' ',
                c => visible(c),
            })
            .collect(),
    )
}

/// A diff safe to print as it is, for a pipe: newlines and tabs stay, so a tool that
/// reads the diff can apply it, and other control characters and format characters
/// become visible stand-ins.
/// As [`one_line`], for words that a person or a model wrote, such as the query of a
/// web search: the zero-width joiner and non-joiner stay, because an emoji sequence and
/// the words of scripts such as Persian or Hindi need them, and neither moves the cursor
/// or turns the text around. Every other format character becomes `U+FFFD`.
pub(crate) fn one_line_words(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            '\u{200c}' | '\u{200d}' => c,
            c => visible(c),
        })
        .collect()
}

pub(crate) fn diff_text(text: &str) -> Cow<'_, str> {
    let unsafe_char = |c: char| (c.is_control() && c != '\n' && c != '\t') || is_format(c);
    if !text.chars().any(unsafe_char) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.chars().map(|c| if unsafe_char(c) { visible(c) } else { c }).collect())
}

/// A control character's visible stand-in, the same mapping `efr-render` uses, and
/// `U+FFFD` for a format character, which draws nothing or turns the text around it.
/// A model's reply in markdown keeps format characters, for the scripts that need
/// them.
fn visible(c: char) -> char {
    match c {
        '\u{0}'..='\u{1f}' => char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'),
        '\u{7f}' => '\u{2421}',
        c if c.is_control() || is_format(c) => '\u{fffd}',
        c => c,
    }
}

/// `body` as a fenced markdown code block labelled `info`. The fence is longer than any
/// run of backticks in the body, so the body cannot close it early.
pub(crate) fn code_block(info: &str, body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    let newline = if body.ends_with('\n') { "" } else { "\n" };
    format!("{fence}{info}\n{body}{newline}{fence}\n")
}

/// `text` cut to `columns` with `…` at the cut, as a terminal that counts widths by
/// `method` shows it; whole when it fits. A grapheme cluster is never cut in two.
pub(crate) fn cut(text: &str, columns: usize, method: WidthMethod) -> String {
    if fits(text, columns, method) {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for piece in pieces(text, method) {
        let w = text_width(piece, method);
        if used + w + 1 > columns {
            break;
        }
        out.push_str(piece);
        used += w;
    }
    out.push('\u{2026}');
    out
}

/// The pieces a terminal that counts by `method` gives a width each: code points, or
/// grapheme clusters. They come one at a time, so a caller that stops at a full row
/// does not cut the rest of the text.
fn pieces(text: &str, method: WidthMethod) -> Pieces<'_> {
    match method {
        WidthMethod::Grapheme => Pieces::Clusters(text.graphemes(true)),
        _ => Pieces::CodePoints(text),
    }
}

/// The iterator of [`pieces`].
enum Pieces<'a> {
    /// The text that is left, cut one code point at a time.
    CodePoints(&'a str),
    Clusters(Graphemes<'a>),
}

impl<'a> Iterator for Pieces<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        match self {
            Pieces::CodePoints(left) => {
                let c = left.chars().next()?;
                let (piece, after) = left.split_at(c.len_utf8());
                *left = after;
                Some(piece)
            }
            Pieces::Clusters(clusters) => clusters.next(),
        }
    }
}

/// True when `text` takes at most `columns` columns on a terminal that counts by
/// `method`. The count stops at the first piece past them, so a long text costs only
/// the start of it.
fn fits(text: &str, columns: usize, method: WidthMethod) -> bool {
    let mut used = 0;
    for piece in pieces(text, method) {
        used += text_width(piece, method);
        if used > columns {
            return false;
        }
    }
    true
}

/// The answer to an approval in a word.
pub(crate) fn decision(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Allow => "allowed",
        ApprovalDecision::AllowAlways => "allowed always",
        ApprovalDecision::Deny => "denied",
        _ => "answered",
    }
}

/// True when the answer runs the call: a "yes", also with an always rule.
pub(crate) fn allows(decision: ApprovalDecision) -> bool {
    matches!(decision, ApprovalDecision::Allow | ApprovalDecision::AllowAlways)
}

/// The surface that did something, as the user knows it.
pub(crate) fn origin(origin: Origin) -> &'static str {
    match origin {
        Origin::Shell => "the shell",
        Origin::Cli => "efr",
        Origin::Proxy => "a terminal proxy",
        Origin::Phone => "the phone",
        _ => "another client",
    }
}

/// How long ago `then` was at `now`, such as `5m ago`; `just now` under a second.
pub(crate) fn ago(then: Timestamp, now: Timestamp) -> String {
    let seconds = now.duration_since(then).as_secs();
    if seconds < 1 {
        return "just now".to_owned();
    }
    format!("{} ago", span(seconds.unsigned_abs()))
}

/// How long until `then` from `now`, such as `in 52m`, or how long ago it passed.
pub(crate) fn until(then: Timestamp, now: Timestamp) -> String {
    let seconds = then.duration_since(now).as_secs();
    if seconds < 0 { ago(then, now) } else { format!("in {}", span(seconds.unsigned_abs())) }
}

/// A duration in its two largest units: `45s`, `5m 3s`, `2h 5m`, `3d 4h`.
pub(crate) fn span(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    let secs = seconds % 60;
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, _) => format!("{minutes}m {secs}s"),
        (0, _, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

/// A running time in whole seconds, as the status row counts it: `0s` to `59s`, then
/// `1m 00s`, then `1h 00m 00s`.
pub(crate) fn elapsed(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, secs) = (seconds / 3_600, seconds / 60 % 60, seconds % 60);
    match (hours, minutes) {
        (0, 0) => format!("{secs}s"),
        (0, _) => format!("{minutes}m {secs:02}s"),
        _ => format!("{hours}h {minutes:02}m {secs:02}s"),
    }
}

/// How long a turn took: tenths of a second under 10 s, such as `4.2s`, else as
/// [`elapsed`] counts.
pub(crate) fn took(time: Duration) -> String {
    if time < Duration::from_secs(10) {
        let tenths = time.as_millis() / 100;
        return format!("{}.{}s", tenths / 10, tenths % 10);
    }
    elapsed(time)
}

/// A count of tokens: as it is under 1000, then `1.2k`, `18.2k`, `120k`, `1.2M`. The
/// digits are cut, never rounded up, so `999999` is `999k`, not `1000k`.
pub(crate) fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..100_000 => format!("{}.{}k", count / 1_000, count / 100 % 10),
        100_000..1_000_000 => format!("{}k", count / 1_000),
        _ => format!("{}.{}M", count / 1_000_000, count / 100_000 % 10),
    }
}

/// A size in bytes: `512 B`, `3.2 KB`, `1.4 MB`, `2.1 GB`, counted in thousands.
pub(crate) fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{}.{} KB", bytes / 1_000, bytes / 100 % 10),
        1_000_000..1_000_000_000 => format!("{}.{} MB", bytes / 1_000_000, bytes / 100_000 % 10),
        _ => format!("{}.{} GB", bytes / 1_000_000_000, bytes / 100_000_000 % 10),
    }
}

/// `at` in `zone`, to the second, as a person reads it: `2026-10-20 12:00:00 CEST`.
/// A zone that jiff cannot name gives the time in UTC: `2026-10-20T10:00:00Z`.
pub(crate) fn local_time(at: Timestamp, zone: &TimeZone) -> String {
    let zoned = at.to_zoned(zone.clone());
    jiff::fmt::strtime::format("%Y-%m-%d %H:%M:%S %Z", &zoned)
        .unwrap_or_else(|_| format!("{at:.0}"))
}

#[cfg(test)]
mod tests;
