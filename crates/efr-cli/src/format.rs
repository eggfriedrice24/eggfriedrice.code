//! Text that the CLI writes itself: its own styled lines, one-line summaries of tool
//! calls, and the `status` and `history` listings. Markdown goes through `efr-render`;
//! everything here is plain lines.
//!
//! Text that comes from the daemon or the model (titles, summaries, error messages,
//! prompts) goes through [`one_line`] or [`lines`] first, so an escape sequence in it
//! cannot drive the terminal.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use efr_protocol::{
    AdminConfigReloadResult, AdminStatusResult, ApprovalDecision, ConfigFileError, ContextUse,
    ConversationStatus, ConversationsListResult, EffectiveSettings, Origin, Usage,
};
use efr_render::{RenderOptions, Role, WidthMethod, text_width};
use efr_stdx::text::is_format;
use jiff::Timestamp;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation as _;

pub(crate) mod card;
pub(crate) mod changes;
pub(crate) mod context;
pub(crate) mod patch;
pub(crate) mod sandbox;

/// Keys of a tool's input that best describe a call in one line, in order of
/// preference.
const DETAIL_KEYS: &[&str] = &["command", "cmd", "path", "file", "url", "query"];

/// How a piece of one of the CLI's own lines looks. Every tone but `Plain` and `Bold`
/// is a colour role of `efr-render`, so the palette decides its colour.
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
}

impl Tone {
    /// The colour role of the tone, when it has one.
    fn role(self) -> Option<Role> {
        match self {
            Tone::Plain | Tone::Bold => None,
            Tone::Dim => Some(Role::Muted),
            Tone::Attention => Some(Role::Warning),
            Tone::Failure => Some(Role::Error),
            Tone::Success => Some(Role::Success),
            Tone::Code => Some(Role::Code),
            Tone::Accent => Some(Role::Accent),
        }
    }
}

/// The kinds of blocks a reply or a transcript is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Block {
    /// The user's prompt.
    Prompt,
    /// An assistant message.
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
}

/// Separates blocks by a blank line, except consecutive notes, which read as one
/// group, the answer right after the question it answers, and the call right after the
/// answer that settled it.
#[derive(Debug, Default)]
pub(crate) struct Spacing {
    last: Option<Block>,
}

impl Spacing {
    /// What to write before a block of `kind`.
    pub(crate) fn before(&mut self, kind: Block) -> &'static str {
        let separator = self.peek(kind);
        self.last = Some(kind);
        separator
    }

    /// What [`before`](Self::before) would write before a block of `kind`, for a block
    /// that shows in the live zone before it is written.
    pub(crate) fn peek(&self, kind: Block) -> &'static str {
        match (self.last, kind) {
            (None, _)
            | (Some(Block::Note), Block::Note)
            | (Some(Block::Question), Block::Settled | Block::Answer)
            | (Some(Block::Settled), Block::Call) => "",
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
    match tone.role() {
        Some(role) => options.paint(role, text),
        None if tone == Tone::Bold => format!("\x1b[1m{text}\x1b[0m"),
        None => text.to_owned(),
    }
}

