//! The text rules of the search index: what efrd does to a text of the log before it
//! goes into the index.
//!
//! In this order: the store has already cut the text to 33,792 characters in SQL
//! (`efr_store::search::SOURCE_CHARS`); a command loses the bodies of its heredocs
//! ([`command_text`]); every control and format character goes ([`strip`]); each
//! secret that the index redaction recognises becomes `[***]`
//! (`efr_conversation::redact_for_index`); the text is cut to the cap of its kind at a
//! line end ([`cut`]); an empty text gives no doc. The strip comes before the
//! redaction, so a key with an escape sequence inside it is one word again when the
//! redaction reads it. The cut comes after it: every cap is far below the cut in SQL,
//! so the end of a key that the SQL cut shortened never reaches the index.

use efr_stdx::text::is_format;
use efr_store::search::DocKind;

/// The most bytes of a prompt or a steer in the index.
pub(crate) const PROMPT_CAP: usize = 32 * 1024;

/// The most bytes of an answer in the index.
pub(crate) const ANSWER_CAP: usize = 16 * 1024;

/// The most bytes of a command in the index; with the heredoc bodies gone, a longer
/// command is mostly a script.
pub(crate) const COMMAND_CAP: usize = 2 * 1024;

/// The text of the doc of `kind` for `text`, or `None` when nothing is left.
pub fn index_text(kind: DocKind, text: &str) -> Option<String> {
    let text = if kind == DocKind::Command { command_text(text) } else { text.to_owned() };
    let text = efr_conversation::redact_for_index(&strip(&text));
    let text = cut(&text, cap(kind));
    (!text.trim().is_empty()).then(|| text.to_owned())
}

/// The cap of a doc of `kind`, in bytes.
pub(crate) fn cap(kind: DocKind) -> usize {
    match kind {
        DocKind::Prompt | DocKind::Steer => PROMPT_CAP,
        DocKind::Answer => ANSWER_CAP,
        DocKind::Command => COMMAND_CAP,
    }
}

/// True for a character that the index never holds: a control character but the line
/// end and the tab (C0, DEL and C1), and a format character (the bidi controls, the
/// zero width characters, U+FEFF). It is the rule of the text that efr shows
/// (`efr_stdx::text::is_format`), so no escape sequence reaches a terminal from the
/// index.
pub(crate) fn is_stripped(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t') || is_format(c)
}

/// `text` without the characters of [`is_stripped`].
pub(crate) fn strip(text: &str) -> String {
    if !text.chars().any(is_stripped) {
        return text.to_owned();
    }
    text.chars().filter(|c| !is_stripped(*c)).collect()
}

/// `text` cut to at most `cap` bytes: at the last line end before the cap, or at a
/// character boundary when the first line is longer.
pub(crate) fn cut(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    match text[..end].rfind('\n') {
        Some(line_end) if line_end > 0 => &text[..line_end],
        _ => &text[..end],
    }
}

/// `command` without the bodies of its heredocs, which hold the text of a file: each
/// line up to and with a heredoc operator (`<<WORD`, `<<'WORD'`, `<<"WORD"`,
/// `<<-WORD`, not the here-string `<<<`) stays, the lines after it up to the line that
/// is the end word (after its leading tabs for `<<-`) go, and the end word and the
/// lines after it stay. A heredoc with no end word goes to the end. A line with two operators
/// drops both bodies, one after the other.
pub(crate) fn command_text(command: &str) -> String {
    if !command.contains("<<") {
        return command.to_owned();
    }
    let mut kept: Vec<&str> = Vec::new();
    // The end words still to come, in order, with whether `<<-` allows leading tabs.
    let mut ends: Vec<(String, bool)> = Vec::new();
    let mut at = 0;
    for line in command.split('\n') {
        if let Some((word, tabs)) = ends.get(at) {
            let line_word = if *tabs { line.trim_start_matches('\t') } else { line };
            if line_word == word {
                kept.push(line);
                at += 1;
                if at == ends.len() {
                    ends.clear();
                    at = 0;
                }
            }
            continue;
        }
        kept.push(line);
        ends = heredoc_words(line);
        at = 0;
    }
    kept.join("\n")
}

/// The end words of the heredoc operators of `line`, in order, each with whether it
/// came with `<<-`.
fn heredoc_words(line: &str) -> Vec<(String, bool)> {
    let mut words = Vec::new();
    let mut from = 0;
    while let Some(at) = line[from..].find("<<").map(|at| from + at) {
        let rest = &line[at + 2..];
        if rest.starts_with('<') {
            // A here-string (`<<<`) has no body.
            from = at + 2 + rest.len() - rest.trim_start_matches('<').len();
            continue;
        }
        let (rest, tabs) = match rest.strip_prefix('-') {
            Some(rest) => (rest, true),
            None => (rest, false),
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        let (word, len) = end_word(rest);
        from = line.len() - rest.len() + len;
        if !word.is_empty() {
            words.push((word, tabs));
        }
    }
    words
}

/// The end word at the start of `text` without its quotes, and how many bytes of
/// `text` it takes: `'EOF'`, `"EOF"`, `\EOF` or `EOF`, up to a blank or a shell
/// operator.
fn end_word(text: &str) -> (String, usize) {
    let mut word = String::new();
    let mut chars = text.char_indices().peekable();
    let mut quote: Option<char> = None;
    while let Some((at, c)) = chars.next() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => word.push(c),
            None => match c {
                '\'' | '"' => quote = Some(c),
                '\\' => {
                    if let Some((_, next)) = chars.next() {
                        word.push(next);
                    }
                }
                c if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '<' | '>' | '(' | ')') => {
                    return (word, at);
                }
                c => word.push(c),
            },
        }
    }
    (word, text.len())
}

#[cfg(test)]
mod tests;
