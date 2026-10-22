//! The live search picker of `,resume`: fzf, inline below the command line.
//!
//! efr starts fzf as a child, writes the first rows to its stdin, and reads the key and
//! the chosen row from its stdout. fzf draws on the terminal itself, in 40% of its
//! height below the command line (`--height`), never full screen, and clears its rows
//! when it ends. On each key fzf runs `efr search --picker` and on each move of the
//! focus `efr search --preview`; both take the query from `FZF_QUERY`, never from an
//! argument, so no other local user can read it in the process list. The words of
//! `,resume <words>` reach fzf in its environment as `FZF_DEFAULT_OPTS`, never in its
//! arguments.
//!
//! fzf is an optional program. Without it, or with one older than [`FZF_FLOOR`], `efr
//! resume` shows the numbered list. The pure parts are here: the command line of fzf
//! ([`fzf_args`]), the quoting of a word for its commands ([`exec_word`]), its colours
//! from the colour roles ([`fzf_colours`]), the version ([`parse_version`]) and what
//! fzf's exit means ([`outcome`]). The child process is in `picker/fzf.rs`.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use efr_protocol::ConversationId;
use efr_render::{ColourMode, RenderOptions, Role};

use crate::context::{Ending, Stop};
use crate::error::CliError;

mod fzf;

pub(crate) use fzf::ProcessFzf;

/// The oldest fzf that has every option of [`fzf_args`] and exports `FZF_QUERY` to the
/// commands of the picker. fzf's `CHANGELOG.md` names 0.46.0 for `FZF_QUERY`; every
/// other option is older (`reload-sync` and the `load` event 0.36.0, the `<N(...)`
/// preview layout 0.31.0, `--disabled` 0.25.0, `refresh-preview` 0.22.0).
pub(crate) const FZF_FLOOR: Version = Version { major: 0, minor: 46, patch: 0 };

/// The name of the program on the `PATH`.
pub(crate) const FZF: &str = "fzf";

/// A version of fzf, such as `0.73.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    pub(crate) major: u32,
    pub(crate) minor: u32,
    pub(crate) patch: u32,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

/// Why `efr resume` shows the list and not the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoPicker {
    /// Stdin or stdout is not a terminal.
    NotATerminal,
    /// `TERM` is unset or `dumb`.
    DumbTerminal,
    /// The context names no terminal.
    NoTerminalInContext,
    /// `resume.picker = "list"`.
    ConfigList,
    /// No `fzf` in an absolute directory of the `PATH`.
    NotFound,
    /// The path of efr or the name of the terminal has a character that fzf would
    /// change in its commands, or is not UTF-8.
    UnsafePath,
    /// fzf is older than [`FZF_FLOOR`].
    TooOld(Version),
    /// `fzf --version` failed, took too long or printed no version.
    VersionFailed,
    /// efrd does not announce the search methods.
    NoSearch,
}

impl NoPicker {
    /// The muted line after the list that says why the picker did not open; `None`
    /// when the list is what the user expects, such as without fzf.
    pub(crate) fn note(&self) -> Option<String> {
        match self {
            NoPicker::TooOld(version) => {
                Some(format!("fzf {version} is older than {FZF_FLOOR}, so efr shows the list"))
            }
            NoPicker::VersionFailed => {
                Some("fzf --version gave no version, so efr shows the list".to_owned())
            }
            NoPicker::UnsafePath => Some(
                "the path of efr or the name of the terminal has a character that fzf would change, so efr shows the list"
                    .to_owned(),
            ),
            NoPicker::NotATerminal
            | NoPicker::DumbTerminal
            | NoPicker::NoTerminalInContext
            | NoPicker::ConfigList
            | NoPicker::NotFound
            | NoPicker::NoSearch => None,
        }
    }
}

/// What the user did in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Enter on a row: continue that conversation here.
    Resume(ConversationId),
    /// Ctrl-O on a row: print the conversation, as `efr history` does.
    Print(ConversationId),
    /// Esc, or Enter with no row: nothing changes.
    Closed,
    /// Ctrl+C.
    Interrupted,
    /// fzf failed with this exit status, or a signal ended it (`None`).
    Failed(Option<i32>),
}

impl Outcome {
    /// The muted line after the list when fzf failed.
    pub(crate) fn failure_note(status: Option<i32>) -> String {
        match status {
            Some(code) => format!("fzf ended with status {code}, so efr shows the list"),
            None => "a signal ended fzf, so efr shows the list".to_owned(),
        }
    }
}

/// The fzf that runs the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fzf {
    pub(crate) path: PathBuf,
    pub(crate) version: Version,
}

