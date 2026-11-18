//! The cross-check between the shell marks and libghostty-vt's own view.
//!
//! The scanner in efr-screen is the source of truth for OSC 133 and OSC 7: it gives
//! marks recording offsets and runs the same for every backend. libghostty-vt parses
//! the same sequences for itself: it reports each OSC 133 step through its semantic
//! prompt callback, keeps a working directory (`Terminal::pwd`) and a semantic prompt
//! state per row (`Row::semantic_prompt`). Where both read the same bytes they must
//! agree. The actor gives the screen the marks of its one scan with the bytes, and
//! the screen writes the bytes with `Terminal::vt_write_until`, which stops at the end
//! of every OSC that libghostty-vt reads. A mark must end where such an OSC ends, and
//! the terminal at that stop must agree with the mark.
//!
//! What is compared at the end of an OSC:
//!
//! - an OSC 133 mark: libghostty-vt reported the same step (prompt start with the same
//!   kind of prompt, input start, output start, or command end with the same exit
//!   code);
//! - an OSC 7 mark: libghostty-vt's working directory, read as an OSC 7 URL, names
//!   the same host and path;
//! - an OSC without a mark: libghostty-vt's working directory did not change (ghostty
//!   also takes OSC 7 forms the scanner refuses, and dispatches an OSC that an `ESC`
//!   cut short);
//! - a prompt mark: the cursor row's prompt state is `Prompt` for a primary or right
//!   prompt and `Continuation` for a continuation or secondary prompt, and after `A`
//!   the cursor is in column 0, because `A` asks for a fresh line.
//!
//! A mark that ends where libghostty-vt read no OSC end is a disagreement too. An OSC
//! 133 step that libghostty-vt reports where the scanner read no mark is not: the
//! scanner skips the marks of a nested tmux on purpose. Nor is a mark whose OSC is
//! longer than libghostty-vt reads ([`LIBRARY_OSC_BYTES`]): the library drops that
//! OSC, while the scanner reads bodies up to 4096 bytes. A disagreement is logged as a
//! warning by the screen and never fails a feed.

use std::fmt;
use std::path::PathBuf;

use efr_screen::{
    PromptKind, SemanticPromptEvent, Seq, ShellMark, ShellMarkKind, ShellMarkScanner,
};
use libghostty_vt::Terminal;
use libghostty_vt::effect::{self, SemanticPrompt, SemanticPromptKind};
use libghostty_vt::point::{Coordinate, Point};
use libghostty_vt::screen::RowSemanticPrompt;
use libghostty_vt::terminal::{Boundary, StringEnd};

use crate::effects::Effects;

/// The longest body of an OSC 7 or OSC 133 that libghostty-vt reads, in bytes, after
/// the number and its `;`: `Parser.MAX_BUF` in ghostty's `src/terminal/osc.zig`, the
/// fixed buffer of both. The library drops a longer one: no step, no working
/// directory. An OSC 7 body holds one byte less, because the library ends the URL
/// with a NUL in the same buffer.
pub(crate) const LIBRARY_OSC_BYTES: u64 = 2048;

/// The stream position and the last working directory of one ghostty screen.
#[derive(Debug, Default)]
pub(crate) struct CrossCheck {
    /// The recording offset after the last byte written; `None` before the first.
    next: Option<u64>,
    /// An OSC that ended with the `ESC` of `ESC \` as the last byte of a chunk: the
    /// offset where its mark ends, after the `\` in the next chunk. The scanner
    /// reports that mark with the next chunk, so the check waits for it. The
    /// terminal does not change in between, because the `\` only ends the string.
    pending: Option<u64>,
    /// libghostty-vt's working directory when it was last read.
    pwd: Option<String>,
}

/// What libghostty-vt reported of OSC 133 since the step was read last.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ReportedStep {
    /// No step.
    #[default]
    Nothing,
    /// A step of a kind that this crate does not know.
    Unknown,
    /// This step.
    Step(Step),
}

