//! A tool call of the followed turn: one row in the live zone while it runs, and one
//! row written once when it ends (the Calls board of the terminal design).
//!
//! ```text
//! ⠼ $ cargo test -p app  12s   ctx 38%
//!     test turn::tests::steer_during_turn ... ok
//!
//! ✓ $ cargo test -p app  38s
//! ✗ $ make  exit 2 · 1.5s
//!     make: *** No rule to make target 'all'.  Stop.
//! ! $ curl -sI https://example.com  refused · no network in the sandbox
//! · $ cargo update -p rusqlite  did not run
//! → $ cargo test --workspace  moved to job j4 after 42s
//!   job j4 · output $SCRATCH/jobs/j4.out · ends in 60 min
//! ```
//!
//! The first column is a scan line: one glyph for each call (`Glyphs::ok`, `failed`,
//! `refused`, `denied`, `moved`, or the spinner while it runs). While the call runs,
//! its row carries the spinner (`accent`), the name of the call (`$`, `read`, ...) in
//! `muted`, what it does in `text`, cut to the width with `…`, from 1 s on how long it
//! has run after two spaces, the tag `full` in `error` in a full turn and the gauge of
//! the model's context after three spaces each. The last [`TAIL_LINES`] lines of its
//! output with text in them follow, 4 columns in, `muted`. The status row hides
//! meanwhile, so the screen shows one sign of work. A call whose approval waits shows
//! nothing yet: the question is on the screen.
//!
//! When the call ends, one row takes the place of the live rows:
//!
//! - A call that went well: `✓` in `success`, the rest `muted`, the time after two
//!   spaces from a tenth of a second on. A file tool shows no time.
//! - A failed call: a bold `✗` in `error`, the name `muted`, what it ran in `text`, then
//!   `exit 2` or `failed` in `error` and ` · ` and the time, which always shows. The last
//!   lines of its output follow, 4 columns in, in `text`.
//! - A call that efr refused before it ran: `!` and `refused` in `warning`, ` · ` and why
//!   in `muted`. A sandbox that could not start says so in the same shape.
//! - A call whose approval was denied: `·` and the whole row in `muted`, `did not run`.
//! - A call that the user moved to a job with Ctrl+B: `→` in `accent`, the row `muted`,
//!   `moved to job j4 after 42s`. A call whose command still runs in the hidden shell
//!   has the same mark and says so.
//!
//! A command of several lines shows each line, the next ones under the first. A line
//! wider than the screen goes on in the next row: after a cut at a space, which ends in
//! a muted `\`, 4 columns after the column of the name (column 6); after a cut inside a
//! word, which ends in a muted `↩`, at the column of the row it goes on from. The time
//! follows the last row.
//!
//! A file write names its file in `text` and the lines that it added (`success`) and
//! removed (`error`), and its diff follows (`format::patch`):
//!
//! ```text
//! ✓ edit src/turn.rs  +5 -1
//!   @@ 88 impl Turn
//!   -     self.state = State::Done;
//!   +     while let Ok(msg) = self.rx.try_recv() {
//!   … 14 more lines · efr diff
//! ✓ write notes.md new  +12
//! ✓ delete src/cache.rs  -212
//!   deleted; its lines are in efr diff
//! ✓ move src/expr.rs → src/expression.rs  +1 -1
//! ✓ patch 3 files  +9 -2
//!   src/turn.rs  +5 -1
//!     @@ 131 fn steer
//!     … 4 more lines
//! ```
//!
//! A call that changed files and shows no diff, such as a shell call, gets one muted
//! row of the files under it, 2 columns in, with the added lines in the `success` role
//! and the removed lines in the `error` role. A connection that the proxy blocked gets
//! a muted row with the host in `warning`.

use std::time::Duration;

use efr_protocol::{Blocked, CallId, ChangeKind, ContextUse, FileChanges, JobId};
use efr_render::{RenderOptions, text_width};
use jiff::Timestamp;

use super::status::{FULL, gauge_room};
use crate::follow::since_then;
use crate::format::changes::Piece;
use crate::format::patch::{self, DiffFile, Op};
use crate::format::{self, CallText, CommandRow, RowEnd, Tone};

/// The lines of output that a running call shows and a failed one keeps.
pub(crate) const TAIL_LINES: usize = 3;

/// The time from which a running call says how long it has run: its time counts whole
/// seconds.
const SHOW_TIME: Duration = Duration::from_secs(1);

/// The time from which the row of a call that went well says how long it ran: a
/// shorter one would read `0.0s` (TM-Calls 14 and 17 and TM-LongTurn 3 show `0.1s`,
/// `0.4s`; a call of a few milliseconds shows none, as in TM-Sequence 4).
const ROW_TIME: Duration = Duration::from_millis(100);

/// What starts a line of a call's output: 4 columns in, under what the call does.
pub(crate) const TAIL_INDENT: &str = "    ";

/// What starts the rows under a call: the files it changed, its notes, its diff.
const INDENT: &str = "  ";

/// How many columns after the column of the name (`$`) a row of a command goes on after
/// a cut at a space.
const GOES_ON: usize = 4;