/// The line under a question that says which key does what, such as `y allow · n
/// deny`: muted, with each key in bold.
pub(crate) fn keys(pairs: &[(&str, &str)], options: &RenderOptions) -> String {
    let mut out = String::new();
    for (at, (key, what)) in pairs.iter().enumerate() {
        if at > 0 {
            out.push_str(&paint(" \u{b7} ", Tone::Dim, options));
        }
        out.push_str(&paint(key, Tone::Bold, options));
        out.push_str(&paint(&format!(" {what}"), Tone::Dim, options));
    }
    out
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

/// A tool call in one line, such as `$ ls -la`, `read /etc/hosts` or `apply_patch
/// src/a.rs +3 −1, src/b.rs +1`.
///
/// A command of several lines shows its first line and how many lines follow, such as
/// `$ cd src (and 2 more lines)`: lines joined by spaces would look like one command
/// with more arguments. With `columns` (and the terminal's way to count widths), the
/// line is cut to fit that width, but never the count of the lines that follow.
pub(crate) fn tool_call(
    tool: &str,
    input: &Value,
    columns: Option<(usize, WidthMethod)>,
) -> String {
    call_line(tool, input).fit(columns, 0)
}

/// The width a line is cut to on a terminal, with the terminal's way to count widths;
/// none when the output is not a terminal.
pub(crate) fn columns(options: &RenderOptions) -> Option<(usize, WidthMethod)> {
    options.is_terminal().then(|| (usize::from(options.width()), options.width_method()))
}

/// The words that name a call of `tool`: `$` for a shell call, `read` and `write` for
/// the file tools, `settings` and `apply_patch` for those tools, else the tool's name
/// and a colon.
fn call_name(tool: &str) -> String {
    match tool {
        "shell" => "$".to_owned(),
        "read_file" => "read".to_owned(),
        "write_file" => "write".to_owned(),
        "settings" => "settings".to_owned(),
        patch::TOOL => patch::TOOL.to_owned(),
        other => format!("{}:", one_line(other)),
    }
}

/// What a call of `tool` with `input` does, before it is split into lines: the files of
/// an `apply_patch` call with their counts, else the first of [`DETAIL_KEYS`] that the
/// input has, else the input as JSON.
fn call_detail(tool: &str, input: &Value) -> String {
    if tool == patch::TOOL
        && let Some(text) = patch::text(input)
    {
        return patch::summary(&patch::files(text));
    }
    match input {
        Value::Object(members) => DETAIL_KEYS
            .iter()
            .find_map(|key| members.get(*key).and_then(Value::as_str))
            .map_or_else(|| input.to_string(), str::to_owned),
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A tool call's line before it is cut to a width: the name and the first line of
/// what it does, and how many lines of a command follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallLine {
    head: String,
    more: String,
}

/// The line of a call of `tool` with `input`, such as `$ cargo test -p app`.
pub(crate) fn call_line(tool: &str, input: &Value) -> CallLine {
    let detail = call_detail(tool, input);
    let name = call_name(tool);
    let lines = command_lines(&detail);
    let (first, more) = match lines.split_first() {
        Some((first, rest)) => (command_line(first), rest.len()),
        None => (String::new(), 0),
    };
    let head = if first.is_empty() {
        name.trim_end_matches(':').to_owned()
    } else {
        format!("{name} {first}")
    };
    let more = match more {
        0 => String::new(),
        1 => " (and 1 more line)".to_owned(),
        more => format!(" (and {more} more lines)"),
    };
    CallLine { head, more }
}

impl CallLine {
    /// The line, cut to `columns` less `reserve` columns that follow it on the same row;
    /// whole without `columns`. The count of the lines that follow is never cut.
    pub(crate) fn fit(&self, columns: Option<(usize, WidthMethod)>, reserve: usize) -> String {
        match columns {
            Some((columns, method)) => {
                let room = columns.saturating_sub(reserve + text_width(&self.more, method));
                format!("{}{}", cut(&self.head, room, method), self.more)
            }
            None => format!("{}{}", self.head, self.more),
        }
    }
}

/// What a call does, for the block of a call: the words that name it (`$`, `read`,
/// `write`, `settings`, `apply_patch`, else the tool and a colon) and each line of what
/// it does, safe to print. Nothing of it is ever left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallText {
    pub(crate) name: String,
    pub(crate) lines: Vec<String>,
    /// The entries of a list that `lines` joins with `, `, such as the files of an
    /// `apply_patch` call: a row of the call's block is cut only between two of them,
    /// and with no mark, because the list is no command. Empty for other calls.
    pub(crate) entries: Vec<String>,
}

/// The text of a call of `tool` with `input`, such as `$` and `cargo test -p app`.
pub(crate) fn call_text(tool: &str, input: &Value) -> CallText {
    let detail = call_detail(tool, input);
    let lines = command_lines(&detail).into_iter().map(command_line).collect();
    let entries = match patch::text(input).filter(|_| tool == patch::TOOL) {
        Some(text) => patch::entries(&patch::files(text)),
        None => Vec::new(),
    };
    CallText { name: call_name(tool).trim_end_matches(':').to_owned(), lines, entries }
}

/// What ends a row of a command that is cut after a space. The rows after it are
/// indented: the cut falls between two words, so the indent reads as the space that is
/// there. The mark is muted and is not part of the command.
pub(crate) const WRAP_MARK: &str = "\\";

/// What ends a row of a command that is cut inside a word, because no space fits in
/// the row. The next row goes on at the same column as this one, so no space shows
/// where the command has none. The mark is muted, is not part of the command, and is
/// no shell syntax.
pub(crate) const WORD_MARK: &str = "\u{21a9}";

/// How a row of a command ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowEnd {
    /// The line ends in this row.
    Last,
    /// The row is cut after a space, and [`WRAP_MARK`] follows.
    Space,
    /// The row is cut inside a word, and [`WORD_MARK`] follows.
    Word,
}

