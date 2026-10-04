//! The cross-check between the shell mark scanner and libghostty-vt's own view.
//!
//! The scanner in efr-screen is the source of truth for OSC 133 and OSC 7: it gives
//! marks recording offsets and runs the same for every backend. libghostty-vt parses
//! the same sequences for itself: it keeps a working directory (`Terminal::pwd`) and
//! a semantic prompt state per row (`Row::semantic_prompt`). Where both read the same
//! bytes they must agree, so a ghostty screen runs a second scanner over what it is
//! fed, writes the bytes to the terminal in pieces that end where a mark ends, and
//! compares the terminal after each piece with what the mark says.
//!
//! What is compared:
//!
//! - an OSC 7 mark: libghostty-vt's working directory, read as an OSC 7 URL, names
//!   the same host and path;
//! - any other piece: libghostty-vt's working directory did not change, because the
//!   scanner saw no OSC 7 (ghostty also takes OSC 7 forms the scanner refuses, and
//!   dispatches an OSC that an `ESC` cut short);
//! - a prompt mark: the cursor row's prompt state is `Prompt` for a primary or right
//!   prompt and `Continuation` for a continuation or secondary prompt, and after `A`
//!   the cursor is in column 0, because `A` asks for a fresh line.
//!
//! A disagreement is logged as a warning by the screen and never fails a feed. When a
//! libghostty-rs pin exposes ghostty's semantic prompt effect (ghostty 7bb45ba), it is
//! wired here, as one more comparison, and nowhere else.

use std::fmt;
use std::path::PathBuf;

use efr_screen::{PromptKind, SemanticPromptEvent, Seq, ShellMarkKind, ShellMarkScanner};
use libghostty_vt::Terminal;
use libghostty_vt::screen::RowSemanticPrompt;
use libghostty_vt::terminal::{Point, PointCoordinate};

use crate::effects::Effects;

/// The scanner and the stream position of one ghostty screen.
#[derive(Debug, Default)]
pub(crate) struct CrossCheck {
    scanner: ShellMarkScanner,
    /// The offset of the next byte in this screen's own stream. The screen sees its
    /// bytes without recording offsets, and a scanner only needs them to continue.
    next: u64,
    /// libghostty-vt's working directory when it was last read.
    pwd: Option<String>,
}

/// One way libghostty-vt and the scanner disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Disagreement {
    /// The scanner read an OSC 7 mark, but libghostty-vt's working directory names
    /// another place, or none.
    Cwd {
        /// The host of the mark.
        host: Option<String>,
        /// The path of the mark.
        path: PathBuf,
        /// libghostty-vt's working directory, as it stores it.
        backend: Option<String>,
    },
    /// libghostty-vt changed its working directory where the scanner read no OSC 7
    /// mark.
    UnmarkedCwd {
        /// libghostty-vt's new working directory, as it stores it.
        backend: Option<String>,
    },
    /// A prompt mark left the cursor row in another prompt state.
    PromptRow {
        /// The kind of prompt the mark started.
        kind: PromptKind,
        /// The state the mark implies.
        expected: RowSemanticPrompt,
        /// The state of the cursor row, if libghostty-vt could tell.
        actual: Option<RowSemanticPrompt>,
    },
    /// `A` asks for a fresh line, but the cursor is not in column 0 after it.
    FreshLine {
        /// The cursor column, if libghostty-vt could tell.
        col: Option<u16>,
    },
}

impl CrossCheck {
    /// A cross-check for `terminal`, which may already hold a working directory (a
    /// screen restored from a snapshot does).
    pub(crate) fn new(terminal: &Terminal<'_, '_>) -> Self {
        let mut check = CrossCheck::default();
        check.read_pwd(terminal);
        check
    }

    /// Writes `bytes` to `terminal`, split at the end of every mark the scanner finds,
    /// and returns what libghostty-vt disagreed on. `effects` must be the buffer of
    /// `terminal`'s callbacks; its working directory flag is consumed here.
    pub(crate) fn write(
        &mut self,
        terminal: &mut Terminal<'_, '_>,
        effects: &Effects,
        bytes: &[u8],
    ) -> Vec<Disagreement> {
        let base = self.next;
        self.next = base.saturating_add(bytes.len() as u64);
        let marks = self.scanner.scan(bytes, Seq::new(base));
        let mut found = Vec::new();
        let mut written = 0;
        for mark in marks {
            let end = usize::try_from(mark.end.get().saturating_sub(base))
                .map_or(bytes.len(), |end| end.clamp(written, bytes.len()));
            terminal.vt_write(&bytes[written..end]);
            written = end;
            self.check(terminal, effects, Some(&mark.kind), &mut found);
        }
        if written < bytes.len() {
            terminal.vt_write(&bytes[written..]);
            self.check(terminal, effects, None, &mut found);
        }
        found
    }