/// How much further a row of text that goes on, such as a long note, is indented than
/// its first row, as in a question's card.
const GOES_ON_TEXT: usize = 2;

/// The lines after the first that a running call shows. With more, the last of these
/// rows says how many lines follow.
const LIVE_LINES: usize = 3;

/// The spaces between what a call does and its time, as between the fields of a row of
/// efr's own that are not the same piece.
const GAP: &str = "  ";

/// The spaces before the gauge and the tag `full` of a running call, as between the
/// fields of the status row.
const FIELD_GAP: &str = "   ";

/// The words of a call that efr ran and that gave a result without its exit code.
const FAILED: &str = "failed";

/// The words of a call that efr refused before it ran.
const REFUSED: &str = "refused";

/// The words of a call whose approval was denied or expired.
const DID_NOT_RUN: &str = "did not run";

/// The row under a file that a patch deleted: its diff would only be its old lines.
pub(crate) const DELETED: &str = "deleted; its lines are in efr diff";

/// The name of the shell tool's call, whose lines are commands.
const SHELL: &str = "$";

/// The name of the read tool's call, whose calls fold into one row (`fold`).
pub(crate) const READ: &str = "read";

/// The words of a file tool's call that the row names a file with, and that shows the
/// counts of its lines in place of a time.
const FILE_WORDS: &[&str] = &[READ, "write", "edit", "delete", "move", "patch", "apply_patch"];

/// How a call ended, for its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome<'a> {
    /// It ran and went well.
    Ran,
    /// Its command exited with this code.
    Exited(i32),
    /// The tool failed without an exit code.
    Failed,
    /// efr refused it before it ran, for this reason.
    Refused(&'a str),
    /// Its sandbox could not start, for this reason.
    NotStarted(&'a str),
    /// It answered the model while its command still runs in the hidden shell
    /// (`left_running`), such as after its timeout.
    LeftRunning,
    /// The user moved it to this background job with Ctrl+B: its command runs on there.
    Moved(&'a JobId),
    /// Its approval was denied or expired, so it never ran.
    Denied,
}

impl Outcome<'_> {
    /// True when the call failed, so its last output lines stay on the screen.
    pub(crate) fn failed(self) -> bool {
        matches!(self, Outcome::Exited(_) | Outcome::Failed)
    }

    /// True when the last output lines stay on the screen under the call: it failed, or
    /// its command still runs, and its last line may be the prompt that it waits at.
    pub(crate) fn keeps_tail(self) -> bool {
        self.failed() || self == Outcome::LeftRunning
    }

    /// The mark of the scan column and its tone.
    fn mark(self, options: &RenderOptions) -> Piece {
        let glyphs = options.glyphs();
        match self {
            Outcome::Ran => (glyphs.ok.to_owned(), Tone::Success),
            Outcome::Exited(_) | Outcome::Failed => (glyphs.failed.to_owned(), Tone::FailureBold),
            Outcome::Refused(_) | Outcome::NotStarted(_) => {
                (glyphs.refused.to_owned(), Tone::Attention)
            }
            Outcome::Denied => (glyphs.denied.to_owned(), Tone::Dim),
            // NOTE: no check mark: the command did not end. It runs on elsewhere, as a
            // job or in the hidden shell, whose end comes later as a line of its own.
            Outcome::Moved(_) | Outcome::LeftRunning => (glyphs.moved.to_owned(), Tone::Accent),
        }
    }

    /// The tone of what the call does: `text` where the row asks to be read (a failure,
    /// a refusal), `muted` where it only says what happened.
    fn text_tone(self) -> Tone {
        match self {
            Outcome::Exited(_) | Outcome::Failed | Outcome::Refused(_) | Outcome::NotStarted(_) => {
                Tone::Text
            }
            _ => Tone::Dim,
        }
    }
}

/// What a file write changed, for the row of its call: `✓ edit src/turn.rs  +5 -1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edit {
    /// The word of the row: `write`, `edit`, `delete`, `move` or `patch`.
    pub(crate) word: &'static str,
    /// The file, safe to print, or for a patch of several files how many: `3 files`.
    pub(crate) path: String,
    /// Where a moved file went.
    pub(crate) to: Option<String>,
    /// The file is new.
    pub(crate) new: bool,
    /// The lines added and removed, when they are known.
    pub(crate) counts: Option<(u32, u32)>,
}

impl Edit {
    /// True when the call deleted its one file: the row under it says so in place of a
    /// diff.
    pub(crate) fn deleted(&self) -> bool {
        self.word == "delete"
    }
}

/// A tool call of the followed turn, from its start to its end.
#[derive(Debug)]
pub(crate) struct Call {
    pub(crate) call_id: CallId,
    text: CallText,
    /// The daemon's time when it began to run: its start, or the answer to its
    /// approval.
    since: Option<Timestamp>,
    /// The time of the first frame that showed it running.
    shown: Option<Timestamp>,
    /// Its approval waits for an answer, so it does not run yet.
    pub(crate) awaiting: bool,
    /// Its first rows wait for stderr when stdout is not a terminal. The row of a call
    /// says how it ended, so it is written once at its end and nothing waits: the view
    /// leaves this false (`due_header`).
    pub(crate) header_due: bool,
    /// It starts a background job (`background: true`): its row says so after its time.
    background: bool,
}

