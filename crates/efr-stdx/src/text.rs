//! Characters that a text must not carry onto the user's screen as they are, the body
//! of a desktop notice, and the texts of a duration, a size, a cut and hex digits that
//! efrd and `efr` both write.

use std::time::Duration;

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

/// The body of the notice at the end of a turn that took `took`: `<place>: done in
/// 4m 12s`, or `<place>: failed after 1m 3s` when it failed ([`notice_body`],
/// [`span`]).
pub fn notice_ended(place: &str, took: Duration, failed: bool) -> String {
    let span = span(took.as_secs());
    let what = if failed { format!("failed after {span}") } else { format!("done in {span}") };
    notice_body(place, &what)
}

/// A duration in its two largest units: `45s`, `5m 3s`, `2h 5m`, `3d 4h`.
pub fn span(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    let secs = seconds % 60;
    match (days, hours, minutes) {
        (0, 0, 0) => format!("{secs}s"),
        (0, 0, _) => format!("{minutes}m {secs}s"),
        (0, _, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

/// One MiB, the unit of `attachments.max_file_mib`.
pub const MIB: u64 = 1024 * 1024;

/// A size in bytes, counted in thousands, with one decimal under 10 of its unit and
/// none from 10: `512 B`, `2.0 KB`, `14 KB`, `2.1 MB`, `310 MB`, `1.2 GB`.
pub fn byte_size(bytes: u64) -> String {
    let (unit_bytes, unit) = match bytes {
        0..1_000 => return format!("{bytes} B"),
        1_000..1_000_000 => (1_000, "KB"),
        1_000_000..1_000_000_000 => (1_000_000, "MB"),
        _ => (1_000_000_000, "GB"),
    };
    let whole = bytes / unit_bytes;
    if whole < 10 {
        format!("{whole}.{} {unit}", bytes / (unit_bytes / 10) % 10)
    } else {
        format!("{whole} {unit}")
    }
}

/// `bytes` as lowercase hex, two digits for each byte.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    hex
}

/// The line that stands where a cut left `omitted` bytes of a text out, with a line
/// break before and after it: `\n[... 1048576 bytes omitted ...]\n`. The capture of a
/// command and the cut of a tool's output write this one form, so the model reads one
/// form.
pub fn omitted_marker(omitted: u64) -> String {
    format!("\n[... {omitted} bytes omitted ...]\n")
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