/// What the commands of the picker need: the quoted path of efr and the quoted name of
/// the terminal, both as [`exec_word`] writes them, and the value of `--color`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerLook {
    efr: String,
    tty: String,
    colours: String,
}

impl PickerLook {
    /// The look for the efr at `efr` on the terminal `tty`; `None` when either one
    /// cannot go into a command of fzf as it is ([`exec_word`]).
    pub(crate) fn new(efr: &OsStr, tty: &str, colours: String) -> Option<PickerLook> {
        Some(PickerLook { efr: exec_word(efr)?, tty: exec_word(OsStr::new(tty))?, colours })
    }
}

/// One run of the picker.
#[derive(Debug, Clone)]
pub(crate) struct PickRequest {
    /// The first rows, as `efr search --picker` prints them: the header, then a row per
    /// conversation.
    pub(crate) rows: Vec<u8>,
    /// The words of `,resume <words>`, the first query.
    pub(crate) query: Option<String>,
    pub(crate) look: PickerLook,
    /// The directory that fzf and its commands run in: the context's.
    pub(crate) cwd: PathBuf,
}

/// What ends the picker early: Ctrl+C, and SIGTERM or SIGHUP.
pub(crate) struct Stops {
    pub(crate) interrupt: Stop,
    pub(crate) terminate: Ending,
}

impl fmt::Debug for Stops {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stops").finish_non_exhaustive()
    }
}

/// The future of [`Picker::find`].
pub(crate) type Finding<'a> = Pin<Box<dyn Future<Output = Result<Fzf, NoPicker>> + Send + 'a>>;

/// The future of [`Picker::run`].
pub(crate) type Picking<'a> = Pin<Box<dyn Future<Output = Result<Outcome, CliError>> + Send + 'a>>;

/// Where the picker comes from: fzf in production, a script in the command tests.
pub(crate) trait Picker: Send + Sync + fmt::Debug {
    /// Finds fzf on the `PATH` (absolute entries only) and checks its version.
    fn find(&self) -> Finding<'_>;

    /// The path of this efr, which the commands of the picker run; `None` when it is
    /// not known.
    fn efr(&self) -> Option<PathBuf>;

    /// Runs the picker until the user chose, closed it, or `stops` ended it. A
    /// SIGTERM or SIGHUP ends it with [`CliError::Ended`].
    fn run(&self, fzf: Fzf, request: PickRequest, stops: Stops) -> Picking<'_>;
}