impl Call {
    /// Call `call_id`, which does `text`, started at `since` on the daemon's clock.
    pub(crate) fn new(call_id: CallId, text: CallText, since: Option<Timestamp>) -> Call {
        Call {
            call_id,
            text,
            since,
            shown: None,
            awaiting: false,
            header_due: false,
            background: false,
        }
    }

    /// The same call, which starts a background job.
    pub(crate) fn in_background(mut self) -> Call {
        self.background = true;
        self
    }

    /// True when the call starts a background job (`background: true`).
    pub(crate) fn background(&self) -> bool {
        self.background
    }

    /// How long the call has shown as running at `now`: from the first frame that
    /// showed it, as its line counts.
    pub(crate) fn ran_for(&self, now: Timestamp) -> Duration {
        self.shown.map_or(Duration::ZERO, |shown| since_then(shown, now))
    }

    /// Its approval was answered at `at`: the call runs from then.
    pub(crate) fn approved(&mut self, at: Option<Timestamp>) {
        self.awaiting = false;
        if at.is_some() {
            self.since = at;
        }
        self.shown = None;
    }

    /// The frame at `now` shows the call: its time counts from the first such frame.
    pub(crate) fn show(&mut self, now: Timestamp) {
        self.shown.get_or_insert(now);
    }

    /// The first line of what the call does: the path of a file tool's call.
    pub(crate) fn subject(&self) -> Option<&str> {
        self.text.lines.first().map(String::as_str)
    }

    /// True for a call of the read tool, whose rows fold into one (`fold`).
    pub(crate) fn reads(&self) -> bool {
        self.text.name == READ
    }

    /// True for a call of a file tool: its row shows no time.
    fn file_tool(&self) -> bool {
        FILE_WORDS.contains(&self.text.name.as_str())
    }

    /// How long the call ran until `end` on the daemon's clock, when both times are
    /// known.
    fn took(&self, end: Option<Timestamp>) -> Option<Duration> {
        self.since.zip(end).map(|(since, end)| since_then(since, end))
    }

    /// The name of the call and what it does, as its rows show them: a patch of one
    /// file names it with the word of what it does (`edit`, `write`, `delete`, `move`),
    /// a patch of several files counts them.
    fn named(&self) -> (String, Vec<String>) {
        if self.text.name == patch::TOOL {
            let files = &self.text.files;
            if let [file] = files.as_slice() {
                let path = format::one_line(&file.path);
                return match &file.op {
                    Op::Update => ("edit".to_owned(), vec![path]),
                    Op::Add => ("write".to_owned(), vec![path]),
                    Op::Delete => ("delete".to_owned(), vec![path]),
                    Op::Move(to) => ("move".to_owned(), vec![path, format::one_line(to)]),
                };
            }
            if !files.is_empty() {
                return ("patch".to_owned(), vec![files_count(files.len())]);
            }
        }
        (self.text.name.clone(), self.text.lines.clone())
    }