/// One row of a line of a command, as [`wrap_command`] cuts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandRow {
    pub(crate) text: String,
    /// The row comes after a cut at a space, so it is indented.
    pub(crate) indented: bool,
    pub(crate) end: RowEnd,
}

impl CommandRow {
    /// The mark after the row, if any.
    pub(crate) fn mark(&self) -> Option<&'static str> {
        match self.end {
            RowEnd::Last => None,
            RowEnd::Space => Some(WRAP_MARK),
            RowEnd::Word => Some(WORD_MARK),
        }
    }
}

/// The rows of `line`, one line of a command that is safe to print, as a terminal that
/// counts widths by `method` shows them. A row has `room` columns, less `indent` from
/// the first cut at a space on, because the rows from there are indented. A row is cut
/// after the last space that fits when that fills at least half of the row; else it is
/// cut inside a word, and the next row goes on at the same column. Its text and its
/// mark fit the row. Nothing of the line is left out.
pub(crate) fn wrap_command(
    line: &str,
    room: usize,
    indent: usize,
    method: WidthMethod,
) -> Vec<CommandRow> {
    let mark = text_width(WRAP_MARK, method).max(text_width(WORD_MARK, method));
    let mut rows = Vec::new();
    let mut left = line;
    let mut indented = false;
    loop {
        let here = if indented { room.saturating_sub(indent) } else { room }.max(1);
        if text_width(left, method) <= here {
            rows.push(CommandRow { text: left.to_owned(), indented, end: RowEnd::Last });
            return rows;
        }
        let budget = here.saturating_sub(mark);
        let (mut end, mut used, mut after_space, mut word) = (0, 0, None, false);
        for piece in pieces(left, method) {
            let width = text_width(piece, method);
            // One piece at least, so every row takes something.
            if end > 0 && used + width > budget {
                break;
            }
            end += piece.len();
            used += width;
            if piece == " " {
                // NOTE: only a space after a word cuts: a row of spaces alone says
                // nothing.
                if word {
                    after_space = Some(end);
                }
            } else {
                word = true;
            }
        }
        // NOTE: a cut at a space early in the row would leave a short first word alone,
        // such as `cp \`, with the rest of the row empty: a space cuts only a row that
        // is at least half full, or one that the word after it cannot reach.
        let (cut, kind) = match after_space {
            Some(at) if at == end || text_width(&left[..at], method) * 2 >= here => {
                (at, RowEnd::Space)
            }
            _ => (end, RowEnd::Word),
        };
        if cut >= left.len() {
            rows.push(CommandRow { text: left.to_owned(), indented, end: RowEnd::Last });
            return rows;
        }
        let (row, after) = left.split_at(cut);
        rows.push(CommandRow { text: row.to_owned(), indented, end: kind });
        left = after;
        indented |= kind == RowEnd::Space;
    }
}

/// The rows of `spans`, pieces of text with their tone, at `first` columns for the
/// first row and `rest` for each row after it: cut at spaces, and inside a word only
/// when the word alone is wider than a row. The spaces at a cut go. Nothing else of
/// the text is left out.
pub(crate) fn wrap_spans(
    spans: &[(String, Tone)],
    first: usize,
    rest: usize,
    method: WidthMethod,
) -> Vec<Vec<(String, Tone)>> {
    let mut rows: Vec<Vec<(String, Tone)>> = vec![Vec::new()];
    let mut used = 0;
    let mut room = first;
    for (text, tone) in spans {
        for token in runs(text) {
            let width = text_width(token, method);
            let space = token.starts_with(' ');
            if space && used == 0 && rows.len() > 1 {
                continue;
            }
            if used + width <= room {
                push_span(&mut rows, token, *tone);
                used += width;
                continue;
            }
            // NOTE: a row with nothing in it yet takes the word that does not fit, cut
            // to its room, so no row stays empty.
            if used > 0 {
                rows.push(Vec::new());
                used = 0;
                room = rest;
            }
            if space {
                continue;
            }
            let mut left = token;
            while text_width(left, method) > room {
                let mut end = 0;
                let mut taken = 0;
                for piece in pieces(left, method) {
                    let width = text_width(piece, method);
                    if end > 0 && taken + width > room {
                        break;
                    }
                    end += piece.len();
                    taken += width;
                }
                let (row, after) = left.split_at(end);
                push_span(&mut rows, row, *tone);
                if after.is_empty() {
                    break;
                }
                rows.push(Vec::new());
                room = rest;
                left = after;
            }
            push_span(&mut rows, left, *tone);
            used = text_width(left, method);
        }
    }
    for row in &mut rows {
        if let Some((text, _)) = row.last_mut() {
            let kept = text.trim_end_matches(' ').len();
            text.truncate(kept);
        }
        row.retain(|(text, _)| !text.is_empty());
    }
    while rows.len() > 1 && rows.last().is_some_and(Vec::is_empty) {
        rows.pop();
    }
    rows
}

