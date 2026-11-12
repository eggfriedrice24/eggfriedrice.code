//! The text of a tool call: the words that name it (`$`, `read`, `write`, ...), what it
//! does, the rows of a long command cut to the width with their marks, and the lines of
//! a call's end. The calls slice of the terminal look owns this file; `format.rs`
//! re-exports its items, so callers name them as `format::...`.

use efr_render::{Glyphs, RenderOptions, WidthMethod, text_width};
use serde_json::Value;

use super::{Tone, cut, fits, one_line, patch, pieces, visible};

/// Keys of a tool's input that best describe a call in one line, in order of
/// preference.
const DETAIL_KEYS: &[&str] = &["command", "cmd", "path", "file", "url", "query"];

/// A tool call in one line, such as `$ ls -la`, `read /etc/hosts` or `apply_patch
/// src/a.rs +3 -1, src/b.rs +1`.
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

/// The name of the tool with which a Claude model changes a file: the exact text
/// `old_string` of the file at `path` becomes `new_string`. A call of it names its file
/// as a `write_file` call does, and its diff and its question show as those of a patch
/// of one file.
pub(crate) const EDIT_TOOL: &str = "edit";

/// The words that name a call of `tool`: `$` for a shell call, `read` and `write` for
/// the file tools, `settings`, `apply_patch` and `edit` for those tools, else the
/// tool's name and a colon.
fn call_name(tool: &str) -> String {
    match tool {
        "shell" => "$".to_owned(),
        "read_file" => "read".to_owned(),
        "write_file" => "write".to_owned(),
        "settings" => "settings".to_owned(),
        patch::TOOL => patch::TOOL.to_owned(),
        EDIT_TOOL => EDIT_TOOL.to_owned(),
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
/// `write`, `settings`, `apply_patch`, `edit`, else the tool and a colon) and each line of what
/// it does, safe to print. Nothing of it is ever left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallText {
    pub(crate) name: String,
    pub(crate) lines: Vec<String>,
    /// The files of an `apply_patch` call, from its patch: the row of the call names
    /// one file with the word of what the patch does to it (`edit`, `write`, `delete`,
    /// `move`), or counts them (`patch 3 files`). Empty for other calls.
    pub(crate) files: Vec<patch::PatchFile>,
}

/// The text of a call of `tool` with `input`, such as `$` and `cargo test -p app`.
pub(crate) fn call_text(tool: &str, input: &Value) -> CallText {
    let detail = call_detail(tool, input);
    let lines = command_lines(&detail).into_iter().map(command_line).collect();
    let files = match patch::text(input).filter(|_| tool == patch::TOOL) {
        Some(text) => patch::files(text),
        None => Vec::new(),
    };
    CallText { name: call_name(tool).trim_end_matches(':').to_owned(), lines, files }
}

/// What ends a row of a command that is cut after a space (`Glyphs::wrap`). The rows
/// after it are indented: the cut falls between two words, so the indent reads as the
/// space that is there. The mark is muted and is not part of the command.
#[cfg(test)]
pub(crate) const WRAP_MARK: &str = Glyphs::UNICODE.wrap;

/// What ends a row of a command that is cut inside a word, because no space fits in
/// the row (`Glyphs::word_cut`). The next row goes on at the same column as this one,
/// so no space shows where the command has none. The mark is muted, is not part of the
/// command, and is no shell syntax.
#[cfg(test)]
pub(crate) const WORD_MARK: &str = Glyphs::UNICODE.word_cut;

/// How a row of a command ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowEnd {
    /// The line ends in this row.
    Last,
    /// The row is cut after a space, and `Glyphs::wrap` follows.
    Space,
    /// The row is cut inside a word, and `Glyphs::word_cut` follows.
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
    /// The mark after the row in the Unicode glyphs, if any.
    pub(crate) fn mark(&self) -> Option<&'static str> {
        self.mark_in(&Glyphs::UNICODE)
    }

    /// The mark after the row in `glyphs`, if any.
    pub(crate) fn mark_in(&self, glyphs: &Glyphs) -> Option<&'static str> {
        match self.end {
            RowEnd::Last => None,
            RowEnd::Space => Some(glyphs.wrap),
            RowEnd::Word => Some(glyphs.word_cut),
        }
    }
}

/// The widest mark that ends a cut row in either table of glyphs: a row keeps room for
/// it whatever the locale.
fn mark_width(method: WidthMethod) -> usize {
    [Glyphs::UNICODE, Glyphs::ASCII]
        .iter()
        .flat_map(|glyphs| [glyphs.wrap, glyphs.word_cut])
        .map(|mark| text_width(mark, method))
        .max()
        .unwrap_or(1)
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
    let mark = mark_width(method);
    let mut rows = Vec::new();
    let mut left = line;
    let mut indented = false;
    loop {
        let here = if indented { room.saturating_sub(indent) } else { room }.max(1);
        if fits(left, here, method) {
            rows.push(CommandRow { text: left.to_owned(), indented, end: RowEnd::Last });
            return rows;
        }
        let budget = here.saturating_sub(mark);
        // `after_space` holds the end of the last space after a word, and the width up
        // to it.
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
                    after_space = Some((end, used));
                }
            } else {
                word = true;
            }
        }
        // NOTE: a cut at a space early in the row would leave a short first word alone,
        // such as `cp \`, with the rest of the row empty: a space cuts only a row that
        // is at least half full, or one that the word after it cannot reach.
        let (cut, kind) = match after_space {
            Some((at, width)) if at == end || width * 2 >= here => (at, RowEnd::Space),
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
            while !fits(left, room, method) {
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
                // NOTE: one piece that is wider than a row has the row to itself and shows
                // once.
                if end == left.len() {
                    break;
                }
                let (row, after) = left.split_at(end);
                push_span(&mut rows, row, *tone);
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
pub(super) fn runs(text: &str) -> Vec<&str> {
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
pub(super) fn push_span(rows: &mut [Vec<(String, Tone)>], text: &str, tone: Tone) {
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
