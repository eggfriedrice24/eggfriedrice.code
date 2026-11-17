//! Characters that a text must not carry onto the user's screen as they are, and the
//! body of a desktop notice.

/// The longest body of a desktop notice, in characters.
pub const NOTICE_BODY_MAX: usize = 200;

/// The `<what>` of the notice for a question that waits for the user: an approval, the
/// quarantine question or a question of the model.
pub const QUESTION_WAITS: &str = "a question waits";

/// The `<what>` of the notice for a command that waits for the user's input.
pub const INPUT_WAITS: &str = "a command waits for input";

/// The body of a desktop notice: `<place>: <what>`, or `<what>` alone when `place` is
/// empty, on one line of at most [`NOTICE_BODY_MAX`] characters ([`one_line`]).
///
/// `place` is the name of the project, or the last part of the working directory. The
/// operating system keeps a history of notices, so a caller never puts a prompt, a
/// command line, a title or text of the model into `what`.
pub fn notice_body(place: &str, what: &str) -> String {
    if place.is_empty() {
        one_line(what, NOTICE_BODY_MAX)
    } else {
        one_line(&format!("{place}: {what}"), NOTICE_BODY_MAX)
    }
}

/// The `<place>` of a desktop notice: `project`, the name of the conversation's
/// project, else the last part of the directory `dir`, else `~`. efrd and the CLI use
/// this one rule, so a notice of each names the same place.
pub fn notice_place(project: Option<String>, dir: Option<&std::path::Path>) -> String {
    project
        .or_else(|| dir?.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "~".to_owned())
}

/// `text` on one line of at most `max` characters: each control character, a line
/// break too, becomes a space, and each format character ([`is_format`]) becomes
/// U+FFFD. A longer text is cut and ends with `...`.
pub fn one_line(text: &str, max: usize) -> String {
    let count = text.chars().count();
    let keep = if count <= max { count } else { max.saturating_sub(3) };
    let mut line = String::with_capacity(text.len().min(max.saturating_mul(4)));
    line.extend(text.chars().take(keep).map(|c| match c {
        c if c.is_control() => ' ',
        c if is_format(c) => '\u{fffd}',
        c => c,
    }));
    if keep < count {
        line.push_str("...");
    }
    line
}

/// The Unicode format characters (general category `Cf`, Unicode 16), as ranges: the
/// bidi marks, embeddings, overrides and isolates, the zero-width spaces and joiners,
/// the soft hyphen, the byte order mark and the tags.
///
/// NOTE: they draw nothing, or they change the order in which a terminal draws the
/// text around them, so a command, a path or a text of the model could look different
/// from what runs.
const FORMAT_CHARS: &[(char, char)] = &[
    ('\u{ad}', '\u{ad}'),
    ('\u{600}', '\u{605}'),
    ('\u{61c}', '\u{61c}'),
    ('\u{6dd}', '\u{6dd}'),
    ('\u{70f}', '\u{70f}'),
    ('\u{890}', '\u{891}'),
    ('\u{8e2}', '\u{8e2}'),
    ('\u{180e}', '\u{180e}'),
    ('\u{200b}', '\u{200f}'),
    ('\u{202a}', '\u{202e}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206f}'),
    ('\u{feff}', '\u{feff}'),
    ('\u{fff9}', '\u{fffb}'),
    ('\u{110bd}', '\u{110bd}'),
    ('\u{110cd}', '\u{110cd}'),
    ('\u{13430}', '\u{1343f}'),
    ('\u{1bca0}', '\u{1bca3}'),
    ('\u{1d173}', '\u{1d17a}'),
    ('\u{e0001}', '\u{e0001}'),
    ('\u{e0020}', '\u{e007f}'),
];

/// True for a Unicode format character (general category `Cf`), such as U+202E, the
/// right-to-left override, or U+200B, the zero-width space.
pub fn is_format(c: char) -> bool {
    FORMAT_CHARS.iter().any(|(first, last)| (*first..=*last).contains(&c))
}

#[cfg(test)]
mod tests;