/// `text` as runs of spaces and runs of other characters.
fn runs(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (at, c) in text.char_indices() {
        let is_space = c == ' ';
        if space.is_some_and(|was| was != is_space) {
            tokens.push(&text[start..at]);
            start = at;
        }
        space = Some(is_space);
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

/// Adds `text` in `tone` to the last row, joined with a piece of the same tone.
fn push_span(rows: &mut [Vec<(String, Tone)>], text: &str, tone: Tone) {
    if text.is_empty() {
        return;
    }
    let Some(row) = rows.last_mut() else { return };
    match row.last_mut() {
        Some((last, last_tone)) if *last_tone == tone => last.push_str(text),
        _ => row.push((text.to_owned(), tone)),
    }
}

/// The lines of a command, split at each newline, without the blank lines at its start
/// and its end, which run nothing. A command of only blanks has none.
pub(crate) fn command_lines(command: &str) -> Vec<&str> {
    let lines: Vec<&str> = command.split('\n').collect();
    let blank = |line: &&str| line.trim().is_empty();
    match (lines.iter().position(|line| !blank(line)), lines.iter().rposition(|line| !blank(line)))
    {
        (Some(start), Some(end)) => lines[start..=end].to_vec(),
        _ => Vec::new(),
    }
}

/// One line of a command, safe to print: a tab becomes a space, and every other control
/// character, a carriage return too, a visible stand-in, because the shell does not
/// read a carriage return as a space.
pub(crate) fn command_line(line: &str) -> String {
    line.chars().map(|c| if c == '\t' { ' ' } else { visible(c) }).collect()
}

/// Each line of a command of several lines, numbered and after `indent`, so the start
/// of each line shows also where a terminal wraps a long one; `None` for a command of
/// one line.
pub(crate) fn numbered_lines(command: &str, indent: &str) -> Option<Vec<String>> {
    let lines = command_lines(command);
    if lines.len() < 2 {
        return None;
    }
    let digits = lines.len().to_string().len();
    let numbered = lines
        .iter()
        .enumerate()
        .map(|(at, line)| {
            let number = at + 1;
            format!("{indent}{number:>digits$}  {}", command_line(line)).trim_end().to_owned()
        })
        .collect();
    Some(numbered)
}

/// The heading of a question about `tool` running `command`: `shell: run "ls -la"`, or
/// for a command of several lines `shell: run 2 lines:` and then each line on its own,
/// numbered. The whole command shows: the user approves exactly what runs.
pub(crate) fn run_heading(tool: &str, command: &str) -> Vec<String> {
    let tool = one_line(tool);
    match numbered_lines(command, "  ") {
        Some(numbered) => {
            let mut lines = vec![format!("{tool}: run {} lines:", numbered.len())];
            lines.extend(numbered);
            lines
        }
        None => {
            let line = command_lines(command)
                .first()
                .map_or_else(String::new, |line| command_line(line.trim_end_matches('\r')));
            vec![format!("{tool}: run \"{line}\"")]
        }
    }
}

/// `text` cut to `columns` with `…` at the cut, as a terminal that counts widths by
/// `method` shows it; whole when it fits. A grapheme cluster is never cut in two.
pub(crate) fn cut(text: &str, columns: usize, method: WidthMethod) -> String {
    if text_width(text, method) <= columns {
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
/// grapheme clusters.
fn pieces(text: &str, method: WidthMethod) -> Vec<&str> {
    match method {
        WidthMethod::Grapheme => text.graphemes(true).collect(),
        _ => text.split_inclusive(|_| true).collect(),
    }
}

/// The end of a tool call worth a line: a failure or a non-zero exit. `None` for a
/// call that went well.
pub(crate) fn tool_result(tool: &str, is_error: bool, exit_code: Option<i32>) -> Option<String> {
    match exit_code {
        Some(code) if code != 0 => Some(format!("{tool} exited with {code}")),
        _ if is_error => Some(format!("{tool} failed")),
        _ => None,
    }
}

/// The line of a call that efr refused before it ran, such as `shell refused: efr's
/// config (floor)`, with the daemon's reason.
pub(crate) fn refused(tool: &str, reason: &str) -> String {
    format!("{} refused: {}", one_line(tool), one_line(reason))
}

/// A turn's settings on one line, such as `mode auto, model gpt-5.4, effort high`: every
/// value with `all`, else only the ones that the prompt set. `None` when that leaves
/// none. An effort that is not sent shows as `default`, the backend's.
pub(crate) fn turn_settings(settings: &EffectiveSettings, all: bool) -> Option<String> {
    let EffectiveSettings { mode, model, effort, overridden, .. } = settings;
    let mut parts = Vec::new();
    if all || overridden.mode {
        parts.push(format!("mode {mode}"));
    }
    if all || overridden.model {
        parts.push(format!("model {}", one_line(model)));
    }
    if all || overridden.effort {
        parts.push(format!("effort {}", one_line(effort.as_deref().unwrap_or("default"))));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The answer to an approval in a word.
pub(crate) fn decision(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Allow => "allowed",
        ApprovalDecision::Deny => "denied",
        _ => "answered",
    }
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

/// A conversation's status in the words of the listing.
fn status_word(status: ConversationStatus) -> &'static str {
    match status {
        ConversationStatus::Idle => "idle",
        ConversationStatus::Running => "running",
        ConversationStatus::AwaitingApproval => "awaiting approval",
        _ => "unknown",
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
fn span(seconds: u64) -> String {
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

/// A size in bytes: `512 B`, `3.2 KB`, `1.4 MB`, counted in thousands.
pub(crate) fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{}.{} KB", bytes / 1_000, bytes / 100 % 10),
        _ => format!("{}.{} MB", bytes / 1_000_000, bytes / 100_000 % 10),
    }
}

/// The end-of-turn line of a completed turn, such as `done in 42s, ctx 43% (89k/207k),
/// 1.1k out` when the turn says how full the context is, else `done in 42s, 18.2k
/// tokens in, 1.1k out`. The time and the tokens are left out when they are not known.
pub(crate) fn turn_done(
    took: Option<Duration>,
    usage: Option<&Usage>,
    context: Option<&ContextUse>,
) -> context::GaugedLine {
    let mut line = match took {
        Some(time) => format!("done in {}", self::took(time)),
        None => "done".to_owned(),
    };
    if let Some((gauge, rest)) = context.and_then(context::end_part) {
        line.push_str(", ");
        let mut after = rest;
        if let Some(usage) = usage {
            let _ = write!(after, ", {} out", tokens(usage.output_tokens));
        }
        return context::GaugedLine::new(line, Some(gauge), after);
    }
    if let Some(usage) = usage {
        let _ = write!(
            line,
            ", {} tokens in, {} out",
            tokens(usage.input_tokens),
            tokens(usage.output_tokens)
        );
    }
    line.into()
}

/// The end of an interrupted turn, such as `interrupted after 12s, ctx 43% (89k/207k)`;
/// `interrupted` alone when neither is known.
pub(crate) fn turn_interrupted(
    took: Option<Duration>,
    context: Option<&ContextUse>,
) -> context::GaugedLine {
    let line = match took {
        Some(time) => format!("interrupted after {}", self::took(time)),
        None => "interrupted".to_owned(),
    };
    match context.and_then(context::end_part) {
        Some((gauge, rest)) => context::GaugedLine::new(format!("{line}, "), Some(gauge), rest),
        None => line.into(),
    }
}

/// `efr status`: the daemon's identity and health, one fact per line.
pub(crate) fn status(status: &AdminStatusResult, socket: &Path, now: Timestamp) -> String {
    let mut out = String::new();
    let mut row = |key: &str, value: &str| {
        let _ = writeln!(out, "{key:<14} {value}");
    };
    row(
        "daemon",
        &format!(
            "efrd {}, protocol {}, pid {}",
            one_line(&status.version),
            status.protocol,
            status.pid
        ),
    );
    row("daemon id", &status.daemon_id.to_string());
    // Whole seconds: the nanoseconds of a start time say nothing to a person.
    row("started", &format!("{:.0} ({})", status.started_at, ago(status.started_at, now)));
    row("socket", &one_line(&socket.display().to_string()));
    row("screen", &one_line(&status.screen_backend));
    row("conversations", &status.conversations.to_string());
    row("shells", &status.shells.to_string());
    if status.providers.is_empty() {
        row("providers", "none configured");
    }
    for provider in &status.providers {
        let state = match (provider.logged_in, provider.expires_at) {
            (false, _) => "not logged in".to_owned(),
            (true, None) => "logged in".to_owned(),
            (true, Some(expires)) => {
                format!("logged in, token expires {expires:.0} ({})", until(expires, now))
            }
        };
        row("provider", &format!("{}: {state}", one_line(&provider.provider)));
    }
    if let Some(config) = &status.config {
        let file = match (&config.symlink_target, config.exists) {
            (Some(target), _) => format!("{} -> {}", config.path.display(), target.display()),
            (None, true) => config.path.display().to_string(),
            (None, false) => format!("{} (absent)", config.path.display()),
        };
        row("config", &one_line(&file));
        if let Some(error) = &config.reload_error {
            row("config error", &config_error(error));
        }
        if !config.restart_needed.is_empty() {
            row("restart needed", &one_line(&config.restart_needed.join(", ")));
        }
    }
    // NOTE: a daemon from before the sandbox reports none, so there is nothing to show.
    if let Some(sandbox) = &status.sandbox {
        row("sandbox", &sandbox::status_line(sandbox));
        if let Some(fix) = sandbox.fix.as_deref().filter(|_| !sandbox.available) {
            row("sandbox fix", &one_line(fix));
        }
    }
    out
}

/// A config file's error as one line: the message, then its key and place.
pub(crate) fn config_error(error: &ConfigFileError) -> String {
    let place = config_error_place(error);
    if place.is_empty() {
        one_line(&error.message)
    } else {
        one_line(&format!("{} ({place})", error.message))
    }
}

/// The key and the place of a config file's error, such as `shell.login, line 3,
/// column 1`, or nothing when it has neither.
pub(crate) fn config_error_place(error: &ConfigFileError) -> String {
    let mut parts = Vec::new();
    if let Some(key) = &error.key {
        parts.push(key.clone());
    }
    match (error.line, error.column) {
        (Some(line), Some(column)) => parts.push(format!("line {line}, column {column}")),
        (Some(line), None) => parts.push(format!("line {line}")),
        _ => {}
    }
    parts.join(", ")
}

/// The outcome of a reload, for `efr config reload` and the commands that change the
/// file.
pub(crate) fn reloaded(result: &AdminConfigReloadResult) -> String {
    let mut out = match (&result.error, result.applied) {
        (_, true) => "config.toml reloaded; new turns use it\n".to_owned(),
        (Some(error), false) => {
            format!("config.toml has an error; the old settings stay: {}\n", config_error(error))
        }
        (None, false) => "config.toml was not applied; the old settings stay\n".to_owned(),
    };
    if !result.restart_needed.is_empty() {
        let _ = writeln!(
            out,
            "restart efrd to apply: {} (systemctl --user restart efrd)",
            one_line(&result.restart_needed.join(", "))
        );
    }
    out
}

/// `efr history` without a conversation: one line per conversation, newest first.
pub(crate) fn conversations(list: &ConversationsListResult, now: Timestamp) -> String {
    if list.conversations.is_empty() {
        return "no conversations yet\n".to_owned();
    }
    let mut out = String::new();
    for summary in &list.conversations {
        let title = summary.title.as_deref().map_or_else(|| "(untitled)".to_owned(), one_line);
        // The age is at most 11 characters wide (`59m 59s ago`, `23h 59m ago`), so the
        // titles start in one column.
        let _ = writeln!(
            out,
            "{}  {:<17}  {:>11}  {title}",
            summary.id,
            status_word(summary.status),
            ago(summary.updated_at, now)
        );
    }
    if let Some(cursor) = &list.next_cursor {
        let _ = writeln!(out, "more: efr history --cursor {}", one_line(cursor.as_str()));
    }
    out
}

#[cfg(test)]
mod tests;
