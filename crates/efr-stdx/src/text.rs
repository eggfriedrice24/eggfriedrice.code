//! Characters that a text must not carry onto the user's screen as they are.

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