/// One OSC 133 step, as both parsers can tell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// A prompt starts (`A` or `P`), of this kind.
    PromptStart(PromptKind),
    /// The input starts (`B`).
    InputStart,
    /// The output starts (`C`).
    OutputStart,
    /// The command ended (`D`), with this exit code.
    CommandEnd(Option<i32>),
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
    /// The scanner read an OSC 133 mark, but libghostty-vt reported another step at
    /// the end of that OSC, or none.
    Step {
        /// The step of the mark.
        expected: Step,
        /// What libghostty-vt reported.
        actual: ReportedStep,
    },
    /// The scanner read a mark that ends where libghostty-vt read no end of an OSC.
    Unread {
        /// The recording offset just after the mark.
        end: u64,
    },
    /// libghostty-vt stopped at the end of an OSC with a kind or an end that the
    /// binding cannot name, so the bytes it consumed are not known. The rest of the
    /// chunk was dropped. The pinned library never gives such a stop.
    UnknownStop {
        /// The recording offset of the last byte known to be written, plus one.
        after: u64,
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

    /// Writes `bytes`, whose first byte is at recording offset `base`, to `terminal`
    /// and returns what libghostty-vt disagreed on with `marks`, the marks that the
    /// actor's scanner found in them. `effects` must be the buffer of `terminal`'s
    /// callbacks; its working directory, reset and step flags are consumed here.
    pub(crate) fn write(
        &mut self,
        terminal: &mut Terminal<'_, '_>,
        effects: &Effects,
        bytes: &[u8],
        base: Seq,
        marks: &[ShellMark],
    ) -> Vec<Disagreement> {
        // An empty chunk changes nothing, not even an OSC that waits for its `\`.
        if bytes.is_empty() {
            return Vec::new();
        }
        let base = base.get();
        // A gap in the recording: the mark of the pending OSC cannot come any more.
        if self.next != Some(base) && self.pending.take().is_some() {
            self.forget(terminal, effects);
        }
        self.next = Some(base.saturating_add(bytes.len() as u64));
        let mut marks = marks.iter().peekable();
        let mut found = Vec::new();
        if let Some(end) = self.pending.take() {
            let mark = marks.next_if(|mark| mark.end.get() == end);
            self.check(terminal, effects, readable(mark, ST_LEN), &mut found);
        }
        let mut written = 0;
        while written < bytes.len() {
            let stop = match terminal.vt_write_until(&bytes[written..], Boundary::OSC_END) {
                Ok(Some(stop)) if stop.consumed > 0 => stop,
                Ok(None) => break,
                // NOTE: only a stop at ground consumes nothing, and OSC_END asks for
                // none. Nothing was consumed, so the rest is written whole.
                Ok(Some(_)) => {
                    terminal.vt_write(&bytes[written..]);
                    break;
                }
                // NOTE: the binding gives an error in two cases. The library refuses a
                // boundary that it does not know before it reads a byte, but OSC_END
                // is one that it knows. Or the stop holds a kind or an end that the
                // binding cannot name, from a library newer than the pin: then the
                // library consumed the bytes up to the stop, and the count is lost. A
                // second write of those bytes would print them and answer their
                // queries twice, so the rest of the chunk is dropped and logged.
                Err(_) => {
                    found.push(Disagreement::UnknownStop {
                        after: base.saturating_add(written as u64),
                    });
                    self.pending = None;
                    self.forget(terminal, effects);
                    return found;
                }
            };
            written += stop.consumed;
            let mut end = base.saturating_add(written as u64);
            let mut terminator = BEL_LEN;
            // The terminal stops after the `ESC` of `ESC \`; the mark ends after the
            // `\`.
            if stop.end == Some(StringEnd::StEsc) {
                end = end.saturating_add(1);
                terminator = ST_LEN;
                if written == bytes.len() {
                    self.pending = Some(end);
                    break;
                }
            }
            while let Some(missed) = marks.next_if(|mark| mark.end.get() < end) {
                found.push(Disagreement::Unread { end: missed.end.get() });
            }
            let mark = marks.next_if(|mark| mark.end.get() == end);
            self.check(terminal, effects, readable(mark, terminator), &mut found);
        }
        for missed in marks {
            found.push(Disagreement::Unread { end: missed.end.get() });
        }
        // A full reset after the last OSC clears the working directory too. The flags
        // of an OSC that waits for its `\` stay for that check.
        if self.pending.is_none() && effects.take_reset() {
            self.pwd = None;
        }
        found
    }

    /// Writes `bytes` without a check, for a feed that comes without marks. The
    /// working directory is read again, so a later check starts from the truth.
    pub(crate) fn write_unchecked(
        &mut self,
        terminal: &mut Terminal<'_, '_>,
        effects: &Effects,
        bytes: &[u8],
    ) {
        terminal.vt_write(bytes);
        self.next = None;
        self.pending = None;
        self.forget(terminal, effects);
    }

    /// Drops what the callbacks reported without a check and reads the working
    /// directory again.
    fn forget(&mut self, terminal: &Terminal<'_, '_>, effects: &Effects) {
        let _ = (effects.take_reset(), effects.take_pwd_changed(), effects.take_step());
        if !self.read_pwd(terminal) {
            self.pwd = None;
        }
    }

    /// Compares `terminal` at the end of one OSC with the mark that ends there, if
    /// any.
    fn check(
        &mut self,
        terminal: &Terminal<'_, '_>,
        effects: &Effects,
        mark: Option<&ShellMarkKind>,
        found: &mut Vec<Disagreement>,
    ) {
        // A full reset clears libghostty-vt's working directory without the callback,
        // so the directory read last is gone too.
        if effects.take_reset() {
            self.pwd = None;
        }
        let pwd_changed = effects.take_pwd_changed();
        let step = effects.take_step();
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
        let Some(ShellMarkKind::SemanticPrompt(event)) = mark else {
            return;
        };
        if let Some(expected) = Step::of_mark(event)
            && step != ReportedStep::Step(expected)
        {
            found.push(Disagreement::Step { expected, actual: step });
        }
        if let SemanticPromptEvent::PromptStart { kind, fresh_line, .. } = event {
            check_prompt(terminal, *kind, *fresh_line, found);
        }
    }

    /// Reads libghostty-vt's working directory into `self.pwd`. False when it cannot
    /// be read or is not UTF-8; `self.pwd` then keeps the last value read.
    fn read_pwd(&mut self, terminal: &Terminal<'_, '_>) -> bool {
        match terminal.pwd().ok().and_then(|pwd| str::from_utf8(pwd).ok()) {
            Some(pwd) => {
                self.pwd = (!pwd.is_empty()).then(|| pwd.to_owned());
                true
            }
            None => false,
        }
    }
}

