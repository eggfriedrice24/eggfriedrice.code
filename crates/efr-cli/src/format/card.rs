//! A question as a card (TM-Questions): a badge and a title, then rows four columns in,
//! then the keys. No frame and no bar, so nothing of the card ends up in a copy.
//!
//! ```text
//!  ?  allow outside the sandbox
//!     cd ~/p/app && find target/debug/build \
//!         -path '*ghostty*' -type f
//!     runs with your full rights: files, secrets, network
//!     programs  cd, find
//!     y allow   n deny
//! ```
//!
//! The badge ` ? ` is the `warning` role in reverse video and bold, and the title is the
//! `warning` role in bold: the loudest element of the turn. Without colour the badge is a
//! bold `?`. The rows say what runs (in the `text` role), what it touches (the risk in
//! `warning`, then labels such as `why`, `programs` and `host`, muted, two spaces, and
//! the value in `text`), and what the model says. The keys are in `warning` bold, what
//! they do muted, three spaces between pairs.
//!
//! An exit question for which the daemon offers a rule (`ExitInfo::always`, phase E)
//! gets the key `a` in its last row: `y allow   n deny   a always…`. After an `a`, two
//! rows confirm it: `always saves the rule  write under ~/notes`, then `y save the rule
//! and allow   n back`, until the next key. A question that another client must answer
//! ends with a muted row that says so, with no keys.
//!
//! Every line of a command shows, each on its own, and nothing of what runs is cut: a
//! line wider than the screen goes on in the next row. A cut after a space ends in a
//! muted `\` and the rows from there are four columns further in; a space cuts only a row
//! that is then at least half full. Else a word is cut, with a muted `↩`, and goes on in
//! the next row at the same column, so the rows show no space that the command does not
//! have. A line that ends in a shell's own `\` goes on in the next line, so its `\` is
//! muted too. Text rows go on in the next row, two columns further in. A diff stands at
//! the card's indent: a row with the file and its counts, the short hunk header, and the
//! lines with their sign. When the output is not a terminal, nothing wraps and nothing is
//! painted.

use efr_render::{
    Attrs, ColourMode, DiffLineKind, Glyphs, RenderOptions, Role, diff_lines, text_width,
};

use super::{CommandRow, RowEnd, Tone, columns, lines, paint, wrap_command, wrap_spans};

/// How far each row of a card stands in: under the title's text.
const INDENT: &str = "    ";

/// How far a row of a command that goes on after a space is indented after the card's
/// indent.
const COMMAND_INDENT: &str = "    ";

/// How far a row of text that goes on is indented after the card's indent.
const TEXT_INDENT: &str = "  ";

/// Between a label and its value.
const LABEL_GAP: &str = "  ";

/// Between the pairs of a row of keys.
const KEY_GAP: &str = "   ";

/// The keys of an approval.
pub(crate) const ALLOW_KEYS: &[(&str, &str)] = &[("y", "allow"), ("n", "deny")];

/// The key that saves the rule that an exit question offers (phase E). It asks for a
/// confirm first.
pub(crate) const ALWAYS_KEY: &str = "a";

/// The word of the key `a`, before the mark of more.
const ALWAYS_WORD: &str = "always";

/// The label of the rule that an `a` answer saves.
const ALWAYS_SAVES: &str = "always saves the rule";

/// The keys of the confirm of an `a` answer.
const CONFIRM_KEYS: &[(&str, &str)] = &[("y", "save the rule and allow"), ("n", "back")];

/// The keys of the quarantine question.
pub(crate) const KEEP_KEYS: &[(&str, &str)] = &[("y", "keep"), ("n", "leave in quarantine")];

/// The row of a question that another client must answer.
const WAITING: &str = "waiting for another client to answer";

/// One row of a card, before it is cut into the rows of a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    /// One line of a command, safe to print: what runs, in the `text` role.
    Command(String),
    /// Text in pieces with their tone, safe to print.
    Text(Vec<(String, Tone)>),
    /// A diff preview, as the daemon sent it.
    Diff(String),
    /// The diff of one file of a patch, as its headers, its lines and the daemon's line
    /// `... N more lines`: its lines show without the headers.
    FileDiff(String),
}

