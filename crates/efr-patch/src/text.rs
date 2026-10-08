//! A file's text as lines that keep their line endings.

/// The end of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    /// `\n`.
    Lf,
    /// `\r\n`.
    CrLf,
}

impl Ending {
    fn as_str(self) -> &'static str {
        match self {
            Ending::Lf => "\n",
            Ending::CrLf => "\r\n",
        }
    }
}

/// One line: its text without the ending, and the ending it has in the file. A line
/// that the patch adds has no ending of its own until [`join`] gives it one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Line<'a> {
    pub(crate) text: &'a str,
    pub(crate) ending: Option<Ending>,
}

/// A text split into lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lines<'a> {
    pub(crate) lines: Vec<Line<'a>>,
    /// The ending of the first line that has one, else `\n`. An added line gets it.
    pub(crate) preferred: Ending,
    /// False when the text has lines and the last one has no ending.
    pub(crate) final_newline: bool,
}

/// Splits `text` at each `\n`. A `\r` right before it is part of the ending, so a
/// file with Windows line endings keeps them; a `\r` anywhere else is text.
pub(crate) fn split(text: &str) -> Lines<'_> {
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(end) = rest.find('\n') {
        let (line, ending) = match rest[..end].strip_suffix('\r') {
            Some(line) => (line, Ending::CrLf),
            None => (&rest[..end], Ending::Lf),
        };
        lines.push(Line { text: line, ending: Some(ending) });
        rest = &rest[end + 1..];
    }
    let final_newline = rest.is_empty();
    if !final_newline {
        lines.push(Line { text: rest, ending: None });
    }
    let preferred = lines.iter().find_map(|line| line.ending).unwrap_or(Ending::Lf);
    Lines { lines, preferred, final_newline }
}

/// Joins `lines` back into a text. A line without an ending gets `preferred`, except
/// the last line when `final_newline` is false: it gets none, so a file that had no
/// final newline keeps that.
pub(crate) fn join(lines: &[Line<'_>], preferred: Ending, final_newline: bool) -> String {
    let size = lines.iter().map(|line| line.text.len() + 2).sum();
    let mut text = String::with_capacity(size);
    let last = lines.len().saturating_sub(1);
    for (index, line) in lines.iter().enumerate() {
        text.push_str(line.text);
        if index < last || final_newline {
            text.push_str(line.ending.unwrap_or(preferred).as_str());
        }
    }
    text
}

#[cfg(test)]
mod tests;