impl Step {
    /// The step that libghostty-vt reports; `None` for a kind that this crate does
    /// not know.
    pub(crate) fn of(event: SemanticPrompt<'_>) -> Option<Step> {
        match event.kind().ok()? {
            SemanticPromptKind::PromptStart => {
                let kind = match event.prompt_kind().ok()? {
                    effect::PromptKind::Primary => PromptKind::Initial,
                    effect::PromptKind::Right => PromptKind::Right,
                    effect::PromptKind::Continuation => PromptKind::Continuation,
                    effect::PromptKind::Secondary => PromptKind::Secondary,
                    _ => return None,
                };
                Some(Step::PromptStart(kind))
            }
            SemanticPromptKind::InputStart => Some(Step::InputStart),
            SemanticPromptKind::OutputStart => Some(Step::OutputStart),
            SemanticPromptKind::CommandEnd => Some(Step::CommandEnd(event.exit_code())),
            _ => None,
        }
    }

    /// The step of a mark; `None` for a kind that this crate does not know yet.
    fn of_mark(event: &SemanticPromptEvent) -> Option<Step> {
        match event {
            SemanticPromptEvent::PromptStart { kind, .. } => Some(Step::PromptStart(*kind)),
            SemanticPromptEvent::InputStart => Some(Step::InputStart),
            SemanticPromptEvent::OutputStart { .. } => Some(Step::OutputStart),
            SemanticPromptEvent::CommandEnd { exit_code, .. } => Some(Step::CommandEnd(*exit_code)),
            _ => None,
        }
    }
}

/// The length of the terminator `BEL`.
const BEL_LEN: u64 = 1;

/// The length of the terminator `ESC \`.
const ST_LEN: u64 = 2;

/// The kind of `mark`, unless libghostty-vt dropped its OSC for its length: then the
/// terminal holds nothing to compare with the mark. `terminator` is the length of the
/// terminator of that OSC.
///
/// The length counts the C0 controls inside the body, which both parsers ignore, so
/// an OSC near the limit that holds such controls can go unchecked. It never gives a
/// false warning.
fn readable(mark: Option<&ShellMark>, terminator: u64) -> Option<&ShellMarkKind> {
    let mark = mark?;
    // The head is `ESC ]`, the number and its `;`.
    let (head, limit) = match mark.kind {
        ShellMarkKind::CwdChanged { .. } => (b"\x1b]7;".len(), LIBRARY_OSC_BYTES - 1),
        _ => (b"\x1b]133;".len(), LIBRARY_OSC_BYTES),
    };
    let length = mark.end.get().saturating_sub(mark.start.get());
    (length.saturating_sub(head as u64 + terminator) <= limit).then_some(&mark.kind)
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
    let point = Point::Active(Coordinate { x, y: u32::from(y) });
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
            Disagreement::Step { expected, actual } => match actual {
                ReportedStep::Step(actual) => write!(
                    f,
                    "the scanner read OSC 133 {expected:?}, but libghostty-vt reported {actual:?}"
                ),
                ReportedStep::Unknown => write!(
                    f,
                    "the scanner read OSC 133 {expected:?}, but libghostty-vt reported a step \
                     of a kind efr does not know"
                ),
                ReportedStep::Nothing => write!(
                    f,
                    "the scanner read OSC 133 {expected:?}, but libghostty-vt reported no step"
                ),
            },
            Disagreement::Unread { end } => write!(
                f,
                "the scanner read a mark that ends at byte {end}, but libghostty-vt read no \
                 end of an OSC there"
            ),
            Disagreement::UnknownStop { after } => write!(
                f,
                "libghostty-vt stopped at the end of an OSC after byte {after} in a way that \
                 efr cannot read, so the rest of the output chunk was dropped"
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