impl Row {
    /// A row of one piece of text in `tone`.
    pub(crate) fn text(text: impl Into<String>, tone: Tone) -> Row {
        Row::Text(vec![(text.into(), tone)])
    }

    /// A row of a muted `label`, two spaces and `value` in the `text` role, such as
    /// `why  network (github.com)`. Both are safe to print.
    pub(crate) fn labelled(label: impl Into<String>, value: impl Into<String>) -> Row {
        Row::Text(vec![
            (label.into(), Tone::Dim),
            (LABEL_GAP.to_owned(), Tone::Plain),
            (value.into(), Tone::Text),
        ])
    }

    /// The value of the row when it is a row of `label`, such as `network (github.com)`
    /// of the row `why  network (github.com)`.
    pub(crate) fn value_of(&self, label: &str) -> Option<String> {
        let Row::Text(pieces) = self else {
            return None;
        };
        match pieces.as_slice() {
            [(first, Tone::Dim), (gap, _), rest @ ..] if first == label && gap == LABEL_GAP => {
                Some(rest.iter().map(|(text, _)| text.as_str()).collect())
            }
            _ => None,
        }
    }
}

/// The row of a question that waits `secs` seconds at most before its call does not
/// run, such as a delete that full mode guards: `expires in 2 min`, in the `warning`
/// role.
pub(crate) fn expires_row(secs: u32) -> Row {
    let time = if secs >= 60 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        super::elapsed(std::time::Duration::from_secs(u64::from(secs)))
    };
    Row::text(format!("expires in {time}"), Tone::Attention)
}

/// What the last row of a card says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Footer {
    /// The keys that answer it here.
    Keys(&'static [(&'static str, &'static str)]),
    /// The keys of an approval and `a`, which saves the rule that the question offers:
    /// `y allow   n deny   a always…`. The rule is safe to print.
    Always(String),
    /// The confirm of an `a` answer: `always saves the rule  write under ~/notes`, then
    /// `y save the rule and allow   n back`. The rule is safe to print.
    Confirm(String),
    /// Another client must answer it.
    Waiting,
}