    /// What the call wrote, for its row: its word, its file, whether the file is new and
    /// the lines it added and removed, from `changes` (what the daemon's snapshots saw),
    /// else from `diff`. `None` for a call that is no file write.
    pub(crate) fn edit(&self, changes: Option<&FileChanges>, diff: Option<&str>) -> Option<Edit> {
        let (word, mut lines) = self.named();
        let word: &'static str = match word.as_str() {
            "write" => "write",
            "edit" => "edit",
            "delete" => "delete",
            "move" => "move",
            "patch" => "patch",
            _ => return None,
        };
        let changes = changes.filter(|changes| !changes.files.is_empty());
        let counts = match (changes, diff) {
            (Some(changes), _) => Some((changes.added, changes.removed)),
            (None, Some(diff)) => Some(patch::count(diff)),
            (None, None) if word == "patch" => {
                Some(self.text.files.iter().fold((0_u32, 0_u32), |(added, removed), file| {
                    (added.saturating_add(file.added), removed.saturating_add(file.removed))
                }))
            }
            (None, None) => None,
        };
        let new = word == "write"
            && (changes.is_some_and(|changes| {
                changes.files.iter().all(|file| file.kind == ChangeKind::Added)
            }) || diff.is_some_and(|diff| {
                patch::file_diffs(diff)
                    .first()
                    .is_some_and(|file| matches!(file.file, DiffFile::Add(_)))
            }));
        let to = (word == "move" && lines.len() > 1).then(|| lines.remove(1));
        let path = lines.into_iter().next().unwrap_or_default();
        Some(Edit { word, path, to, new, counts })
    }

    /// The rows of the running call at `now`, with `spinner`, the tag `full` when `full`,
    /// the gauge of `context` and their newlines: each cut to the width with `…` at the
    /// cut.
    pub(crate) fn running(
        &self,
        spinner: char,
        now: Timestamp,
        full: bool,
        context: Option<&ContextUse>,
        options: &RenderOptions,
    ) -> String {
        let elapsed = self.ran_for(now);
        let time = (elapsed >= SHOW_TIME).then(|| format::elapsed(elapsed));
        let method = options.width_method();
        let width = usize::from(options.width());
        let (name, lines) = self.named();
        // The spinner and a space, the name and a space, the time after two spaces, and
        // `full` and the gauge after three each.
        let mut reserve = 2 + text_width(&name, method) + 1;
        if let Some(time) = &time {
            reserve += GAP.len() + text_width(time, method);
        }
        if full {
            reserve += FIELD_GAP.len() + FULL.len();
        }
        let mut room = width.saturating_sub(reserve);
        // NOTE: `gauge_room` counts two spaces before the gauge; the row has three.
        let spare = room.saturating_sub(1);
        let mut left = spare;
        let gauge = gauge_room(context, &mut left, options);
        if gauge.is_some() {
            room = left;
        }
        let mut out = format::paint(&spinner.to_string(), Tone::Accent, options);
        out.push(' ');
        out.push_str(&format::paint(&name, Tone::Dim, options));
        if let Some(head) = lines.first() {
            out.push(' ');
            out.push_str(&format::paint(&format::cut(head, room, method), Tone::Text, options));
        }
        if let Some(time) = time {
            out.push_str(GAP);
            out.push_str(&format::paint(&time, Tone::Dim, options));
        }
        if full {
            out.push_str(FIELD_GAP);
            out.push_str(&format::paint(FULL, Tone::Failure, options));
        }
        if let Some(gauge) = gauge {
            out.push_str(FIELD_GAP);
            out.push_str(&gauge.paint(options));
        }
        out.push('\n');
        // NOTE: a moved file's second line is where it goes, which the head names.
        if self.text.name == patch::TOOL {
            return out;
        }
        let indent = " ".repeat(self.text_column(&name));
        let room = width.saturating_sub(indent.len());
        let more = &lines[1.min(lines.len())..];
        // A note in place of one line would take its row and show less.
        let shown = if more.len() <= LIVE_LINES { more.len() } else { LIVE_LINES - 1 };
        for line in &more[..shown] {
            out.push_str(&indent);
            out.push_str(&format::paint(&format::cut(line, room, method), Tone::Text, options));
            out.push('\n');
        }
        let hidden = more.len() - shown;
        if hidden > 0 {
            let unit = if hidden == 1 { "line" } else { "lines" };
            out.push_str(&indent);
            out.push_str(&format::paint(&format!("({hidden} more {unit})"), Tone::Dim, options));
            out.push('\n');
        }
        out
    }

    /// The column where the text of each line starts: after the mark and the name.
    fn text_column(&self, name: &str) -> usize {
        // NOTE: every mark of the scan column is one column wide.
        1 + 1 + name.chars().count() + 1
    }

    /// The rows that name the call before it ends, without a mark of how it ended: for
    /// the view's `due_header`, which nothing asks for any more ([`Call::header_due`]).
    pub(crate) fn header(&self, options: &RenderOptions) -> String {
        let (name, lines) = self.named();
        let body = Body::Command { lines: &lines, tone: Tone::Text, command: name == SHELL };
        let mark = (options.glyphs().denied.to_owned(), Tone::Dim);
        layout(mark, &name, body, Vec::new(), options)
    }

    /// The row of the call that ended at `end` on the daemon's clock as `outcome` says,
    /// with its newline: what it wrote (`edit`) for a file write that went well.
    pub(crate) fn row(
        &self,
        end: Option<Timestamp>,
        outcome: Outcome<'_>,
        edit: Option<&Edit>,
        options: &RenderOptions,
    ) -> String {
        let (name, lines) = self.named();
        let tone = outcome.text_tone();
        let mut suffix: Vec<Piece> = Vec::new();
        let took = self.took(end);
        let separator = options.glyphs().separator;
        match outcome {
            Outcome::Ran => {
                if let Some(took) = took.filter(|took| *took >= ROW_TIME && !self.file_tool()) {
                    suffix.push((GAP.to_owned(), Tone::Plain));
                    suffix.push((took_text(took), Tone::Dim));
                }
                if self.background {
                    suffix.push((GAP.to_owned(), Tone::Plain));
                    suffix.push((BACKGROUND.to_owned(), Tone::Dim));
                }
            }
            Outcome::Exited(_) | Outcome::Failed => {
                let what = match outcome {
                    Outcome::Exited(code) => format!("exit {code}"),
                    _ => FAILED.to_owned(),
                };
                suffix.push((GAP.to_owned(), Tone::Plain));
                suffix.push((what, Tone::Failure));
                if let Some(took) = took {
                    suffix.push((" ".to_owned(), Tone::Plain));
                    suffix.push((format!("{separator} {}", took_text(took)), Tone::Dim));
                }
            }
            Outcome::Refused(reason) | Outcome::NotStarted(reason) => {
                suffix.push((GAP.to_owned(), Tone::Plain));
                suffix.push((REFUSED.to_owned(), Tone::Attention));
                let reason = format::one_line(reason);
                let why = match outcome {
                    Outcome::NotStarted(_) => {
                        format!("the sandbox could not start: {reason}; efr checks it again")
                    }
                    _ => reason,
                };
                if !why.trim().is_empty() {
                    suffix.push((" ".to_owned(), Tone::Plain));
                    suffix.push((format!("{separator} {why}"), Tone::Dim));
                }
            }
            Outcome::Denied => {
                suffix.push((GAP.to_owned(), Tone::Plain));
                suffix.push((DID_NOT_RUN.to_owned(), Tone::Dim));
            }
            Outcome::Moved(job) => {
                let mut words = format!("moved to job {}", format::one_line(job.as_str()));
                if let Some(took) = took.filter(|took| *took >= ROW_TIME) {
                    words.push_str(&format!(" after {}", took_text(took)));
                }
                suffix.push((GAP.to_owned(), Tone::Plain));
                suffix.push((words, Tone::Dim));
            }
            Outcome::LeftRunning => {
                suffix.push((GAP.to_owned(), Tone::Plain));
                suffix.push((LEFT_RUNNING.to_owned(), Tone::Dim));
            }
        }
        let mark = outcome.mark(options);
        if let Some(edit) = edit.filter(|_| outcome == Outcome::Ran) {
            return layout(
                mark,
                edit.word,
                Body::Spans(edit_pieces(edit, options)),
                suffix,
                options,
            );
        }
        let body = match lines.as_slice() {
            // NOTE: a moved file reads `move a → b`, with the arrow muted.
            [from, to] if name == "move" => Body::Spans(vec![
                (from.clone(), tone),
                (format!(" {} ", options.glyphs().arrow), Tone::Dim),
                (to.clone(), tone),
            ]),
            _ => Body::Command { lines: &lines, tone, command: name == SHELL },
        };
        layout(mark, &name, body, suffix, options)
    }
}