/// The command line of fzf after the program, for `look`. Every option is explicit, so
/// a newer fzf with other defaults draws the same picker.
pub(crate) fn fzf_args(look: &PickerLook) -> Vec<OsString> {
    let PickerLook { efr, tty, colours } = look;
    [
        "--height=40%".to_owned(),
        "--min-height=15".to_owned(),
        "--layout=reverse".to_owned(),
        "--no-mouse".to_owned(),
        "--disabled".to_owned(),
        "--ansi".to_owned(),
        "--delimiter=\t".to_owned(),
        "--with-nth=2..".to_owned(),
        "--header-lines=1".to_owned(),
        "--prompt=resume> ".to_owned(),
        "--expect=ctrl-o,esc".to_owned(),
        "--bind=load:refresh-preview".to_owned(),
        // NOTE: the reload has no closing parenthesis and comes last in its own
        // --bind, so no character of the path of efr can end it.
        format!("--bind=change:first+reload-sync:exec {efr} search --picker --tty {tty}"),
        format!("--preview=exec {efr} search --preview {{1}} --tty {tty}"),
        "--preview-window=right,50%,border-left,<50(down,50%,border-top)".to_owned(),
        format!("--color={colours}"),
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

/// `word` in single quotes for `sh`, with its exact bytes; `None` when it is not UTF-8
/// or holds a control character, `{` or `}`: fzf replaces its placeholders also inside
/// quotes, and a control character has no place in a command.
pub(crate) fn exec_word(word: &OsStr) -> Option<String> {
    let text = word.to_str()?;
    if text.is_empty() || text.chars().any(|c| c.is_control() || c == '{' || c == '}') {
        return None;
    }
    Some(quote(text))
}

/// `text` in single quotes for a POSIX shell and for fzf's word splitter: each `'`
/// becomes `'\''`.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The value of `FZF_DEFAULT_OPTS` that gives fzf `words` as its first query. fzf
/// splits the variable into words with shell quotes, so the query stays one word; a
/// control character becomes a blank.
pub(crate) fn fzf_query_opts(words: &str) -> OsString {
    let clean: String = words.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    OsString::from(format!("--query={}", quote(&clean)))
}

/// The value of `--color`: a base scheme, then the colour of each part of fzf from the
/// efr role with the same palette key in the eggfriedrice design system (its `fzf` and
/// `efr` extras). The base is `bw` without colour (`NO_COLOR`), `16` with the
/// terminal's 16 colours, and `dark` or `light` (`light`) in 24-bit colour. A role
/// with no colour writes `-1`, the terminal's own. The parts with no efr role
/// (`bg+`, `border`, `separator`, `label`, `scrollbar`) keep the base scheme's.
pub(crate) fn fzf_colours(options: &RenderOptions, light: bool) -> String {
    let base = match options.colour() {
        ColourMode::None => "bw",
        ColourMode::Ansi16 => "16",
        ColourMode::TrueColor if light => "light",
        ColourMode::TrueColor => "dark",
    };
    let parts = [
        ("fg", Some(Role::Text)),
        ("fg+", Some(Role::Text)),
        ("query", Some(Role::Text)),
        ("hl", Some(Role::Accent)),
        ("hl+", Some(Role::Accent)),
        ("prompt", Some(Role::Accent)),
        ("pointer", Some(Role::Accent)),
        ("marker", Some(Role::Success)),
        ("info", Some(Role::Quote)),
        ("spinner", Some(Role::DiffHunk)),
        ("header", Some(Role::DiffHunk)),
        ("bg", None),
        ("gutter", None),
    ];
    let mut out = base.to_owned();
    for (name, role) in parts {
        let colour = role.and_then(|role| fzf_colour(options, role));
        out.push(',');
        out.push_str(name);
        out.push(':');
        out.push_str(colour.as_deref().unwrap_or("-1"));
    }
    out
}

/// The colour of `role` as fzf writes one: a palette number (`3`) or `#rrggbb`, from
/// the SGR parameters that efr writes for the role, so it is the colour that efr's own
/// lines show in this colour mode; `None` without one.
fn fzf_colour(options: &RenderOptions, role: Role) -> Option<String> {
    let params = options.sgr(role)?;
    let codes: Vec<u16> = params.split(';').filter_map(|code| code.parse().ok()).collect();
    let mut colour = None;
    let mut at = 0;
    while let Some(&code) = codes.get(at) {
        match code {
            30..=37 => colour = Some((code - 30).to_string()),
            90..=97 => colour = Some((code - 90 + 8).to_string()),
            38 => match codes.get(at + 1) {
                Some(5) => {
                    colour = codes.get(at + 2).map(u16::to_string);
                    at += 2;
                }
                Some(2) => {
                    if let [r, g, b] = codes.get(at + 2..at + 5).unwrap_or_default() {
                        colour = Some(format!("#{r:02x}{g:02x}{b:02x}"));
                    }
                    at += 4;
                }
                _ => {}
            },
            _ => {}
        }
        at += 1;
    }
    colour
}

/// The version that `fzf --version` prints, such as `0.73.1 (ce4bef75)` or
/// `0.44.1 (debian)`; a missing patch is 0. `None` without a major and a minor number.
pub(crate) fn parse_version(text: &str) -> Option<Version> {
    let word = text.split_whitespace().next()?;
    let mut parts = word.split('.').map(|part| {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    let major = parts.next()??;
    let minor = parts.next()??;
    let patch = parts.next().flatten().unwrap_or(0);
    Some(Version { major, minor, patch })
}

/// What fzf's exit `status` (`None` for a signal) and its `stdout` mean. With
/// `--expect`, the first line is the key that ended fzf (empty for Enter) and the
/// second the chosen row, whose first field, before the tab, is the conversation id.
pub(crate) fn outcome(status: Option<i32>, stdout: &[u8]) -> Outcome {
    match status {
        Some(0 | 1) => {}
        Some(130) => return Outcome::Interrupted,
        other => return Outcome::Failed(other),
    }
    let mut lines = stdout.split(|byte| *byte == b'\n');
    let key = lines.next().unwrap_or_default();
    let id = lines
        .next()
        .and_then(|row| row.split(|byte| *byte == b'\t').next())
        .and_then(|id| std::str::from_utf8(id).ok())
        .and_then(|id| id.trim().parse::<ConversationId>().ok());
    match (key, id) {
        (b"esc", _) | (_, None) => Outcome::Closed,
        (b"", Some(id)) => Outcome::Resume(id),
        (b"ctrl-o", Some(id)) => Outcome::Print(id),
        _ => Outcome::Closed,
    }
}

/// The first `fzf` on `path`, a `PATH` value, for which `executable` holds. An entry
/// that is empty or not absolute (`.`, `./node_modules/.bin`) is skipped, so a file of
/// the current project never runs.
pub(crate) fn find_on_path(path: &OsStr, executable: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(FZF))
        .find(|candidate| executable(candidate))
}

#[cfg(test)]
mod tests;