#[cfg(test)]
thread_local! {
    /// How many cards this thread rendered: a test seam, for the count of the frame
    /// bench (`bench_perf_card_frames`).
    pub(crate) static RENDERS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A question: its title, safe to print, and its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Card {
    pub(crate) title: String,
    pub(crate) rows: Vec<Row>,
}

impl Card {
    /// The card as lines, each with its newline, the last row `footer` when there is
    /// one: cut into rows of the screen's width on a terminal, as they are elsewhere.
    pub(crate) fn render(&self, footer: Option<&Footer>, options: &RenderOptions) -> String {
        #[cfg(test)]
        RENDERS.with(|renders| renders.set(renders.get() + 1));
        let columns = columns(options);
        let glyphs = options.glyphs();
        let mut out = String::new();
        let title = vec![
            (format!(" {} ", glyphs.question), Tone::Badge),
            (" ".to_owned(), Tone::Plain),
            (self.title.clone(), Tone::AttentionBold),
        ];
        for (at, row) in spans(&title, columns, 0, INDENT.len()).into_iter().enumerate() {
            if at > 0 {
                out.push_str(INDENT);
            }
            push_pieces(&mut out, &row, options);
            out.push('\n');
        }
        for row in &self.rows {
            push_row(&mut out, row, options);
        }
        if let Some(footer) = footer {
            out.push_str(&footer_row(footer, options));
        }
        out
    }

    /// What the question asked about, for the line that takes its place, such as
    /// `network (github.com)` or `outside the sandbox`: the value of its `why` row, else
    /// its title after `allow ` when that is not `this …`.
    pub(crate) fn subject(&self) -> Option<String> {
        self.rows
            .iter()
            .find_map(|row| row.value_of(WHY))
            .or_else(|| {
                // NOTE: `allow this command` names no subject of its own.
                let rest = self.title.strip_prefix("allow ")?;
                (!rest.starts_with("this ")).then(|| rest.to_owned())
            })
            .filter(|subject| !subject.trim().is_empty())
    }
}

/// The label of the row that says why a question asks.
pub(crate) const WHY: &str = "why";

/// The last row of a card, with its newline: the keys, the confirm of an `a` answer,
/// or that another client must answer. A row wider than the screen goes on in the next
/// row, indented, as a text row does.
pub(crate) fn footer_row(footer: &Footer, options: &RenderOptions) -> String {
    let glyphs = options.glyphs();
    let mut out = String::new();
    let pieces = match footer {
        Footer::Keys(keys) => key_pieces(keys),
        Footer::Waiting => vec![(WAITING.to_owned(), Tone::Dim)],
        Footer::Always(_) => {
            let mut pieces = key_pieces(ALLOW_KEYS);
            pieces.push((KEY_GAP.to_owned(), Tone::Plain));
            let word = format!("{ALWAYS_WORD}{}", glyphs.more);
            pieces.extend(key_pieces(&[(ALWAYS_KEY, word.as_str())]));
            pieces
        }
        Footer::Confirm(rule) => {
            push_row(&mut out, &Row::labelled(ALWAYS_SAVES, rule.clone()), options);
            key_pieces(CONFIRM_KEYS)
        }
    };
    push_text(&mut out, &pieces, options);
    out
}

/// `keys` as pieces of a row: each key in the `warning` role in bold, a space, what it
/// does muted, three spaces between the pairs.
fn key_pieces(keys: &[(&str, &str)]) -> Vec<(String, Tone)> {
    let mut pieces = Vec::new();
    for (at, (key, what)) in keys.iter().enumerate() {
        if at > 0 {
            pieces.push((KEY_GAP.to_owned(), Tone::Plain));
        }
        pieces.push(((*key).to_owned(), Tone::AttentionBold));
        pieces.push((" ".to_owned(), Tone::Plain));
        pieces.push(((*what).to_owned(), Tone::Dim));
    }
    pieces
}

/// Adds the screen rows of the text `pieces` at the card's indent, the rows after the
/// first two columns further in, with their newlines.
fn push_text(out: &mut String, pieces: &[(String, Tone)], options: &RenderOptions) {
    for (at, row) in
        spans(pieces, columns(options), INDENT.len(), TEXT_INDENT.len()).into_iter().enumerate()
    {
        out.push_str(INDENT);
        if at > 0 {
            out.push_str(TEXT_INDENT);
        }
        push_pieces(out, &row, options);
        out.push('\n');
    }
}

/// Adds the screen rows of `row`, each at the card's indent, with their newlines.
fn push_row(out: &mut String, row: &Row, options: &RenderOptions) {
    let glyphs = options.glyphs();
    match row {
        Row::Command(line) => {
            // NOTE: a line that ends in the shell's own `\` goes on in the next line;
            // its mark is the one of a cut after a space (TM-Questions 1).
            let (body, continued) = match line.strip_suffix(glyphs.wrap) {
                Some(body) if body.is_empty() || body.ends_with(' ') => (body, true),
                _ => (line.as_str(), false),
            };
            let rows = match columns(options) {
                Some((width, method)) => {
                    let mark = if continued { text_width(glyphs.wrap, method) } else { 0 };
                    let room = width.saturating_sub(INDENT.len() + mark).max(1);
                    wrap_command(body, room, COMMAND_INDENT.len(), method)
                }
                None => {
                    vec![CommandRow { text: body.to_owned(), indented: false, end: RowEnd::Last }]
                }
            };
            let last = rows.len().saturating_sub(1);
            for (at, row) in rows.iter().enumerate() {
                out.push_str(INDENT);
                if row.indented {
                    out.push_str(COMMAND_INDENT);
                }
                out.push_str(&paint(&row.text, Tone::Text, options));
                let mark = match row.end {
                    RowEnd::Last if at == last && continued => Some(glyphs.wrap),
                    RowEnd::Last => None,
                    RowEnd::Space => Some(glyphs.wrap),
                    RowEnd::Word => Some(glyphs.word_cut),
                };
                if let Some(mark) = mark {
                    out.push_str(&paint(mark, Tone::Dim, options));
                }
                out.push('\n');
            }
        }
        Row::Text(pieces) => push_text(out, pieces, options),
        Row::Diff(diff) => push_diff(out, diff, true, options),
        Row::FileDiff(diff) => push_diff(out, diff, false, options),
    }
}

/// Adds the rows of `diff` at the card's indent: with `heading`, a row with the file of
/// its headers and its counts (`hyprland.conf  +1 -1`), then each hunk header in its
/// short form (`@@ 41 input`) and each line with its sign, one space and the code, the
/// row's background to its end, then the lines that the daemon cut.
fn push_diff(out: &mut String, diff: &str, heading: bool, options: &RenderOptions) {
    let glyphs = options.glyphs();
    let parts = super::changes::diff_parts(diff);
    if heading {
        let path = parts
            .head
            .iter()
            .find_map(|line| line.strip_prefix("+++ "))
            .map(|path| path.strip_prefix("b/").unwrap_or(path))
            .filter(|path| *path != "/dev/null");
        let mut pieces: Vec<(String, Tone)> = Vec::new();
        if let Some(path) = path {
            pieces.push((super::one_line(path), Tone::Text));
            pieces.push((LABEL_GAP.to_owned(), Tone::Plain));
        }
        pieces.extend(counts(&parts.body));
        push_text(out, &pieces, options);
    }
    if !options.is_terminal() {
        for line in lines(&parts.body.join("\n")).lines() {
            out.push_str(INDENT);
            out.push_str(line);
            out.push('\n');
        }
    } else {
        // NOTE: the file headers give the lines their syntax colours; they show as the
        // row of the file, not as rows of their own.
        let source: Vec<&str> = parts.head.iter().chain(&parts.body).copied().collect();
        let mut text = source.join("\n");
        text.push('\n');
        let inner = usize::from(options.width()).saturating_sub(INDENT.len() + 3).max(10);
        let narrow = options.clone().with_width(u16::try_from(inner).unwrap_or(u16::MAX));
        let lines = diff_lines(&text, &narrow).into_iter().zip(&source).skip(parts.head.len());
        for (line, source) in lines {
            match line.kind {
                DiffLineKind::Hunk => {
                    let header = short_hunk(&super::one_line(source), glyphs);
                    out.push_str(INDENT);
                    out.push_str(&options.paint(Role::DiffHunk, &header));
                    out.push('\n');
                }
                DiffLineKind::Other => {
                    for row in &line.rows {
                        out.push_str(INDENT);
                        out.push_str(row);
                        out.push('\n');
                    }
                }
                // NOTE: without colour the lines have no syntax, so the sign comes from
                // the source line: bold for `+` and `-`, and a removed line dim (TM-Legend
                // 21).
                kind if options.colour() == ColourMode::None => {
                    let (sign, code) = source.split_at(source.len().min(1));
                    let rows = chunks(&super::one_line(code), inner, options);
                    let last = rows.len().saturating_sub(1);
                    for (at, row) in rows.iter().enumerate() {
                        out.push_str(INDENT);
                        match kind {
                            DiffLineKind::Added | DiffLineKind::Removed if at == 0 => {
                                out.push_str(&paint(sign, Tone::Bold, options));
                                out.push(' ');
                            }
                            _ => out.push_str("  "),
                        }
                        if kind == DiffLineKind::Removed {
                            out.push_str(&options.paint_with(Role::Text, Attrs::DIM, row));
                        } else {
                            out.push_str(row);
                        }
                        if at < last {
                            out.push_str(&paint(glyphs.word_cut, Tone::Dim, options));
                        }
                        out.push('\n');
                    }
                }
                _ => {
                    let last = line.rows.len().saturating_sub(1);
                    for (at, row) in line.rows.iter().enumerate() {
                        out.push_str(INDENT);
                        out.push_str(if at == 0 { &line.sign } else { " " });
                        out.push(' ');
                        out.push_str(row);
                        // A row cut inside the line ends in a muted mark; the next row
                        // goes on at the column of the code.
                        if at < last {
                            out.push_str(&paint(glyphs.word_cut, Tone::Dim, options));
                        }
                        if let Some(fill) = &line.fill {
                            out.push_str(&format!("\x1b[{fill}m\x1b[K\x1b[0m"));
                        }
                        out.push('\n');
                    }
                }
            }
        }
    }
    if parts.cut > 0 {
        let more = format!("{} {} more lines", glyphs.more, parts.cut);
        out.push_str(INDENT);
        out.push_str(&paint(&more, Tone::Dim, options));
        out.push('\n');
    }
}

/// `text` in rows of at most `columns` columns, cut anywhere, as a diff line is.
fn chunks(text: &str, columns: usize, options: &RenderOptions) -> Vec<String> {
    let method = options.width_method();
    let mut rows = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let width = text_width(&c.to_string(), method);
        if used + width > columns.max(1) && used > 0 {
            rows.push(String::new());
            used = 0;
        }
        used += width;
        if let Some(row) = rows.last_mut() {
            row.push(c);
        }
    }
    rows
}