/// How long a call ran, as its row says it: tenths of a second under 10 s, without
/// `.0` from 1 s on (`0.4s`, `7.8s`, `9s`), else as the status row counts (`38s`, `2m
/// 12s`).
fn took_text(took: Duration) -> String {
    let text = format::took(took);
    match text.strip_suffix(".0s") {
        Some(whole) if whole != "0" => format!("{whole}s"),
        _ => text,
    }
}

/// `3 files`, or `1 file`.
fn files_count(count: usize) -> String {
    if count == 1 { "1 file".to_owned() } else { format!("{count} files") }
}

/// The pieces of a file write after its word: the file in `text` (`a → b` for a move,
/// with the arrow muted), a muted `new` after a new file, then two spaces and the
/// counts.
fn edit_pieces(edit: &Edit, options: &RenderOptions) -> Vec<Piece> {
    let mut pieces = vec![(edit.path.clone(), Tone::Text)];
    if let Some(to) = &edit.to {
        pieces.push((format!(" {} ", options.glyphs().arrow), Tone::Dim));
        pieces.push((to.clone(), Tone::Text));
    }
    if edit.new {
        pieces.push((" ".to_owned(), Tone::Plain));
        pieces.push(("new".to_owned(), Tone::Dim));
    }
    if let Some((added, removed)) = edit.counts {
        let counts = format::changes::counts(added, removed);
        if !counts.is_empty() {
            pieces.push((GAP.to_owned(), Tone::Plain));
            pieces.extend(counts);
        }
    }
    pieces
}

/// What a row of a call names after its name.
enum Body<'a> {
    /// The lines of what the call does, in `tone`: a shell command (`command`) is cut
    /// with the marks of a command (`format::wrap_command`), other text at its spaces.
    Command { lines: &'a [String], tone: Tone, command: bool },
    /// Pieces with their own tones, cut at their spaces.
    Spans(Vec<Piece>),
}

