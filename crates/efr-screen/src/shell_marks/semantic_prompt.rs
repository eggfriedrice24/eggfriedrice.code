//! OSC 133, the semantic prompt marks a shell emits around its prompt and commands.
//!
//! The grammar is the one ghostty's own parser accepts (FinalTerm's marks with the
//! kitty and ghostty options), so the scanner and the ghostty backend agree on what a
//! mark is. The body after `133;` is one action letter, then optionally `;` and
//! options separated by `;`:
//!
//! | Action | Meaning | Options read here |
//! |---|---|---|
//! | `A` | fresh line, then a new prompt starts | `aid`, `k`, `cl` |
//! | `P` | a prompt starts (a continuation or a right prompt) | `aid`, `k`, `cl` |
//! | `B` | the prompt ends and the user's input starts | none |
//! | `C` | the input ends and the command's output starts | `aid`, `cmdline`, `cmdline_url` |
//! | `D` | the command ended | the exit code first, then `aid`, `err` |
//!
//! An option is `key=value`. The first occurrence of a key wins, a malformed value is
//! ignored and unknown keys are ignored, as ghostty does. The data model follows
//! wezterm's semantic prompt and command status types.

use super::percent_decode;

/// One OSC 133 mark.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SemanticPromptEvent {
    /// A prompt starts (`A` or `P`).
    PromptStart {
        /// Which prompt this is (`k`); the primary prompt when absent.
        kind: PromptKind,
        /// The application id (`aid`) that pairs marks of one shell.
        aid: Option<String>,
        /// How the shell handles a click in its input line (`cl`).
        click: Option<ClickMode>,
        /// True for `A`, which also asks for a fresh line; false for `P`.
        fresh_line: bool,
    },
    /// The prompt ended and the user's input starts (`B`).
    InputStart,
    /// The input ended and the command's output starts (`C`). The output is the
    /// recording from the end of this mark to the start of the next
    /// [`CommandEnd`](SemanticPromptEvent::CommandEnd).
    OutputStart {
        /// The command line, decoded from `cmdline` (printf `%q` quoting) or
        /// `cmdline_url` (percent encoding), when the shell sent it.
        command: Option<String>,
        /// The application id (`aid`).
        aid: Option<String>,
    },
    /// The command ended (`D`). A bare `D` closes a `C` whose command never ran, such
    /// as an empty line or one cancelled with Ctrl+C.
    CommandEnd {
        /// The exit status, the first value after `D;`, when there is one.
        exit_code: Option<i32>,
        /// The error text (`err`), when the shell sent one.
        error: Option<String>,
        /// The application id (`aid`).
        aid: Option<String>,
    },
}

/// Which prompt a [`PromptStart`](SemanticPromptEvent::PromptStart) begins (`k`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PromptKind {
    /// The primary prompt (`k=i`, and the default).
    #[default]
    Initial,
    /// A continuation line the user can edit (`k=c`).
    Continuation,
    /// A secondary prompt such as `PS2` (`k=s`).
    Secondary,
    /// A right-aligned prompt (`k=r`).
    Right,
}

/// How a shell moves its cursor when the user clicks in the input (`cl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClickMode {
    /// Left and right arrows within one line (`cl=line`).
    Line,
    /// Left and right arrows across the lines of the input (`cl=m`).
    Multiple,
    /// Up and down arrows too, moving to the line start first (`cl=v`).
    ConservativeVertical,
    /// Up and down arrows with editor-aware column clamping (`cl=w`).
    SmartVertical,
}

/// Parses the body after `133;`. `None` means the body is not a mark this scanner
/// knows: an empty body, an unknown action or an action followed by something other
/// than `;`.
pub(crate) fn parse(data: &[u8]) -> Option<SemanticPromptEvent> {
    let (&action, rest) = data.split_first()?;
    let options = match rest {
        [] => &[][..],
        [b';', options @ ..] => options,
        _ => return None,
    };
    let event = match action {
        b'A' | b'P' => SemanticPromptEvent::PromptStart {
            kind: option(options, b"k").and_then(prompt_kind).unwrap_or_default(),
            aid: text_option(options, b"aid"),
            click: option(options, b"cl").and_then(click_mode),
            fresh_line: action == b'A',
        },
        b'B' => SemanticPromptEvent::InputStart,
        b'C' => SemanticPromptEvent::OutputStart {
            command: command_line(options),
            aid: text_option(options, b"aid"),
        },
        b'D' => SemanticPromptEvent::CommandEnd {
            exit_code: exit_code(options),
            error: text_option(options, b"err"),
            aid: text_option(options, b"aid"),
        },
        _ => return None,
    };
    Some(event)
}

/// The value of the first `key=value` field whose key is `key`.
fn option<'a>(options: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    options.split(|&byte| byte == b';').find_map(|field| {
        let equals = field.iter().position(|&byte| byte == b'=')?;
        let (name, value) = field.split_at(equals);
        (name == key).then(|| &value[1..])
    })
}

fn text_option(options: &[u8], key: &[u8]) -> Option<String> {
    option(options, key).and_then(|value| String::from_utf8(value.to_vec()).ok())
}

/// The first field after `D;`, when it is a number.
fn exit_code(options: &[u8]) -> Option<i32> {
    let first = options.split(|&byte| byte == b';').next()?;
    std::str::from_utf8(first).ok()?.parse().ok()
}

fn prompt_kind(value: &[u8]) -> Option<PromptKind> {
    match value {
        b"i" => Some(PromptKind::Initial),
        b"c" => Some(PromptKind::Continuation),
        b"s" => Some(PromptKind::Secondary),
        b"r" => Some(PromptKind::Right),
        _ => None,
    }
}

fn click_mode(value: &[u8]) -> Option<ClickMode> {
    match value {
        b"line" => Some(ClickMode::Line),
        b"m" => Some(ClickMode::Multiple),
        b"v" => Some(ClickMode::ConservativeVertical),
        b"w" => Some(ClickMode::SmartVertical),
        _ => None,
    }
}

/// `cmdline` wins over `cmdline_url` when both are present, even when it does not
/// decode, as in ghostty.
fn command_line(options: &[u8]) -> Option<String> {
    let decoded = match option(options, b"cmdline") {
        Some(value) => printf_q_decode(value)?,
        None => percent_decode(option(options, b"cmdline_url")?)?,
    };
    String::from_utf8(decoded).ok()
}

/// Undoes the quoting of the shell's `printf %q`: an optional `$'...'` or `'...'`
/// wrapper, then the backslash escapes ghostty accepts. Any other escape, or a
/// trailing backslash, makes the value undecodable.
fn printf_q_decode(value: &[u8]) -> Option<Vec<u8>> {
    let inner = if let Some(rest) = value.strip_prefix(b"$'") {
        rest.strip_suffix(b"'")?
    } else if let Some(rest) = value.strip_prefix(b"'") {
        rest.strip_suffix(b"'")?
    } else {
        value
    };
    let mut decoded = Vec::with_capacity(inner.len());
    let mut bytes = inner.iter();
    while let Some(&byte) = bytes.next() {
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        let escaped = match *bytes.next()? {
            literal @ (b' ' | b'\\' | b'"' | b'\'' | b'$') => literal,
            b'e' => 0x1b,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            _ => return None,
        };
        decoded.push(escaped);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests;