/// The counts of the lines of a diff `body` that it adds and removes: `+1` in the
/// `success` role and `-1` in the `error` role.
fn counts(body: &[&str]) -> Vec<(String, Tone)> {
    let added = body.iter().filter(|line| line.starts_with('+')).count();
    let removed = body.iter().filter(|line| line.starts_with('-')).count();
    let mut pieces = Vec::new();
    if added > 0 {
        pieces.push((format!("+{added}"), Tone::Success));
    }
    if removed > 0 {
        if added > 0 {
            pieces.push((" ".to_owned(), Tone::Plain));
        }
        pieces.push((format!("-{removed}"), Tone::Failure));
    }
    pieces
}

/// The short form of the hunk header `line`, `@@ -41,1 +41,1 @@ input`: `@@ 41 input`,
/// the first line of the new side and what git names after the header.
fn short_hunk(line: &str, glyphs: &Glyphs) -> String {
    let rest = line.trim_end().strip_prefix(glyphs.hunk).unwrap_or(line).trim_start();
    let (ranges, context) = rest.split_once(glyphs.hunk).unwrap_or((rest, ""));
    let start = ranges
        .split_whitespace()
        .find_map(|range| range.strip_prefix('+'))
        .map(|range| range.split(',').next().unwrap_or(range))
        .unwrap_or_default();
    let context = context.trim().trim_end_matches('{').trim_end();
    let mut short = glyphs.hunk.to_owned();
    for part in [start, context] {
        if !part.is_empty() {
            short.push(' ');
            short.push_str(part);
        }
    }
    short
}