/// The rows of a call: `mark`, a space, `name`, a space, then `body`, and `suffix` after
/// the last row, each row with its newline. On a terminal a row wider than the screen
/// goes on in the next one: see [`Body`] and the module. Elsewhere each line of a
/// command is one row.
fn layout(
    mark: Piece,
    name: &str,
    body: Body<'_>,
    suffix: Vec<Piece>,
    options: &RenderOptions,
) -> String {
    let glyphs = options.glyphs();
    let mut out = format::paint(&mark.0, mark.1, options);
    out.push(' ');
    out.push_str(&format::paint(name, Tone::Dim, options));
    let method = options.width_method();
    let column = text_width(&mark.0, method) + 1 + text_width(name, method) + 1;
    let goes_on = text_width(&mark.0, method) + 1 + GOES_ON;
    let columns = format::columns(options);
    let suffix_width: usize = suffix.iter().map(|(text, _)| text_width(text, method)).sum();
    match body {
        Body::Spans(mut pieces) => {
            if pieces.iter().all(|(text, _)| text.is_empty()) {
                pieces.clear();
            } else {
                out.push(' ');
            }
            pieces.extend(suffix);
            let rows = match columns {
                Some((width, method)) => format::wrap_spans(
                    &pieces,
                    width.saturating_sub(column).max(1),
                    width.saturating_sub(goes_on).max(1),
                    method,
                ),
                None => vec![pieces],
            };
            for (at, row) in rows.iter().enumerate() {
                if at > 0 {
                    out.push_str(&" ".repeat(goes_on));
                }
                push_pieces(&mut out, row, options);
                out.push('\n');
            }
        }
        Body::Command { lines, tone, command } => {
            if lines.is_empty() {
                push_pieces(&mut out, &suffix, options);
                out.push('\n');
                return out;
            }
            out.push(' ');
            let mut rows: Vec<(usize, CommandRow)> = Vec::new();
            for (at, line) in lines.iter().enumerate() {
                let last = at + 1 == lines.len();
                let wrapped = match columns {
                    Some((width, method)) => {
                        let room = width.saturating_sub(column).max(1);
                        let indent = goes_on.saturating_sub(column);
                        let wrap = |room: usize| -> Vec<CommandRow> {
                            if command {
                                format::wrap_command(line, room, indent, method)
                            } else {
                                plain_rows(line, room, method)
                            }
                        };
                        let mut wrapped = wrap(room);
                        // NOTE: the suffix follows the last row; a row that has no room
                        // for it is cut shorter, when that leaves at least half the row.
                        if last && !suffix.is_empty() {
                            let end = wrapped.last().map_or(column, |row| {
                                let at = if row.indented { goes_on } else { column };
                                at + text_width(&row.text, method)
                            });
                            if end + suffix_width > width && room > suffix_width * 2 {
                                wrapped = wrap(room - suffix_width);
                            }
                        }
                        wrapped
                    }
                    None => {
                        vec![CommandRow { text: line.clone(), indented: false, end: RowEnd::Last }]
                    }
                };
                for (at_row, row) in wrapped.into_iter().enumerate() {
                    let at_column = if row.indented {
                        goes_on
                    } else if at == 0 && at_row == 0 {
                        0
                    } else {
                        column
                    };
                    rows.push((at_column, row));
                }
            }
            let mut end = 0;
            for (at, (at_column, row)) in rows.iter().enumerate() {
                if at > 0 {
                    out.push_str(&" ".repeat(*at_column));
                    end = *at_column;
                } else {
                    end = column;
                }
                out.push_str(&format::paint(&row.text, tone, options));
                end += text_width(&row.text, method);
                if let Some(mark) = row.mark_in(glyphs) {
                    out.push_str(&format::paint(mark, Tone::Dim, options));
                    end += text_width(mark, method);
                }
                if at + 1 < rows.len() {
                    out.push('\n');
                }
            }
            let fits = columns.is_none_or(|(width, _)| end + suffix_width <= width);
            if fits {
                push_pieces(&mut out, &suffix, options);
                out.push('\n');
            } else {
                out.push('\n');
                let suffix: Vec<Piece> =
                    suffix.into_iter().skip_while(|(text, _)| text.trim().is_empty()).collect();
                let rows = match columns {
                    Some((width, method)) => {
                        let room = width.saturating_sub(goes_on).max(1);
                        format::wrap_spans(&suffix, room, room, method)
                    }
                    None => vec![suffix],
                };
                for row in rows {
                    out.push_str(&" ".repeat(goes_on));
                    push_pieces(&mut out, &row, options);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// The row under a file that the call deleted, in place of its diff.
pub(crate) fn deleted(options: &RenderOptions) -> String {
    format!("{INDENT}{}\n", format::paint(DELETED, Tone::Dim, options))
}

/// The row of calls of `word` that went well and fold into one (`fold`), such as `✓
/// read 6 files  lib.rs, session.rs, json.rs (+3)`: the mark in `success`, the rest
/// muted.
pub(crate) fn folded(word: &str, text: &str, options: &RenderOptions) -> String {
    let mark = Outcome::Ran.mark(options);
    layout(mark, word, Body::Spans(vec![(text.to_owned(), Tone::Dim)]), Vec::new(), options)
}

/// `line` as rows of `room` columns, cut at spaces and with no marks, for what is no
/// command, such as a path or a URL.
fn plain_rows(line: &str, room: usize, method: efr_render::WidthMethod) -> Vec<CommandRow> {
    let pieces = [(line.to_owned(), Tone::Plain)];
    format::wrap_spans(&pieces, room, room, method)
        .into_iter()
        .map(|row| CommandRow {
            text: row.into_iter().map(|(text, _)| text).collect(),
            indented: false,
            end: RowEnd::Last,
        })
        .collect()
}

/// Paints `pieces` onto `out`.
fn push_pieces(out: &mut String, pieces: &[Piece], options: &RenderOptions) {
    for (text, tone) in pieces {
        out.push_str(&format::paint(text, *tone, options));
    }
}

/// The result of a call whose command still runs in the hidden shell.
pub(crate) const LEFT_RUNNING: &str = "still runs in the hidden shell";

/// The mark after the time of a call that starts a background job.
const BACKGROUND: &str = "(background)";

/// Muted rows of notes about a call under its row, each 2 columns in, such as the line
/// of the job it started.
pub(crate) fn notes(lines: &[Vec<Piece>], options: &RenderOptions) -> String {
    lines.iter().map(|pieces| indented_pieces(pieces, options)).collect()
}

/// A note as one muted piece.
pub(crate) fn note(text: String) -> Vec<Piece> {
    vec![(text, Tone::Dim)]
}

/// The row of a connection that efr's proxy blocked: `network: blocked
/// registry.npmjs.org:443 (not on the allow list) · 3 times`, muted with the host in
/// `warning`.
pub(crate) fn blocked(blocked: &Blocked, options: &RenderOptions) -> Vec<Piece> {
    let host = format!("{}:{}", format::one_line(&blocked.host), blocked.port);
    let mut rest = format!(" ({})", format::sandbox::block_reason(blocked.reason));
    if blocked.count > 1 {
        rest.push_str(&format!(" {} {} times", options.glyphs().separator, blocked.count));
    }
    vec![("network: blocked ".to_owned(), Tone::Dim), (host, Tone::Attention), (rest, Tone::Dim)]
}

/// The rows of the connections that efr's proxy blocked, then `network: and N more
/// refused` for those that the list leaves out.
pub(crate) fn blocked_rows(
    list: &[Blocked],
    more: u32,
    options: &RenderOptions,
) -> Vec<Vec<Piece>> {
    let mut rows: Vec<Vec<Piece>> = list.iter().map(|entry| blocked(entry, options)).collect();
    if more > 0 {
        rows.push(note(format!("network: and {more} more refused")));
    }
    rows
}

/// The row of the files that a call changed, under its row: muted, with the added
/// lines in the `success` role and the removed ones in the `error` role, as rows that
/// fit the screen. Nothing when it changed none.
pub(crate) fn changes(changes: &FileChanges, options: &RenderOptions) -> String {
    let pieces = format::changes::call_row(changes, options);
    if pieces.is_empty() {
        return String::new();
    }
    indented_pieces(&pieces, options)
}

/// The rows of a file write's diff under the call's row, 2 columns in: at most `limit`
/// lines of each file from its first hunk on (`format::patch::layout`), then a muted
/// `… N more lines · efr diff` for the lines left out. The file headers stay out,
/// because the row names the file; they only give the file's syntax colours, as `path`,
/// the file that the call writes, does for a diff without them. A diff of several
/// files, such as the one of an `apply_patch` call, shows one row for each file at 2
/// columns, the file in `text` with its counts, and its diff at 4 columns.
pub(crate) fn diff(
    diff: &str,
    path: Option<&str>,
    limit: usize,
    changes: Option<&FileChanges>,
    options: &RenderOptions,
) -> String {
    let files = patch::file_diffs(diff);
    if files.len() < 2 {
        let parts = format::changes::diff_parts(diff);
        return patch::layout(&parts, path, limit, INDENT, true, options);
    }
    let inner = format!("{INDENT}{INDENT}");
    let mut out = String::new();
    for file in &files {
        let mut pieces = file_pieces(&file.file, options);
        // NOTE: the daemon may cut the diff of a file; the counts of its snapshots are
        // whole.
        let counted = file
            .file
            .path()
            .and_then(|path| changes.and_then(|changes| file_counts(changes, path)))
            .unwrap_or_else(|| patch::count_parts(&file.parts));
        let counts = format::changes::counts(counted.0, counted.1);
        if !counts.is_empty() {
            if !pieces.is_empty() {
                pieces.push((GAP.to_owned(), Tone::Plain));
            }
            pieces.extend(counts);
        }
        if !pieces.is_empty() {
            out.push_str(&indented_pieces(&pieces, options));
        }
        if matches!(file.file, DiffFile::Delete(_)) {
            out.push_str(&inner);
            out.push_str(&format::paint(DELETED, Tone::Dim, options));
            out.push('\n');
            continue;
        }
        out.push_str(&patch::layout(&file.parts, file.file.path(), limit, &inner, false, options));
    }
    out
}

/// The lines added and removed in the file at `path`, as `changes` counts them: the file
/// whose path is `path`, or ends with it after a `/`, or the other way round.
fn file_counts(changes: &FileChanges, path: &str) -> Option<(u32, u32)> {
    let same = |known: &str| {
        known == path
            || known.strip_suffix(path).is_some_and(|start| start.ends_with('/'))
            || path.strip_suffix(known).is_some_and(|start| start.ends_with('/'))
    };
    changes.files.iter().find(|file| same(&file.path)).map(|file| (file.added, file.removed))
}

/// The row that names one file of a patch of several: the path in `text`, a muted
/// `new` before a new file, `delete` before a deleted one, `move a → b` for a moved one.
fn file_pieces(file: &DiffFile, options: &RenderOptions) -> Vec<Piece> {
    let arrow = options.glyphs().arrow;
    match file {
        DiffFile::Unknown => Vec::new(),
        DiffFile::Update(path) => vec![(format::one_line(path), Tone::Text)],
        DiffFile::Add(path) => {
            vec![("new ".to_owned(), Tone::Dim), (format::one_line(path), Tone::Text)]
        }
        DiffFile::Delete(path) => {
            vec![("delete ".to_owned(), Tone::Dim), (format::one_line(path), Tone::Text)]
        }
        DiffFile::Move { from, to } => vec![
            ("move ".to_owned(), Tone::Dim),
            (format::one_line(from), Tone::Text),
            (format!(" {arrow} "), Tone::Dim),
            (format::one_line(to), Tone::Text),
        ],
    }
}

/// Pieces of text with their tones after [`INDENT`], as rows that fit the screen: a
/// text wider than the screen goes on in the next row, indented 2 more columns, so the
/// terminal never wraps a row of efr's own and nothing is cut. Elsewhere than on a
/// terminal no screen sets a width, so the text stays on one row.
fn indented_pieces(pieces: &[Piece], options: &RenderOptions) -> String {
    let rows: Vec<Vec<Piece>> = match format::columns(options) {
        Some((width, method)) => {
            let first = width.saturating_sub(INDENT.len()).max(1);
            let rest = width.saturating_sub(INDENT.len() + GOES_ON_TEXT).max(1);
            format::wrap_spans(pieces, first, rest, method)
        }
        None => vec![pieces.to_vec()],
    };
    let mut out = String::new();
    for (at, row) in rows.iter().enumerate() {
        out.push_str(INDENT);
        if at > 0 {
            out.push_str(&" ".repeat(GOES_ON_TEXT));
        }
        push_pieces(&mut out, row, options);
        out.push('\n');
    }
    out
}

/// The last lines of output of a running call, 4 columns in, muted, without the spaces
/// at their start and cut to the width on a terminal. The prompt of a command that
/// waits for input stays muted too: the answer line under it is the one row in the
/// warning colour (TM-Live 21 to 23, "one warning colour at a time").
pub(crate) fn tail(lines: &[String], options: &RenderOptions) -> String {
    let lines: Vec<String> = lines.iter().map(|line| line.trim_start().to_owned()).collect();
    tail_in(&lines, Tone::Dim, options)
}

/// The last lines of output of a failed call, 4 columns in, in `text`, with the spaces
/// at their start, as the output wrote them (`   --> src/a.rs:9:9`).
pub(crate) fn failed_tail(lines: &[String], options: &RenderOptions) -> String {
    tail_in(lines, Tone::Text, options)
}

/// [`tail`] in `tone`, the lines as they are.
fn tail_in(lines: &[String], tone: Tone, options: &RenderOptions) -> String {
    let method = options.width_method();
    let room = usize::from(options.width()).saturating_sub(TAIL_INDENT.len());
    let mut out = String::new();
    for line in lines {
        // NOTE: elsewhere than on a terminal no screen sets a width, so nothing is cut.
        let line =
            if options.is_terminal() { format::cut(line, room, method) } else { line.clone() };
        out.push_str(TAIL_INDENT);
        out.push_str(&format::paint(&line, tone, options));
        out.push('\n');
    }
    out
}

/// The last [`TAIL_LINES`] lines of `output` with text in them, without the spaces at
/// their end and safe to print. The spaces at their start stay: a failed call keeps
/// them, as in `   --> src/a.rs:9:9` under an error.
pub(crate) fn last_lines(output: &str) -> Vec<String> {
    let mut lines: Vec<String> = output
        .lines()
        .rev()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .take(TAIL_LINES)
        .map(format::one_line)
        .collect();
    lines.reverse();
    lines
}

/// The last lines of a tool's `output` as the model got it, for a call whose output
/// showed no line while it ran: the notes that efr adds at its end in brackets, such
/// as `[exit code 2, cwd /home/me]`, are left out.
pub(crate) fn output_lines(output: &str) -> Vec<String> {
    let mut lines: Vec<&str> = output.lines().collect();
    while lines.last().is_some_and(|line| {
        let line = line.trim();
        line.is_empty() || (line.starts_with('[') && line.ends_with(']'))
    }) {
        lines.pop();
    }
    last_lines(&lines.join("\n"))
}

/// The last lines that a finished call shows, when `shown` are the lines that its
/// updates brought while it ran and `output` is its output as the model got it.
///
/// An update can lag behind the command: its last line can stop in the middle, such as
/// `ls: cannot access '/x'` without the reason after it, and lines can still be missing.
/// So when `output` holds the last shown line, whole or as the start of a line, the
/// last lines of `output` are the truth. When it does not, `output` is efr's own text
/// about the call, such as the note of a command that timed out, and `shown` stays.
pub(crate) fn settled_lines(shown: &[String], output: &str) -> Vec<String> {
    let whole = output_lines(output);
    let held = shown.last().is_some_and(|last| {
        output.lines().any(|line| format::one_line(line.trim_end()).starts_with(last.as_str()))
    });
    if held && !whole.is_empty() { whole } else { shown.to_vec() }
}

#[cfg(test)]
mod tests;