    /// Compares `terminal` with the mark that ended the piece just written, or with
    /// no mark for a piece after the last mark.
    pub(crate) fn check(
        &mut self,
        terminal: &Terminal<'_, '_>,
        effects: &Effects,
        mark: Option<&ShellMarkKind>,
        found: &mut Vec<Disagreement>,
    ) {
        let pwd_changed = effects.take_pwd_changed();
        match mark {
            // A working directory that is not UTF-8 cannot be read through the safe
            // API, so there is nothing to compare it with.
            Some(ShellMarkKind::CwdChanged { host, path }) => {
                if self.read_pwd(terminal)
                    && !same_place(self.pwd.as_deref(), host.as_deref(), path)
                {
                    found.push(Disagreement::Cwd {
                        host: host.clone(),
                        path: path.clone(),
                        backend: self.pwd.clone(),
                    });
                }
            }
            _ if pwd_changed => {
                let before = self.pwd.clone();
                if self.read_pwd(terminal) && self.pwd != before {
                    found.push(Disagreement::UnmarkedCwd { backend: self.pwd.clone() });
                }
            }
            _ => {}
        }
        if let Some(ShellMarkKind::SemanticPrompt(SemanticPromptEvent::PromptStart {
            kind,
            fresh_line,
            ..
        })) = mark
        {
            check_prompt(terminal, *kind, *fresh_line, found);
        }
    }

    /// Reads libghostty-vt's working directory into `self.pwd`. False when it cannot
    /// be read; `self.pwd` then keeps the last value read.
    fn read_pwd(&mut self, terminal: &Terminal<'_, '_>) -> bool {
        match terminal.pwd() {
            Ok(pwd) => {
                self.pwd = (!pwd.is_empty()).then(|| pwd.to_owned());
                true
            }
            Err(_) => false,
        }
    }
}

/// The prompt state a prompt of `kind` gives its row, as ghostty records it; `None`
/// for a kind this crate does not know yet.
fn expected_row(kind: PromptKind) -> Option<RowSemanticPrompt> {
    match kind {
        PromptKind::Initial | PromptKind::Right => Some(RowSemanticPrompt::Prompt),
        PromptKind::Continuation | PromptKind::Secondary => Some(RowSemanticPrompt::Continuation),
        _ => None,
    }
}

fn check_prompt(
    terminal: &Terminal<'_, '_>,
    kind: PromptKind,
    fresh_line: bool,
    found: &mut Vec<Disagreement>,
) {
    let col = terminal.cursor_x().ok();
    if fresh_line && col != Some(0) {
        found.push(Disagreement::FreshLine { col });
    }
    let Some(expected) = expected_row(kind) else {
        return;
    };
    let actual = cursor_row_prompt(terminal);
    if actual != Some(expected) {
        found.push(Disagreement::PromptRow { kind, expected, actual });
    }
}

fn cursor_row_prompt(terminal: &Terminal<'_, '_>) -> Option<RowSemanticPrompt> {
    let x = terminal.cursor_x().ok()?;
    let y = terminal.cursor_y().ok()?;
    let point = Point::Active(PointCoordinate { x, y: u32::from(y) });
    terminal.grid_ref(point).ok()?.row().ok()?.semantic_prompt().ok()
}

/// True when libghostty-vt's working directory `backend`, read as an OSC 7 URL by
/// the same scanner, names `host` and `path`.
fn same_place(backend: Option<&str>, host: Option<&str>, path: &std::path::Path) -> bool {
    let Some(backend) = backend else {
        return false;
    };
    // libghostty-vt stores the URL as it arrived. Reading it back through a scanner
    // applies exactly the rules that produced the mark, so the two can only differ
    // when libghostty-vt really holds another directory.
    let mut osc = Vec::with_capacity(backend.len() + 5);
    osc.extend_from_slice(b"\x1b]7;");
    osc.extend_from_slice(backend.as_bytes());
    osc.push(0x07);
    let marks = ShellMarkScanner::new().scan(&osc, Seq::ZERO);
    marks.iter().any(|mark| match &mark.kind {
        ShellMarkKind::CwdChanged { host: their_host, path: their_path } => {
            their_host.as_deref() == host && their_path == path
        }
        _ => false,
    })
}

impl Disagreement {
    /// Logs the disagreement as a warning: the screen is still right, but one of the
    /// two parsers misread the stream, and a shell mark may be missing or wrong.
    pub(crate) fn log(&self) {
        tracing::warn!(disagreement = %self, "the ghostty backend disagrees with the shell mark scanner");
    }
}

impl fmt::Display for Disagreement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Disagreement::Cwd { host, path, backend } => write!(
                f,
                "the scanner read OSC 7 for {} on host {}, but libghostty-vt holds {}",
                path.display(),
                host.as_deref().unwrap_or("(none)"),
                backend.as_deref().unwrap_or("no working directory"),
            ),
            Disagreement::UnmarkedCwd { backend } => write!(
                f,
                "libghostty-vt changed its working directory to {} without an OSC 7 mark",
                backend.as_deref().unwrap_or("nothing"),
            ),
            Disagreement::PromptRow { kind, expected, actual } => write!(
                f,
                "a {kind:?} prompt mark left the cursor row in prompt state {actual:?}, not \
                 {expected:?}"
            ),
            Disagreement::FreshLine { col } => {
                write!(f, "OSC 133 A left the cursor in column {col:?}, not at the start of a line")
            }
        }
    }
}

#[cfg(test)]
mod tests;