/// The rows of the preview of an `apply_patch` or `edit` call: for each file of the
/// patch a row that names it, then its diff; an `edit` call has one file. The row of a file that the patch deletes or moves is
/// in the `warning` role (`delete old.rs`, `move a.rs → b.rs`), and a new file's row
/// starts with a muted `new`. Nothing of a diff is left out but what the daemon cut.
pub(crate) fn patch_rows(diff: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for file in super::patch::file_diffs(diff) {
        let mut heading = file.file.heading(true);
        if !heading.is_empty() {
            let counted = counts(&file.parts.body);
            if !counted.is_empty() {
                heading.push((LABEL_GAP.to_owned(), Tone::Plain));
                heading.extend(counted);
            }
            rows.push(Row::Text(heading));
        }
        rows.push(Row::FileDiff(file.text()));
    }
    rows
}

/// `pieces` cut into rows after `before` columns, with `indent` more on each row after
/// the first; one row when nothing wraps.
fn spans(
    pieces: &[(String, Tone)],
    columns: Option<(usize, efr_render::WidthMethod)>,
    before: usize,
    indent: usize,
) -> Vec<Vec<(String, Tone)>> {
    match columns {
        Some((width, method)) => {
            let first = width.saturating_sub(before).max(1);
            let rest = first.saturating_sub(indent).max(1);
            wrap_spans(pieces, first, rest, method)
        }
        None => vec![pieces.to_vec()],
    }
}

/// The card of an approval without an exit: `summary` is the daemon's, `tool` the
/// call's tool and `command` the command of its `tool_call_started`, when they are
/// known.
///
/// A summary that quotes exactly the call's command (`run "cd src\nls"`) shows each
/// line of it on its own; what the summary says besides follows after `why:`. Any
/// other summary shows as one row. The mark of a job notice turn at the start of a
/// summary shows in the title. The line of the parts of a command line that ask
/// follows after `asks for:`.
pub(crate) fn approval(summary: &str, tool: Option<&str>, command: Option<&str>) -> Card {
    // NOTE: the mark of a job notice turn goes to the title, so the rest of the summary
    // reads as any summary and a command keeps its rows.
    if let Some(rest) = strip_wake(summary) {
        let mut card = approval(rest, tool, command);
        mark_wake(&mut card);
        return card;
    }
    // NOTE: a question that full mode still asks starts with a line that says why; it
    // stands first, in the `warning` role, and the rest of the summary follows as for
    // any question, or the command when the summary has nothing more.
    if let Some((guard, rest)) = super::full_guard(summary) {
        let rest = rest.trim();
        let mut card = if rest.is_empty() || rest.starts_with(ASKS) {
            let mut rows = command.map(command_rows).unwrap_or_default();
            if !rest.is_empty() {
                rows.push(asks_row(&super::one_line(rest)));
            }
            Card { title: title(tool).to_owned(), rows }
        } else {
            approval(rest, tool, command)
        };
        card.rows.insert(0, guard_row(guard));
        return card;
    }
    let (first, asking) = super::split_summary(summary);
    let title = title(tool);
    let subjects = tool.and_then(|tool| first.strip_prefix(&format!("{tool}: ")));
    let mut rows = Vec::new();
    let quoted = command.zip(subjects).and_then(|(command, subjects)| {
        let (before, after) = super::around_quote(subjects, &format!("run {command:?}"))?;
        Some((command, before, after))
    });
    match quoted {
        Some((command, before, after)) => {
            rows.extend(command_rows(command));
            let rest: Vec<&str> = [before.trim_end_matches("; ").trim(), after.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            if !rest.is_empty() {
                rows.push(Row::labelled(WHY, super::one_line(&rest.join("; "))));
            }
        }
        None => rows.push(Row::text(super::one_line(subjects.unwrap_or(first)), Tone::Plain)),
    }
    if let Some(asking) = asking {
        rows.push(asks_row(&asking));
    }
    Card { title: title.to_owned(), rows }
}

/// The summary of a question in a job notice turn without its mark
/// ([`efr_protocol::WAKE_QUESTION`]); `None` for any other summary.
pub(crate) fn strip_wake(summary: &str) -> Option<&str> {
    summary.strip_prefix(efr_protocol::WAKE_QUESTION)
}

/// Adds the mark of a job notice turn to the title of `card`:
/// `allow this command (job notice turn)`.
pub(crate) fn mark_wake(card: &mut Card) {
    card.title = format!("{} {}", card.title, efr_protocol::WAKE_QUESTION.trim_end());
}

/// What the line after a summary that names the parts that ask starts with.
const ASKS: &str = "asks for: ";

/// The row of the line that names the parts of a command line that ask, `asks for:
/// curl, ssh`, as a label and its value.
fn asks_row(line: &str) -> Row {
    match line.strip_prefix(ASKS) {
        Some(parts) => Row::labelled(ASKS.trim_end_matches([':', ' ']), parts.to_owned()),
        None => Row::text(line.to_owned(), Tone::Dim),
    }
}

/// The title of an approval of a call of `tool`.
fn title(tool: Option<&str>) -> &'static str {
    match tool {
        Some("shell") => "allow this command",
        Some("write_file") => "allow this write",
        Some(super::patch::TOOL) => "allow this patch",
        Some(super::EDIT_TOOL) => "allow this edit",
        Some("read_file") => "allow this read",
        Some("settings") => "allow this change of the settings",
        _ => "allow this call",
    }
}

/// The row of the line of a summary that says what full mode still asks for, such as
/// `full mode still asks for this: delete of ~ (your home directory)`, in the `warning`
/// role.
pub(crate) fn guard_row(line: &str) -> Row {
    Row::text(super::one_line(line), Tone::Attention)
}

/// A row for each line of `command`, safe to print. A carriage return at the end of a
/// line shows as its stand-in, because a shell does not read it as a space.
pub(crate) fn command_rows(command: &str) -> Vec<Row> {
    super::command_lines(command)
        .into_iter()
        .map(|line| Row::Command(super::command_line(line)))
        .collect()
}

fn push_pieces(out: &mut String, pieces: &[(String, Tone)], options: &RenderOptions) {
    for (text, tone) in pieces {
        out.push_str(&paint(text, *tone, options));
    }
}

#[cfg(test)]
mod tests;
