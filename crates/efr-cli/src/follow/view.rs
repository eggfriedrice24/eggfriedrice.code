//! What a followed turn looks like: the turn's events in, the text to write out.
//!
//! No IO here: the follow loop feeds events and writes what comes back, so the whole
//! appearance of a turn can be tested with events and a fixed screen size.
//!
//! On a terminal, assistant messages stream through an `efr_render::Renderer`: its
//! committed output is written once and its live zone is redrawn in place through a
//! [`LiveZone`]. Notes (tool calls, answers, the end of the turn) are muted lines
//! between the messages, and a pending approval question sits below the live zone. Every
//! colour goes through a role of the palette in the render options. When stdout
//! is not a terminal, the messages are written as raw markdown and everything else
//! goes to stderr, so stdout holds the reply alone.
//!
//! While the turn waits behind another one, the other turn's approvals show too, and so
//! do its running call's last output line and the input it waits for: that turn may be
//! parked on a question nobody else will answer, and the prompt runs only once it is
//! answered. This client tells the daemon that a person here can answer, so it must
//! ask for the running turn's input as well.
//!
//! While a tool call of this turn runs on a terminal, its line (`call`) sits in the live
//! zone with the spinner and its time, in place of the status row, and the last three
//! lines of its output with text in them follow, muted and cut to the width. When the
//! call ends, one line is written once in its place, with the exit code of a failure,
//! and a failed call keeps its last lines of output. When the call's command waits for
//! input and keys can be read, the view
//! asks for an answer line below it: a hidden answer (a password) never reaches the
//! view at all, and a visible one is echoed here as the user types it, unless its
//! prompt looks like a password prompt behind another program (`looks_secret`).
//!
//! A call of this turn that takes a manual input (`manual_input` on its
//! `tool_call_started`, a shell call whose command does not type into a shell that reads
//! command lines), reports no wait and has printed nothing for a while gets one dim line
//! that offers `Ctrl+\` ([`TurnView::silence`], [`TurnView::silent`]). The view reads no
//! key for it: text typed meanwhile stays typeahead for the user's shell (until
//! `Ctrl+\`, which makes the terminal throw it away). Only `Ctrl+\`
//! opens an answer line ([`TurnView::manual`]), which goes as a manual answer. That line
//! is never shown as it is typed: nothing reported a prompt, so nothing tells whether
//! the command asks for a password, and the program's own echo still shows in its
//! output. `Ctrl+\` again closes the line unsent ([`TurnView::manual_cancelled`]).
//!
//! A call whose approval says it may wait for input at the terminal (`interactive`) and
//! that the user allowed with a key here keeps the keys typed while it runs
//! ([`Ask::Retain`]): the follow loop reads them, without echo, into a pending line that
//! is neither shown nor sent. When the call then reports a visible wait, that text
//! starts the answer line, shown unless the prompt looks secret (then a note gives how
//! many characters it starts with), and the user still presses Enter after the
//! question appears: an Enter typed before it is dropped. A hidden wait throws the
//! pending text away, and so does the call's end. After a wait
//! that ends without asking for a password the keys are kept again; after one that
//! asked for a password, or after a manual answer line that may have held one, they are
//! thrown away as before. Keys typed outside such a call stay typeahead for the user's
//! shell.
//!
//! Events change the view and collect committed output; they write no escape sequence.
//! [`TurnView::frame`] turns what changed into one write: the committed output since the
//! last frame, then the live zone, inside synchronized output. The follow loop decides
//! when a frame goes out (at most one per 16 ms, a question and the end at once), and
//! [`TurnView::tick`] draws the status row again ten times a second. A message's text
//! goes into its renderer at the pace of the frames (`message`), and the status row
//! (`status`) says what the turn does: waiting for the model, thinking, writing,
//! preparing a tool call or running one. Drafts ([`TurnView::draft`]), the part of a
//! running turn that the daemon sends before it records it, merge with the persisted
//! updates: a persisted update that repeats text the drafts showed changes nothing. On
//! a terminal the cursor hides while the status row shows, and comes back for a
//! question and at the end. A completed turn ends with one muted line of its time and
//! tokens, and a progress bar in the terminal's tab (OSC 9;4) runs while the turn does,
//! when the terminal draws one.
//!
//! In `auto`, the first call of a turn that runs in the sandbox gets one dim line that
//! says where it can write, and a failed contained call ends with `(sandbox)`. An
//! approval for an exit shows the whole line of the call, what leaves the sandbox and
//! how the call runs after a "yes", every program word of a line that runs outside
//! the sandbox (with the untrusted mark for a program that the sandbox wrote), efr's
//! own facts and the model's reason, labelled as the model's. A turn whose mode fell
//! back says so at its start. When a call changed git settings that run programs, the
//! quarantine question ([`Ask::Surface`]) asks whether to keep them, with its own
//! question id: it is not an approval of a call.

mod call;
mod message;
mod status;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use efr_protocol::{
    ApprovalDecision, CallId, Draft, DraftPart, ErrorBody, Event, EventEnvelope, ExitInfo,
    ExitRecord, InputWait, Launch, Origin, QuestionId, Scope, Seq, SurfaceChange, TurnId,
};
use efr_render::{RenderOptions, render, render_trace};
use jiff::Timestamp;
use unicode_width::UnicodeWidthChar as _;

use crate::format::{self, Block, Spacing, Tone, sandbox};
use crate::live::{LiveZone, Measured, effective_width};
use crate::progress;
use crate::terminal::{Size, at_width};
use call::{Call, Outcome};
use message::Message;
use status::{State, Status, spinner};

pub(crate) use status::TICK;

/// Hides the cursor while the status row shows.
const HIDE_CURSOR: &str = "\x1b[?25l";

/// Shows the cursor again.
const SHOW_CURSOR: &str = "\x1b[?25h";

/// The question under a pending approval.
const QUESTION: &str = "allow? y = yes, n = no";

/// The heading of an approval of the followed turn.
const APPROVAL: &str = "approval needed:";

/// The heading of an approval of the turn that the followed one waits behind.
const BLOCKING_APPROVAL: &str = "the running turn needs approval:";

/// The line under the prompt of a command that waits for hidden input. It promises no
/// more than echo being off: the prompt text comes from the command, and the program
/// that reads the answer can print it, so a fake password prompt would get it.
const HIDDEN_INPUT: &str = "type the answer and press Enter; it is not shown, and the agent sees it only if the program prints it";

/// The line under the prompt of a command that waits for visible input. It promises no
/// more than this: on a plain terminal the echo puts the answer in the output that the
/// model reads, but behind a relay such as `sudo`'s own pty the program on the inner
/// terminal decides whether the answer is shown.
const VISIBLE_INPUT: &str =
    "type the answer and press Enter; the agent sees it if the program shows it";

/// The line under a visible prompt that looks like a password prompt behind a relay,
/// such as `sudo`'s own pty: what is typed is not shown, and it goes as a visible
/// answer, because the program on the inner terminal decides whether it is shown.
const SECRET_INPUT: &str = "this looks like a password prompt behind another program: your typing is not shown here, and the agent sees it only if that program shows it";

/// The line under a call that has printed nothing for a while and reports no wait;
/// the follow loop shows it after [`SILENCE`](crate::follow::SILENCE).
const SILENCE_HINT: &str = "no output for 10 s; press Ctrl+\\ to type an input for the command";

/// The line under a manual answer, which is not shown: no prompt was reported, so it
/// may be a password. The program's own echo, if any, shows in the output.
const MANUAL_INPUT: &str = "type the input and press Enter, or Ctrl+\\ to cancel; your typing is not shown here, and the agent sees it only if the program shows it";

/// The note when the user closed a manual answer line unsent.
const MANUAL_CANCELLED: &str = "the input was not sent";

/// The note when a command waits for hidden input and no key can be read here.
const HIDDEN_INPUT_ELSEWHERE: &str =
    "the command waits for hidden input, such as a password; efr cannot ask for it here";

/// The note when a contained call waits for hidden input: efr stops it and never asks
/// (efr's auto spec, sections 3.12 and 14.7).
const HIDDEN_INPUT_SANDBOXED: &str =
    "sandbox: the command asked for a password; efr does not type secrets into the sandbox";

/// The note when a command waits for visible input and no key can be read here.
const VISIBLE_INPUT_ELSEWHERE: &str = "the command waits for input; efr cannot ask for it here";

/// The note after an answer reached the command.
const ANSWER_SENT: &str = "answer sent";

/// The note when the daemon refused an answer because the wait was over.
const ANSWER_REFUSED: &str = "the command no longer waits for that input; nothing was sent";

/// What the echo of a visible answer starts with.
const ECHO_PREFIX: &str = "> ";

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnEnd {
    /// It finished normally.
    Completed,
    /// It failed with this error.
    Failed(ErrorBody),
    /// Someone interrupted it.
    Interrupted,
    /// The daemon cancelled it at a restart.
    Cancelled,
}

/// What one input to the view produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Step {
    /// Text for stdout when it is not a terminal; on a terminal, [`TurnView::frame`]
    /// writes it.
    pub(crate) out: String,
    /// Text for stderr.
    pub(crate) err: String,
    /// Something to ask the user: read keys for it.
    pub(crate) ask: Option<Ask>,
    /// What was asked is settled elsewhere or no longer waits; stop reading keys.
    pub(crate) settled: bool,
    /// The turn ended.
    pub(crate) end: Option<TurnEnd>,
}

/// How an answer line is read and sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnswerKind {
    /// The terminal's echo is off: never shown, sent as a hidden answer.
    Hidden,
    /// Shown as it is typed, sent as a visible answer.
    Visible,
    /// A visible wait whose prompt looks like a password prompt behind a relay: not
    /// shown, but sent as a visible answer, the kind the daemon reported.
    Masked,
    /// A line the user asked for with `Ctrl+\` while the command reported no wait: not
    /// shown, because it may be a password, and sent as a visible, manual answer.
    Manual,
}

impl AnswerKind {
    /// True when what is typed is shown as it is typed.
    pub(crate) fn shown(self) -> bool {
        self == AnswerKind::Visible
    }

    /// True when the answer goes as a hidden one.
    pub(crate) fn hidden(self) -> bool {
        self == AnswerKind::Hidden
    }

    /// True when the answer goes as a manual one, without a reported wait.
    pub(crate) fn manual(self) -> bool {
        self == AnswerKind::Manual
    }

    /// True when what is typed may be a password: until the call completes, keys typed
    /// while it asks nothing are read and thrown away.
    fn guards(self) -> bool {
        matches!(self, AnswerKind::Hidden | AnswerKind::Masked)
    }

    /// The line under the prompt.
    fn line(self) -> &'static str {
        match self {
            AnswerKind::Hidden => HIDDEN_INPUT,
            AnswerKind::Visible => VISIBLE_INPUT,
            AnswerKind::Masked => SECRET_INPUT,
            AnswerKind::Manual => MANUAL_INPUT,
        }
    }
}

/// What the user is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ask {
    /// An approval, answered with one key.
    Approval(CallId),
    /// A line for the running call `call_id`, whose command waits for input, read and
    /// sent as `kind` says.
    Input { call_id: CallId, kind: AnswerKind },
    /// Nothing to answer now, but keep reading keys and throw them away until call
    /// `call_id` completes. It asked for a hidden answer, or a manual one of a call that
    /// kept its keys, and may ask again, as `sudo` does after a wrong password, and a
    /// password typed again meanwhile must neither show nor stay queued for the user's
    /// shell.
    Discard(CallId),
    /// Nothing to answer now, but keep reading keys into a pending line for call
    /// `call_id`, which the user approved here as one that may wait for input: a
    /// visible wait of the call starts its answer line with them. Keys already queued
    /// stay, because they were typed for the call.
    Retain(CallId),
    /// The quarantine question, answered with one key: `y` keeps the change, `n` leaves
    /// it in quarantine.
    Surface(QuestionId),
}

/// The tool call whose output is arriving now.
#[derive(Debug)]
struct Running {
    call_id: CallId,
    /// The last line of its output with text in it.
    tail: String,
    /// The last lines of its output with text in them, at most `call::TAIL_LINES`.
    lines: Vec<String>,
    /// What its command waits for, as the daemon said last.
    wait: InputWait,
    /// The answer the user is being asked for now, if any.
    asking: Option<AnswerKind>,
    /// It asked for an answer that may be a password and still runs: keys are read and
    /// thrown away while it asks nothing.
    guarding: bool,
    /// What the user typed so far, for a visible answer only.
    typed: String,
    /// A call of the followed turn that takes a manual input, which may offer `Ctrl+\`
    /// when it is silent.
    takes_manual: bool,
    /// Changes whenever the call shows a sign of life: output, a wait, an answer. The
    /// follow loop times the silence from the last change.
    activity: u64,
    /// The line that offers `Ctrl+\` is shown.
    hinted: bool,
}

impl Running {
    fn new(call_id: CallId) -> Running {
        Running {
            call_id,
            tail: String::new(),
            lines: Vec::new(),
            wait: InputWait::None,
            asking: None,
            guarding: false,
            typed: String::new(),
            takes_manual: false,
            activity: 0,
            hinted: false,
        }
    }

    /// True while keys are read for it, to answer or to throw away.
    fn reads_keys(&self) -> bool {
        self.asking.is_some() || self.guarding
    }

    /// The call shows a sign of life: its silence starts again, and the line that
    /// offers `Ctrl+\` goes.
    fn stirred(&mut self) {
        self.activity = self.activity.wrapping_add(1);
        self.hinted = false;
    }

    /// True when the call takes a manual input, waits for nothing the daemon reported
    /// and nothing reads keys for it.
    fn quiet(&self) -> bool {
        self.takes_manual && self.wait == InputWait::None && !self.reads_keys()
    }
}

/// What `config.toml` switches in the look of a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Look {
    /// The spinner turns and a band moves over the state (`render.motion`).
    pub(crate) motion: bool,
    /// A muted line ends each completed turn (`render.turn_summary`).
    pub(crate) summary: bool,
    /// The progress bar of the terminal's tab shows while the turn runs
    /// (`render.progress`, already checked against the terminal).
    pub(crate) progress: bool,
}

/// The state of one followed turn on the screen.
#[derive(Debug)]
pub(crate) struct TurnView {
    turn: TurnId,
    /// The options for the turn; the width is replaced by the screen's at each use.
    options: RenderOptions,
    message: Option<Message>,
    /// Messages with a smaller index are complete.
    next_index: u32,
    tools: HashMap<CallId, String>,
    live: LiveZone,
    spacing: Spacing,
    /// Raw markdown messages written so far, to separate them by a blank line.
    raw_messages: u32,
    /// The approval waiting for a key.
    asking: Option<CallId>,
    /// The approval that this client answered, whose resolution needs no second note.
    answered: Option<CallId>,
    /// The turn waits behind another one and has not started.
    queued: bool,
    /// The other turn's approvals shown while this one waited.
    blocking: HashSet<CallId>,
    /// Calls whose approval was denied or expired: a note said so already, so their
    /// failed end needs no second one.
    refused: HashSet<CallId>,
    /// The tool call that runs, once its output or an input wait arrived.
    running: Option<Running>,
    /// The tool call of this turn from its start to its end, whose line shows in the
    /// live zone while it runs, on a terminal.
    call: Option<Call>,
    /// When stdout is not a terminal: stderr's last line is the echo of a visible answer
    /// (`> ` and what is typed), without its newline.
    echo_line: bool,
    /// Approvals asked for calls that may wait for input at the terminal.
    interactive: HashSet<CallId>,
    /// The call that the user allowed here as one that may wait for input, while it
    /// runs: its keys are kept for it whenever nothing else reads them.
    retained: Option<CallId>,
    /// The home directory, which the sandbox's lines print as `~`.
    home: Option<PathBuf>,
    /// The turn runs in a registered project, which its sandbox can write.
    in_project: bool,
    /// The line that says where a contained call can write is shown for this turn.
    traced: bool,
    /// Calls that run in the sandbox, whose failure says so.
    contained: HashSet<CallId>,
    /// The record of each call's exit, for the lines of its approval.
    exits: HashMap<CallId, ExitRecord>,
    /// The command of each shell call, whose approval shows each of its lines.
    commands: HashMap<CallId, String>,
    /// The quarantine question waiting for a key.
    surface: Option<QuestionId>,
    /// The quarantine question that this client answered, whose answer needs no second
    /// note.
    surface_answered: Option<QuestionId>,
    /// Every quarantine question shown, of this turn or of the turn it waits behind.
    surfaces: HashSet<QuestionId>,
    /// What `config.toml` switches.
    look: Look,
    /// On a terminal: the committed output since the last frame.
    pending: String,
    /// Something changed since the last frame.
    dirty: bool,
    /// The status row, once the follow loop started the view on a terminal.
    status: Option<Status>,
    /// The turn ended or the view was closed: the live zone and the status row are gone.
    ended: bool,
    /// The progress bar's sequence for the end: cleared, or failed.
    end_progress: Option<&'static str>,
    /// The progress bar's sequence sent last.
    progress_sent: Option<&'static str>,
    /// The cursor is hidden.
    cursor_hidden: bool,
    /// The time of the last frame.
    last_frame: Option<Timestamp>,
    /// The daemon's time of the event that is being taken, and of the turn's start.
    event_at: Option<Timestamp>,
    started_at: Option<Timestamp>,
    /// The sequence number of the newest event of this turn, which a draft must not be
    /// older than.
    turn_seq: Option<Seq>,
}

impl TurnView {
    /// A view of turn `turn` rendered with `options`.
    pub(crate) fn new(turn: TurnId, options: RenderOptions) -> TurnView {
        TurnView {
            turn,
            live: LiveZone::new(options.width_method()),
            options,
            message: None,
            next_index: 0,
            tools: HashMap::new(),
            spacing: Spacing::default(),
            raw_messages: 0,
            asking: None,
            answered: None,
            queued: false,
            blocking: HashSet::new(),
            refused: HashSet::new(),
            running: None,
            call: None,
            echo_line: false,
            interactive: HashSet::new(),
            retained: None,
            home: None,
            in_project: false,
            traced: false,
            contained: HashSet::new(),
            exits: HashMap::new(),
            commands: HashMap::new(),
            surface: None,
            surface_answered: None,
            surfaces: HashSet::new(),
            look: Look::default(),
            pending: String::new(),
            dirty: false,
            status: None,
            ended: false,
            end_progress: None,
            progress_sent: None,
            cursor_hidden: false,
            last_frame: None,
            event_at: None,
            started_at: None,
            turn_seq: None,
        }
    }

    /// The same view with `look`.
    pub(crate) fn with_look(mut self, look: Look) -> TurnView {
        self.look = look;
        self
    }

    /// Starts the status row, on a terminal: the prompt's line was just sent, and the
    /// row shows from the next frame until the turn ends.
    pub(crate) fn start(&mut self) {
        if self.terminal() && self.status.is_none() {
            let mut status = Status::new(self.look.motion);
            if self.queued {
                status.set(State::Queued);
            }
            self.status = Some(status);
            self.dirty = true;
        }
    }

    /// The same view, which prints paths below `home` as `~`.
    pub(crate) fn with_home(mut self, home: Option<PathBuf>) -> TurnView {
        self.home = home;
        self
    }

    /// True while an approval or the quarantine question waits for a key.
    fn question_pending(&self) -> bool {
        self.asking.is_some() || self.surface.is_some()
    }

    /// The turn waits behind the running one: until it starts, the running turn's
    /// approvals show and can be answered here.
    pub(crate) fn queue(&mut self) {
        self.queued = true;
        self.state(State::Queued);
    }

    /// Takes one event of the conversation with its sequence number and its time on the
    /// daemon's clock, which time the turn for its end-of-turn line.
    pub(crate) fn envelope(&mut self, envelope: &EventEnvelope, size: Size, can_ask: bool) -> Step {
        if envelope.event.turn_id() == Some(self.turn) {
            self.turn_seq = Some(envelope.seq);
            self.event_at = Some(envelope.at);
        }
        let step = self.event(&envelope.event, size, can_ask);
        self.event_at = None;
        step
    }

    /// Takes a draft: the part of this turn that the daemon sends before it records it.
    /// Text merges with the persisted updates of its message; reasoning and the input of
    /// a tool call only change the status row. A draft of another turn, or one older
    /// than an event of this turn that the view took, changes nothing.
    pub(crate) fn draft(&mut self, draft: &Draft, size: Size) -> Step {
        if draft.turn_id != self.turn
            || self.ended
            || self.turn_seq.is_some_and(|seq| draft.after_seq < seq)
        {
            return Step::default();
        }
        if let Some(status) = &mut self.status {
            status.stir();
        }
        match &draft.draft {
            DraftPart::Text { index, offset, delta } => {
                if *index < self.next_index {
                    return Step::default();
                }
                self.state(State::Writing);
                self.message_delta(*index, *offset, delta, size)
            }
            DraftPart::Reasoning { title, .. } => {
                let title = title.as_deref().map(format::one_line);
                self.state(State::Thinking(title.filter(|title| !title.trim().is_empty())));
                Step::default()
            }
            DraftPart::ToolInput { call, tool, bytes } => {
                // The calls of one answer come one after another: the newest one shows.
                let newer = match self.status.as_ref().map(Status::state) {
                    Some(State::Preparing { call: shown, .. }) => call >= shown,
                    _ => true,
                };
                if newer {
                    let tool = format::one_line(tool);
                    self.state(State::Preparing { call: *call, tool, bytes: *bytes });
                }
                Step::default()
            }
            _ => Step::default(),
        }
    }

    /// One write that brings the screen up to date at `now`: the committed output since
    /// the last frame, then the live zone with the status row, inside synchronized
    /// output, and the cursor and the progress bar as they must be now. Empty when
    /// nothing changed, and always empty when stdout is not a terminal.
    pub(crate) fn frame(&mut self, size: Size, now: Timestamp) -> String {
        self.draw(size, now, false)
    }

    /// The frame of a tick: the status row moves on, and the progress bar is sent again,
    /// because a terminal hides a bar that is not sent again.
    pub(crate) fn tick(&mut self, size: Size, now: Timestamp) -> String {
        self.draw(size, now, true)
    }

    /// True while a frame would write something new: a change, or text that waits to
    /// show.
    pub(crate) fn wants_frame(&self) -> bool {
        self.dirty || self.message.as_ref().is_some_and(Message::waiting)
    }

    /// True while the status row runs and needs a tick.
    pub(crate) fn ticks(&self) -> bool {
        self.status.is_some() && !self.ended
    }

    /// What a sudden way out must write to leave the terminal as it was: the cursor
    /// back, and no progress bar.
    pub(crate) fn restore(&self) -> String {
        let mut text = String::new();
        if self.cursor_hidden {
            text.push_str(SHOW_CURSOR);
        }
        if matches!(self.progress_sent, Some(sent) if sent == progress::RUNNING || sent == progress::PAUSED)
        {
            text.push_str(progress::CLEAR);
        }
        text
    }

    fn draw(&mut self, size: Size, now: Timestamp, tick: bool) -> String {
        if !self.terminal() {
            return String::new();
        }
        let since_frame = self
            .last_frame
            .map(|last| Duration::try_from(now.duration_since(last)).unwrap_or_default());
        self.last_frame = Some(now);
        if let Some(message) = &mut self.message {
            let revealed = message.reveal(now, since_frame);
            self.pending.push_str(&revealed);
        }
        let committed = std::mem::take(&mut self.pending);
        self.dirty = false;
        let call_shown = self.call_shown();
        if call_shown && let Some(call) = &mut self.call {
            call.show(now);
        }
        let asking = self.waits_for_user();
        if let Some(status) = &mut self.status
            && !self.ended
        {
            status.at(now, asking);
        }
        let (body, measured) = self.live_body(size, now);
        let options = self.options_at(size);
        // A running call's line carries the spinner instead of the row.
        let row = match &self.status {
            Some(status) if !self.ended && !asking && !call_shown => status.row(now, &options),
            _ => String::new(),
        };
        let hide = self.ticks() && !asking;
        let mut out = String::new();
        if hide && !self.cursor_hidden {
            out.push_str(HIDE_CURSOR);
            self.cursor_hidden = true;
        }
        out.push_str(&self.live.draw(&committed, &body, measured, &row, size));
        if !hide && self.cursor_hidden {
            out.push_str(SHOW_CURSOR);
            self.cursor_hidden = false;
        }
        if let Some(sequence) = self.progress_now(asking)
            && (Some(sequence) != self.progress_sent || (tick && !self.ended))
        {
            out.push_str(sequence);
            self.progress_sent = Some(sequence);
        }
        out
    }

    /// The progress bar's sequence for now, when the bar is on.
    fn progress_now(&self, asking: bool) -> Option<&'static str> {
        if !self.look.progress || self.status.is_none() {
            return None;
        }
        if self.ended {
            return Some(self.end_progress.unwrap_or(progress::CLEAR));
        }
        Some(if asking { progress::PAUSED } else { progress::RUNNING })
    }

    /// True while the user is asked something here: an approval, the quarantine
    /// question or an answer line.
    fn waits_for_user(&self) -> bool {
        self.question_pending()
            || self.running.as_ref().is_some_and(|running| running.asking.is_some())
    }

    /// True when a turn ends with a line of its time and tokens: on a terminal, where
    /// it closes the turn in the scrollback. Piped output keeps its notes as they were.
    fn summary(&self) -> bool {
        self.look.summary && self.terminal()
    }

    /// How long the turn took on the daemon's clock, from its start to the event being
    /// taken; `None` when one of the two times is not known.
    fn took(&self) -> Option<Duration> {
        let (start, end) = (self.started_at?, self.event_at?);
        Duration::try_from(end.duration_since(start)).ok()
    }

    /// The approval of call `call_id` was answered or expired: an answer that came from
    /// elsewhere lets the call go on.
    fn answer_came(&mut self, call_id: CallId) {
        if self.status.as_ref().is_some_and(|status| *status.state() == State::Answer) {
            let tool =
                self.tools.get(&call_id).map_or_else(String::new, |tool| format::one_line(tool));
            self.state(State::Tool(tool));
        }
    }

    /// The status row says `state` now.
    fn state(&mut self, state: State) {
        if let Some(status) = &mut self.status {
            status.set(state);
            self.dirty = true;
        }
    }

    /// True while the turn waits behind another one.
    pub(crate) fn is_queued(&self) -> bool {
        self.queued
    }

    fn terminal(&self) -> bool {
        self.options.is_terminal()
    }

    fn options_at(&self, size: Size) -> RenderOptions {
        at_width(&self.options, effective_width(size))
    }

    /// True when the live zone shows the line of the running call of this turn: it
    /// runs, no question about it or another waits, and the view did not end.
    fn call_shown(&self) -> bool {
        self.call.as_ref().is_some_and(|call| {
            !call.awaiting && !self.refused.contains(&call.call_id) && !self.question_pending()
        }) && self.terminal()
            && !self.ended
    }

    /// Takes one event of the conversation. Events of other turns change nothing,
    /// except approvals while this turn waits behind another one. `can_ask` says
    /// whether one-key answers can be read.
    pub(crate) fn event(&mut self, event: &Event, size: Size, can_ask: bool) -> Step {
        if event.turn_id() != Some(self.turn) {
            return self.other_turn(event, size, can_ask);
        }
        if let Some(status) = &mut self.status {
            status.stir();
        }
        match event {
            Event::TurnStarted { scope, settings, .. } => {
                self.queued = false;
                self.started_at = self.event_at;
                if let Some(status) = &mut self.status {
                    status.turn_started();
                }
                self.in_project = matches!(scope, Scope::Project(_));
                // A call or a question of the turn ahead that is still shown or kept is
                // over now.
                self.call = None;
                let retained = self.retained.take().is_some();
                let surface = self.surface.take().is_some();
                let mut step = match self.running.take() {
                    Some(running) => Step {
                        settled: running.reads_keys() || retained || surface,
                        ..self.commit(String::new())
                    },
                    None => Step { settled: retained || surface, ..Step::default() },
                };
                let fallback = settings.as_ref().and_then(|settings| {
                    let fallback = settings.fallback.as_ref()?;
                    Some(sandbox::fallback_note(fallback, settings.mode))
                });
                if let Some(note) = fallback {
                    let noted = self.note(&note, size);
                    step.out.push_str(&noted.out);
                    step.err.push_str(&noted.err);
                }
                step
            }
            Event::AssistantMessageUpdated { index, offset, delta, .. } => {
                if *index >= self.next_index {
                    self.state(State::Writing);
                }
                self.message_delta(*index, *offset, delta, size)
            }
            Event::AssistantMessageCompleted { index, text, .. } => {
                self.state(State::Model);
                self.message_text(*index, text, true, size)
            }
            Event::ToolCallStarted { call_id, tool, input, manual_input, launch, .. } => {
                self.state(State::Tool(format::one_line(tool)));
                self.tools.insert(*call_id, tool.clone());
                if let Some(command) = format::command_of(input) {
                    self.commands.insert(*call_id, command.to_owned());
                }
                // A call that takes a manual input is followed from its start, so a
                // command that never prints can still offer `Ctrl+\`.
                let mut settled = false;
                if *manual_input {
                    let (running, replaced) = self.running(*call_id);
                    running.takes_manual = true;
                    settled = replaced;
                }
                let contained = matches!(launch, Some(Launch::Contained { .. }));
                if contained {
                    self.contained.insert(*call_id);
                }
                // The model writes a tool call after the text it belongs to, so the
                // message before it is complete.
                let before = self.finish_message();
                // On a terminal the call's line shows in the live zone while it runs and
                // is written once when it ends; elsewhere a note says that it starts.
                let mut step = if self.terminal() {
                    let line = format::call_line(tool, input);
                    self.call = Some(Call::new(*call_id, line, self.event_at));
                    self.commit(before)
                } else {
                    let line = format::tool_call(tool, input, None);
                    self.note_after(before, &line, size)
                };
                // One line per turn says that the sandbox is on, then nothing new.
                if contained && !self.traced {
                    self.traced = true;
                    let trace = if self.in_project {
                        sandbox::CONTAINED_IN_PROJECT
                    } else {
                        sandbox::CONTAINED
                    };
                    let noted = self.note(trace, size);
                    step.out.push_str(&noted.out);
                    step.err.push_str(&noted.err);
                }
                Step { settled, ..step }
            }
            Event::ToolCallOutputUpdated { call_id, tail, .. } => self.output(*call_id, tail),
            Event::ToolCallInputChanged { call_id, input, looks_secret, .. } => {
                self.input_changed(*call_id, *input, *looks_secret, size, can_ask)
            }
            Event::ToolCallCompleted {
                call_id,
                output,
                is_error,
                exit_code,
                sandbox: summary,
                refusal,
                ..
            } => {
                self.state(State::Model);
                let lines = match &self.running {
                    Some(running) if running.call_id == *call_id && !running.lines.is_empty() => {
                        running.lines.clone()
                    }
                    _ => call::output_lines(output),
                };
                let settled = self.call_ended(*call_id);
                let tool = self.tools.get(call_id).map_or("the tool", String::as_str);
                let contained = self.contained.remove(call_id)
                    || summary.as_ref().is_some_and(|summary| summary.confined);
                let setup = summary.as_ref().and_then(|summary| summary.setup_error.as_deref());
                let denied = self.refused.contains(call_id);
                let mut notes: Vec<String> = Vec::new();
                match self.call.take().filter(|call| call.call_id == *call_id) {
                    // A denied call never ran: its question and the answer said it all.
                    Some(_) if denied && refusal.is_none() => {}
                    Some(call) => {
                        let outcome = match (refusal, setup, *exit_code) {
                            (Some(reason), _, _) => Outcome::Refused(reason),
                            // A setup failure has its own line below.
                            (None, Some(_), _) => Outcome::Ran,
                            (None, None, Some(code)) if code != 0 => {
                                Outcome::Exited { code, contained }
                            }
                            (None, None, None) if *is_error => Outcome::Failed { contained },
                            _ => Outcome::Ran,
                        };
                        self.call_line(&call, outcome, &lines, size);
                    }
                    None => {
                        // A setup failure has its own line; the call's status says
                        // nothing more. A refused call never ran, in the sandbox or out
                        // of it.
                        let line =
                            match refusal {
                                Some(reason) => Some(format::refused(tool, reason)),
                                None => format::tool_result(tool, *is_error, *exit_code)
                                    .filter(|_| !denied && setup.is_none())
                                    .map(|line| {
                                        if contained { format!("{line} (sandbox)") } else { line }
                                    }),
                            };
                        notes.extend(line);
                    }
                }
                notes.extend(sandbox::setup_failed(setup));
                if let Some(summary) = summary {
                    notes.extend(summary.blocked.iter().map(sandbox::blocked));
                    notes.extend(sandbox::background_stopped(&summary.background_stopped));
                    notes.extend(sandbox::survivors(&summary.survivors));
                }
                // The live tail goes either way.
                Step { settled, ..self.notes(&notes, size) }
            }
            Event::ExitRequested { call_id, record, .. } => {
                self.exits.insert(*call_id, (**record).clone());
                Step::default()
            }
            Event::ApprovalRequested {
                call_id, summary, diff_preview, interactive, exit, ..
            } => {
                if *interactive {
                    self.interactive.insert(*call_id);
                }
                if let Some(call) = self.call.as_mut().filter(|call| call.call_id == *call_id) {
                    call.awaiting = true;
                }
                if !can_ask {
                    self.state(State::Answer);
                }
                let request = Request {
                    heading: APPROVAL,
                    summary,
                    diff: diff_preview.as_deref(),
                    exit: exit.as_ref(),
                };
                self.approval(*call_id, &request, size, can_ask)
            }
            Event::SandboxSurfaceChanged { changes, .. } => {
                match sandbox::surface_changed(changes, self.home.as_deref()) {
                    Some(line) => self.note(&line, size),
                    None => Step::default(),
                }
            }
            Event::SurfaceQuestionRequested { question_id, changes, .. } => {
                self.surface_question(*question_id, changes, size, can_ask)
            }
            Event::SurfaceQuestionAnswered { question_id, keep, origin, .. } => {
                self.surface_resolved(*question_id, *keep, *origin, size)
            }
            Event::TurnSurfaceReport { files, .. } => {
                self.dim_block(&sandbox::surface_report(files), size)
            }
            Event::ApprovalResolved { call_id, decision, origin, .. } => {
                if let Some(call) = self.call.as_mut().filter(|call| call.call_id == *call_id) {
                    call.approved(self.event_at);
                }
                self.answer_came(*call_id);
                self.resolved(*call_id, *decision, *origin, size)
            }
            Event::ApprovalExpired { call_id, .. } => {
                self.answer_came(*call_id);
                self.expired(*call_id, size)
            }
            Event::TurnSteered { text, .. } => {
                self.note(&format!("steered: {}", format::one_line(text)), size)
            }
            Event::TurnInterruptRequested { origin, .. } => {
                self.note(&format!("interrupt requested from {}", format::origin(*origin)), size)
            }
            Event::TurnCompleted { usage, .. } => {
                let line = self.summary().then(|| format::turn_done(self.took(), usage.as_ref()));
                self.end(TurnEnd::Completed, line, size)
            }
            Event::TurnFailed { error, .. } => self.end(TurnEnd::Failed(error.clone()), None, size),
            Event::TurnInterrupted { .. } => {
                let line = match self.took().filter(|_| self.summary()) {
                    Some(took) => format!("interrupted after {}", format::took(took)),
                    None => "interrupted".to_owned(),
                };
                self.end(TurnEnd::Interrupted, Some(line), size)
            }
            Event::TurnCancelled { .. } => self.end(TurnEnd::Cancelled, None, size),
            _ => Step::default(),
        }
    }

    /// An event of another turn: only the approvals of the turn this one waits behind
    /// and its running call show, and only until this one starts.
    fn other_turn(&mut self, event: &Event, size: Size, can_ask: bool) -> Step {
        if !self.queued {
            return Step::default();
        }
        match event {
            Event::ToolCallStarted { call_id, tool, input, .. } => {
                self.tools.insert(*call_id, tool.clone());
                if let Some(command) = format::command_of(input) {
                    self.commands.insert(*call_id, command.to_owned());
                }
                Step::default()
            }
            Event::ExitRequested { call_id, record, .. } => {
                self.exits.insert(*call_id, (**record).clone());
                Step::default()
            }
            Event::ApprovalRequested {
                call_id, summary, diff_preview, interactive, exit, ..
            } => {
                self.blocking.insert(*call_id);
                if *interactive {
                    self.interactive.insert(*call_id);
                }
                let request = Request {
                    heading: BLOCKING_APPROVAL,
                    summary,
                    diff: diff_preview.as_deref(),
                    exit: exit.as_ref(),
                };
                self.approval(*call_id, &request, size, can_ask)
            }
            Event::SurfaceQuestionRequested { question_id, changes, .. } => {
                self.surface_question(*question_id, changes, size, can_ask)
            }
            Event::SurfaceQuestionAnswered { question_id, keep, origin, .. }
                if self.surfaces.contains(question_id) =>
            {
                self.surface_resolved(*question_id, *keep, *origin, size)
            }
            Event::ApprovalResolved { call_id, decision, origin, .. }
                if self.blocking.contains(call_id) =>
            {
                self.resolved(*call_id, *decision, *origin, size)
            }
            Event::ApprovalExpired { call_id, .. } if self.blocking.contains(call_id) => {
                self.expired(*call_id, size)
            }
            Event::ToolCallOutputUpdated { call_id, tail, .. } => self.output(*call_id, tail),
            Event::ToolCallInputChanged { call_id, input, looks_secret, .. } => {
                self.input_changed(*call_id, *input, *looks_secret, size, can_ask)
            }
            Event::ToolCallCompleted { call_id, .. }
                if self.running.as_ref().is_some_and(|running| running.call_id == *call_id)
                    || self.retained == Some(*call_id) =>
            {
                let settled = self.call_ended(*call_id);
                Step { settled, ..self.commit(String::new()) }
            }
            _ => Step::default(),
        }
    }

    fn resolved(
        &mut self,
        call_id: CallId,
        decision: ApprovalDecision,
        origin: Origin,
        size: Size,
    ) -> Step {
        if decision == ApprovalDecision::Deny {
            self.refused.insert(call_id);
        }
        if self.answered == Some(call_id) {
            return Step::default();
        }
        let settled = self.settle(call_id);
        let line = format!("{} from {}", format::decision(decision), format::origin(origin));
        Step { settled, ..self.note(&line, size) }
    }

    fn expired(&mut self, call_id: CallId, size: Size) -> Step {
        self.refused.insert(call_id);
        let settled = self.settle(call_id);
        Step { settled, ..self.note("the approval expired", size) }
    }

    /// Dim note lines, one after the other; with none, only the live zone is redrawn.
    fn notes(&mut self, lines: &[String], size: Size) -> Step {
        let mut step = Step::default();
        if lines.is_empty() {
            return self.commit(String::new());
        }
        for line in lines {
            let noted = self.note(line, size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        step
    }

    /// Dim lines that keep their indentation and their whole width, unlike a note, which
    /// is cut to the screen: a list of files must stay complete.
    fn dim_block(&mut self, lines: &[String], size: Size) -> Step {
        let options = self.options_at(size);
        let mut text = String::new();
        for line in lines {
            text.push_str(&format::paint(line, Tone::Dim, &options));
            text.push('\n');
        }
        if !self.terminal() {
            return Step { err: self.raw_err(text), ..Step::default() };
        }
        let mut committed = self.spacing.before(Block::Note).to_owned();
        committed.push_str(&text);
        self.stage(&committed);
        Step::default()
    }

    /// The quarantine question `question_id` about `changes`: a git setting that runs
    /// programs, which the last call changed and the launcher moved to quarantine. Like
    /// an approval it waits below the live zone for one key, but no call waits for it.
    fn surface_question(
        &mut self,
        question_id: QuestionId,
        changes: &[SurfaceChange],
        size: Size,
        can_ask: bool,
    ) -> Step {
        self.surfaces.insert(question_id);
        let before = self.finish_message();
        let options = self.options_at(size);
        let mut text = format::paint(sandbox::SURFACE_QUESTION, Tone::Attention, &options);
        text.push('\n');
        for change in changes {
            text.push_str(&sandbox::surface_change(change, self.home.as_deref()));
            text.push('\n');
        }
        let mut step = Step { out: before, ..Step::default() };
        if can_ask {
            self.surface = Some(question_id);
            step.ask = Some(Ask::Surface(question_id));
            // One question at a time: the question's key reader replaces any other.
            self.retained = None;
            if let Some(running) = &mut self.running {
                running.asking = None;
                running.guarding = false;
                running.typed.clear();
                running.hinted = false;
            }
            if !self.terminal() {
                text.push_str(sandbox::KEEP_QUESTION);
                text.push('\n');
            }
        } else {
            text.push_str(&render_trace("waiting for another client to answer", &options));
        }
        self.show_question(step, &text)
    }

    /// The user answered the quarantine question `question_id` with a key here.
    pub(crate) fn surface_answered(
        &mut self,
        question_id: QuestionId,
        keep: bool,
        size: Size,
    ) -> Step {
        if self.surface == Some(question_id) {
            self.surface = None;
        }
        self.surface_answered = Some(question_id);
        let line = if keep { "kept" } else { "left in quarantine" };
        self.note(line, size)
    }

    /// The quarantine question `question_id` was answered, expired or ended with an
    /// interrupt.
    fn surface_resolved(
        &mut self,
        question_id: QuestionId,
        keep: bool,
        origin: Option<Origin>,
        size: Size,
    ) -> Step {
        if self.surface_answered == Some(question_id) {
            return Step::default();
        }
        let settled = self.surface == Some(question_id);
        if settled {
            self.surface = None;
        }
        let line = sandbox::surface_answered(keep, origin.map(format::origin));
        Step { settled, ..self.note(&line, size) }
    }

    /// Writes the question `text` after `step`'s output: below the live zone on a
    /// terminal, on stderr otherwise.
    fn show_question(&mut self, mut step: Step, text: &str) -> Step {
        if self.terminal() {
            let mut committed = std::mem::take(&mut step.out);
            committed.push_str(self.spacing.before(Block::Approval));
            committed.push_str(text);
            self.stage(&committed);
        } else {
            step.err = self.raw_err(text.to_owned());
        }
        step
    }

    /// A dim note line, such as `queued behind the running turn`.
    pub(crate) fn note(&mut self, text: &str, size: Size) -> Step {
        self.note_after(String::new(), text, size)
    }

    /// A note written after `before`, the end of a message.
    fn note_after(&mut self, before: String, text: &str, size: Size) -> Step {
        let options = self.options_at(size);
        if !self.terminal() {
            let err = self.raw_err(render_trace(text, &options));
            return Step { out: before, err, ..Step::default() };
        }
        let mut committed = before;
        committed.push_str(self.spacing.before(Block::Note));
        committed.push_str(&render_trace(text, &options));
        self.stage(&committed);
        Step::default()
    }

    /// The output of call `call_id` grew and now ends in `tail`.
    fn output(&mut self, call_id: CallId, tail: &str) -> Step {
        let (running, settled) = self.running(call_id);
        running.tail = last_line(tail);
        running.lines = call::last_lines(tail);
        running.stirred();
        Step { settled, ..self.commit(String::new()) }
    }

    /// The call that may offer `Ctrl+\` once it has been silent long enough, with its
    /// activity count, which changes at every sign of life: a call of this turn that
    /// takes a manual input, runs and reports no wait, while no key is read for it and
    /// no approval waits, and whose line that offers `Ctrl+\` is not shown yet.
    pub(crate) fn silence(&self) -> Option<(CallId, u64)> {
        let running = self.running.as_ref()?;
        let quiet = running.quiet() && !running.hinted && !self.question_pending();
        quiet.then_some((running.call_id, running.activity))
    }

    /// The call whose line that offers `Ctrl+\` is shown now; the follow loop waits for
    /// the key exactly while there is one.
    pub(crate) fn manual_offer(&self) -> Option<CallId> {
        let running = self.running.as_ref()?;
        let offered = running.quiet() && running.hinted && !self.question_pending();
        offered.then_some(running.call_id)
    }

    /// Call `call_id` has been silent since its activity count was last read: shows the
    /// line that offers `Ctrl+\`. Nothing when it is no longer the silent call.
    pub(crate) fn silent(&mut self, call_id: CallId, size: Size) -> Step {
        if self.silence().map(|(silent, _)| silent) != Some(call_id) {
            return Step::default();
        }
        if let Some(running) = &mut self.running {
            running.hinted = true;
        }
        if self.terminal() { self.commit(String::new()) } else { self.note(SILENCE_HINT, size) }
    }

    /// The user pressed `Ctrl+\` while the line for call `call_id` was shown: asks for a
    /// line that is not shown as it is typed and goes as a manual answer.
    pub(crate) fn manual(&mut self, call_id: CallId, size: Size) -> Step {
        if self.manual_offer() != Some(call_id) {
            return Step::default();
        }
        if let Some(running) = &mut self.running {
            running.hinted = false;
        }
        self.ask_for(call_id, AnswerKind::Manual, size)
    }

    /// True while a manual answer line is open, which `Ctrl+\` closes.
    pub(crate) fn manual_open(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.asking == Some(AnswerKind::Manual))
    }

    /// The user pressed `Ctrl+\` again while a manual answer line was open: the line
    /// closes unsent, its keys stop, and the line that offers `Ctrl+\` comes back.
    pub(crate) fn manual_cancelled(&mut self, size: Size) -> Step {
        if !self.manual_open() {
            return Step::default();
        }
        let mut call = None;
        if let Some(running) = &mut self.running {
            running.asking = None;
            running.typed.clear();
            running.hinted = true;
            call = Some(running.call_id);
        }
        let step = self.note(MANUAL_CANCELLED, size);
        self.manual_closed(call, step)
    }

    /// `step` for the moment a manual answer line of call `call` closes. The keys of a
    /// call that kept them are read and thrown away from now on instead of kept: the
    /// line may have held a password, and a second copy typed ahead for a prompt that
    /// efr cannot see must never start a later answer line that is shown. The call's
    /// silence still offers `Ctrl+\` for the next manual line. Otherwise the keys stop.
    fn manual_closed(&self, call: Option<CallId>, step: Step) -> Step {
        match call.filter(|call| self.retained == Some(*call)) {
            Some(call) => Step { ask: Some(Ask::Discard(call)), ..step },
            None => Step { settled: true, ..step },
        }
    }

    /// `step` for the moment nothing reads keys for call `call` any more: the keys are
    /// kept for it when it is the retained call, and stop otherwise.
    fn keys_free(&self, call: Option<CallId>, step: Step) -> Step {
        match call.filter(|call| self.retained == Some(*call)) {
            Some(call) => Step { ask: Some(Ask::Retain(call)), ..step },
            None => Step { settled: true, ..step },
        }
    }

    /// Call `call_id` began or stopped waiting for input; `looks_secret` when a visible
    /// wait's prompt looks like a password prompt behind another program.
    fn input_changed(
        &mut self,
        call_id: CallId,
        input: InputWait,
        looks_secret: bool,
        size: Size,
        can_ask: bool,
    ) -> Step {
        let approval_pending = self.question_pending();
        let (running, replaced) = self.running(call_id);
        if running.asking.is_some_and(AnswerKind::guards) {
            running.guarding = true;
        }
        let settled = replaced || running.reads_keys();
        running.wait = input;
        running.asking = None;
        running.typed.clear();
        running.stirred();
        let kind = match input {
            InputWait::Hidden => AnswerKind::Hidden,
            InputWait::Visible if looks_secret => AnswerKind::Masked,
            InputWait::Visible => AnswerKind::Visible,
            // No wait, or one this build does not know: nothing to ask, but a call that
            // asked for a password keeps the keys quiet until it completes.
            _ if running.guarding => {
                let mut step = self.commit(String::new());
                step.ask = Some(Ask::Discard(call_id));
                return step;
            }
            _ if settled && !replaced => {
                let step = self.commit(String::new());
                return self.keys_free(Some(call_id), step);
            }
            _ => return Step { settled, ..self.commit(String::new()) },
        };
        if kind.hidden() && self.contained.contains(&call_id) {
            return Step { settled, ..self.note(HIDDEN_INPUT_SANDBOXED, size) };
        }
        if !can_ask {
            let note = if kind.hidden() { HIDDEN_INPUT_ELSEWHERE } else { VISIBLE_INPUT_ELSEWHERE };
            return Step { settled, ..self.note(note, size) };
        }
        // One question at a time, so an approval and an input never overlap. Tool calls
        // run one after another, so an approval does not come while a command runs.
        if approval_pending {
            return Step { settled, ..Step::default() };
        }
        self.ask_for(call_id, kind, size)
    }

    /// Asks for an answer line of `kind` for the running call `call_id`, below its last
    /// output line.
    fn ask_for(&mut self, call_id: CallId, kind: AnswerKind, size: Size) -> Step {
        let Some(running) = self.running.as_mut().filter(|running| running.call_id == call_id)
        else {
            return Step::default();
        };
        running.asking = Some(kind);
        let prompt = format::one_line(&running.tail);
        let mut step = if self.terminal() {
            self.commit(String::new())
        } else {
            let options = self.options_at(size);
            let mut err = String::new();
            if !prompt.is_empty() {
                err.push_str(&render_trace(&prompt, &options));
            }
            err.push_str(kind.line());
            err.push('\n');
            let mut err = self.raw_err(err);
            // A shown answer is echoed on the next line as it is typed.
            if kind.shown() {
                err.push_str(ECHO_PREFIX);
                self.echo_line = true;
            }
            Step { err, ..Step::default() }
        };
        step.ask = Some(Ask::Input { call_id, kind });
        step
    }

    /// The running call, made `call_id` when it was another one or none; true when
    /// another one was asking for input, which therefore stops.
    fn running(&mut self, call_id: CallId) -> (&mut Running, bool) {
        let mut replaced = false;
        if let Some(running) = &self.running
            && running.call_id != call_id
        {
            replaced = running.reads_keys();
            self.running = None;
        }
        (self.running.get_or_insert_with(|| Running::new(call_id)), replaced)
    }

    /// Call `call_id` completed: its tail, any question for it and the keys kept for it
    /// go. True when keys were read for it.
    fn call_ended(&mut self, call_id: CallId) -> bool {
        let retained = self.retained == Some(call_id);
        if retained {
            self.retained = None;
        }
        self.interactive.remove(&call_id);
        let asked = match self.running.take() {
            Some(running) if running.call_id == call_id => running.reads_keys(),
            other => {
                self.running = other;
                false
            }
        };
        asked || retained
    }

    /// What the user typed so far for a shown answer, echoed below the prompt: in the
    /// live zone on a terminal, otherwise on stderr after `> `, where a removed
    /// character is erased with Backspace. An answer that is not shown is never passed
    /// here, and text passed while no shown answer is asked for is dropped.
    pub(crate) fn typed(&mut self, text: &str) -> Step {
        let terminal = self.terminal();
        let echo_line = self.echo_line;
        let Some(running) = &mut self.running else {
            return Step::default();
        };
        if !running.asking.is_some_and(AnswerKind::shown) {
            return Step::default();
        }
        if terminal {
            text.clone_into(&mut running.typed);
            return self.commit(String::new());
        }
        // A note since the last key ended the echo line; a new one starts empty.
        let (mut err, shown) = if echo_line {
            (String::new(), running.typed.as_str())
        } else {
            (ECHO_PREFIX.to_owned(), "")
        };
        err.push_str(&echo_edit(shown, text));
        text.clone_into(&mut running.typed);
        self.echo_line = true;
        Step { err, ..Step::default() }
    }

    /// An answer line that is not shown starts with `count` characters typed ahead while
    /// the call asked nothing: a note says so, because nothing else on the screen does,
    /// and a stray key in front of a password fails it.
    pub(crate) fn typed_ahead(&mut self, count: usize, size: Size) -> Step {
        let unit = if count == 1 { "character" } else { "characters" };
        let line = format!("the answer starts with {count} {unit} typed ahead; Ctrl+U clears them");
        self.note(&line, size)
    }

    /// The answer reached the command.
    pub(crate) fn answer_sent(&mut self, size: Size) -> Step {
        self.answer_note(ANSWER_SENT, size)
    }

    /// The daemon refused the answer because the command no longer waits for it.
    pub(crate) fn answer_refused(&mut self, size: Size) -> Step {
        self.answer_note(ANSWER_REFUSED, size)
    }

    /// The daemon refused the answer for another reason, which `message` gives.
    pub(crate) fn answer_failed(&mut self, message: &str, size: Size) -> Step {
        let line = format!("the answer was not sent: {}", format::one_line(message));
        self.answer_note(&line, size)
    }

    /// A note about an answer; the echo of what was typed goes with it.
    /// A manual answer asks once: after it, the keys stop and the call's silence starts
    /// again.
    fn answer_note(&mut self, text: &str, size: Size) -> Step {
        let mut freed = None;
        if let Some(running) = &mut self.running {
            running.typed.clear();
            if running.asking.is_some_and(AnswerKind::manual) {
                running.asking = None;
                running.stirred();
                freed = Some(running.call_id);
            }
        }
        let step = self.note(text, size);
        match freed {
            Some(call) => self.manual_closed(Some(call), step),
            None => step,
        }
    }

    /// The user answered the approval `call_id` with a key. Allowing a call that may
    /// wait for input at the terminal keeps the keys for it ([`Ask::Retain`]).
    pub(crate) fn answered(
        &mut self,
        call_id: CallId,
        decision: ApprovalDecision,
        size: Size,
    ) -> Step {
        self.asking = None;
        self.answered = Some(call_id);
        if decision == ApprovalDecision::Deny {
            self.refused.insert(call_id);
        }
        if let Some(call) = self.call.as_mut().filter(|call| call.call_id == call_id) {
            call.approved(None);
        }
        let step = self.note(format::decision(decision), size);
        if decision == ApprovalDecision::Allow && self.interactive.contains(&call_id) {
            self.retained = Some(call_id);
            return Step { ask: Some(Ask::Retain(call_id)), ..step };
        }
        step
    }

    /// Ends the view early: commits what the current message has so far and clears the
    /// live zone, before an error or an interrupt is reported.
    pub(crate) fn close(&mut self) -> Step {
        self.asking = None;
        self.surface = None;
        self.running = None;
        self.call = None;
        self.retained = None;
        self.ended = true;
        self.end_progress.get_or_insert(progress::CLEAR);
        let committed = self.finish_message();
        // An echo line left open would carry what is written after the view.
        let err = self.raw_err(String::new());
        Step { err, ..self.commit(committed) }
    }

    /// An update of message `index`: `delta` at byte `offset` of its text. An update
    /// that starts past what this view holds, because it joined in the middle of the
    /// message, is skipped; the completed message fills the gap.
    fn message_delta(&mut self, index: u32, offset: u64, delta: &str, size: Size) -> Step {
        let held = self
            .message
            .as_ref()
            .filter(|message| message.index == index)
            .map_or("", Message::received);
        let Some(before) = usize::try_from(offset).ok().and_then(|offset| held.get(..offset))
        else {
            return Step::default();
        };
        let text = format!("{before}{delta}");
        self.message_text(index, &text, false, size)
    }

    fn message_text(&mut self, index: u32, text: &str, complete: bool, size: Size) -> Step {
        if index < self.next_index {
            return Step::default();
        }
        let mut committed = String::new();
        if self.message.as_ref().is_some_and(|message| message.index != index) {
            committed.push_str(&self.finish_message());
        }
        if self.message.is_none() {
            if self.terminal() {
                committed.push_str(self.spacing.before(Block::Message));
            } else if self.raw_messages > 0 {
                committed.push('\n');
            }
            self.message = Some(Message::new(index, self.options_at(size)));
        }
        let terminal = self.terminal();
        if let Some(message) = &mut self.message {
            message.receive(text);
            // On a terminal the text goes into the renderer at the pace of the frames.
            if !terminal {
                committed.push_str(&message.push_all());
            }
        }
        if complete {
            committed.push_str(&self.finish_message());
        }
        if !terminal {
            return Step { out: committed, ..Step::default() };
        }
        // NOTE: not `stage`, which would push the waiting text of this message before
        // the output that comes ahead of it.
        self.pending.push_str(&committed);
        self.dirty = true;
        Step::default()
    }

    /// Finishes the current message and returns the rest of its output.
    fn finish_message(&mut self) -> String {
        let Some(message) = self.message.take() else {
            return String::new();
        };
        self.next_index = message.index.saturating_add(1);
        let text = message.received();
        let open_line = !text.is_empty() && !text.ends_with('\n');
        let mut rest = message.finish();
        if !self.terminal() {
            self.raw_messages += 1;
            if open_line {
                rest.push('\n');
            }
        }
        rest
    }

    fn approval(
        &mut self,
        call_id: CallId,
        request: &Request<'_>,
        size: Size,
        can_ask: bool,
    ) -> Step {
        let before = self.finish_message();
        let options = self.options_at(size);
        let record = self.exits.get(&call_id);
        // NOTE: an exit shows the whole line from its record, never a shortened summary.
        let (heading, asking) = match request.exit.and_then(|_| sandbox::exit_heading(record)) {
            Some(heading) => (heading, None),
            None => {
                let tool = self.tools.get(&call_id).map(String::as_str);
                let command = self.commands.get(&call_id).map(String::as_str);
                format::approval_heading(request.summary, tool.zip(command))
            }
        };
        let mut text = format::paint(request.heading, Tone::Attention, &options);
        for (at, line) in heading.iter().enumerate() {
            if at == 0 {
                text.push(' ');
            }
            text.push_str(line);
            text.push('\n');
        }
        if let Some(asking) = asking {
            text.push_str(&asking);
            text.push('\n');
        }
        if let Some(exit) = request.exit {
            for line in sandbox::exit_lines(exit, record, self.home.as_deref()) {
                for (piece, tone) in &line {
                    text.push_str(&format::paint(piece, *tone, &options));
                }
                text.push('\n');
            }
        }
        if let Some(diff) = request.diff {
            if self.terminal() {
                text.push_str(&render(&format::code_block("diff", diff), &options));
            } else {
                text.push_str(&format::lines(diff));
                if !diff.ends_with('\n') {
                    text.push('\n');
                }
            }
        }
        let mut step = Step { out: before, ..Step::default() };
        // Calls run one after another, so a call that kept keys is over by now.
        self.retained = None;
        if can_ask {
            self.asking = Some(call_id);
            step.ask = Some(Ask::Approval(call_id));
            // One question at a time: the approval's key reader replaces the input's.
            if let Some(running) = &mut self.running {
                running.asking = None;
                running.guarding = false;
                running.typed.clear();
                running.hinted = false;
            }
            if !self.terminal() {
                text.push_str(QUESTION);
                text.push('\n');
            }
        } else {
            text.push_str(&render_trace("waiting for another client to answer", &options));
        }
        self.show_question(step, &text)
    }

    /// Clears the question when it was about `call_id`; true when it was.
    fn settle(&mut self, call_id: CallId) -> bool {
        let settled = self.asking == Some(call_id);
        if settled {
            self.asking = None;
        }
        settled
    }

    fn end(&mut self, end: TurnEnd, note: Option<String>, size: Size) -> Step {
        let input = self.running.as_ref().is_some_and(Running::reads_keys);
        let settled = self.asking.take().is_some()
            || self.surface.take().is_some()
            || input
            || self.retained.is_some();
        let failed = matches!(end, TurnEnd::Failed(_));
        self.end_progress = Some(if failed { progress::FAILED } else { progress::CLEAR });
        let mut step = self.close();
        if let Some(note) = note {
            let noted = self.note(&note, size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        Step { settled, end: Some(end), ..step }
    }

    /// `text` for stderr when stdout is not a terminal, on a line of its own: an open
    /// echo line ends first.
    fn raw_err(&mut self, text: String) -> String {
        if std::mem::take(&mut self.echo_line) { format!("\n{text}") } else { text }
    }

    /// Writes `committed` once: through the live zone on a terminal, as it is
    /// otherwise.
    fn commit(&mut self, committed: String) -> Step {
        if self.terminal() {
            self.stage(&committed);
            Step::default()
        } else {
            Step { out: committed, ..Step::default() }
        }
    }

    /// Keeps `committed` for the next frame, after the text that the current message
    /// still holds back, and marks the live zone for that frame.
    fn stage(&mut self, committed: &str) {
        if !committed.is_empty()
            && let Some(message) = &mut self.message
        {
            let held = message.push_all();
            self.pending.push_str(&held);
        }
        self.pending.push_str(committed);
        self.dirty = true;
    }

    /// The line of call `call`, which ended as `outcome` at the time of the event
    /// being taken, written once; a failed call keeps `lines`, the last of its output.
    fn call_line(&mut self, call: &Call, outcome: Outcome<'_>, lines: &[String], size: Size) {
        let options = self.options_at(size);
        let mut committed = self.spacing.before(Block::Note).to_owned();
        committed.push_str(&call.ended(self.event_at, outcome, &options));
        if outcome.failed() {
            committed.push_str(&call::tail(lines, false, &options));
        }
        self.stage(&committed);
    }

    /// The live zone above the status row at `now`: the current message's live text,
    /// the running call's line, the last lines of its output and the input it waits
    /// for, then the question when one is pending.
    fn live_body(&self, size: Size, now: Timestamp) -> (String, Option<Measured>) {
        let (mut live, mut measured) = match &self.message {
            Some(message) => {
                let (live, measured) = message.live();
                (live.to_owned(), measured)
            }
            None => (String::new(), None),
        };
        let options = self.options_at(size);
        let before = live.len();
        if self.call_shown()
            && let Some(call) = &self.call
        {
            live.push_str(self.spacing.peek(Block::Note));
            let spinner = match &self.status {
                Some(status) => status.spinner(now),
                None => spinner(self.look.motion, 0),
            };
            live.push_str(&call.running(spinner, now, &options));
        }
        if let Some(running) = &self.running {
            live.push_str(&call::tail(&running.lines, running.asking.is_some(), &options));
            if let Some(kind) = running.asking {
                live.push_str(&format::paint(kind.line(), Tone::Plain, &options));
                live.push('\n');
                if kind.shown() {
                    live.push_str(ECHO_PREFIX);
                    live.push_str(&format::one_line(&running.typed));
                    live.push('\n');
                }
            } else if running.hinted && !self.question_pending() {
                live.push_str(&format::paint(SILENCE_HINT, Tone::Dim, &options));
                live.push('\n');
            }
        }
        if live.len() != before {
            measured = None;
        }
        let question = if self.asking.is_some() {
            Some(QUESTION)
        } else if self.surface.is_some() {
            Some(sandbox::KEEP_QUESTION)
        } else {
            None
        };
        if let Some(question) = question {
            live.push_str(&format::keys(question, &options));
            live.push('\n');
            measured = None;
        }
        (live, measured)
    }
}

/// What turns the echo `shown` into `text` on a line that a terminal shows: each
/// character after their common start is erased with Backspace, space, Backspace per
/// column it takes, then the rest of `text` is written. The text never holds a control
/// character: the answer line drops them.
fn echo_edit(shown: &str, text: &str) -> String {
    let common = shown
        .char_indices()
        .zip(text.chars())
        .find(|((_, a), b)| a != b)
        .map_or_else(|| shown.len().min(text.len()), |((at, _), _)| at);
    // Both strings share their first `common` bytes, which end on a character boundary.
    let (Some(removed), Some(added)) = (shown.get(common..), text.get(common..)) else {
        return String::new();
    };
    let mut edit = String::new();
    for c in removed.chars().rev() {
        for _ in 0..c.width().unwrap_or(0) {
            edit.push_str("\x08 \x08");
        }
    }
    edit.push_str(added);
    edit
}

/// The last line of `tail` with text in it, without the spaces around it; empty when
/// there is none.
fn last_line(tail: &str) -> String {
    tail.lines().rev().map(str::trim).find(|line| !line.is_empty()).unwrap_or("").to_owned()
}

/// An approval request as the view shows it.
struct Request<'a> {
    heading: &'static str,
    summary: &'a str,
    diff: Option<&'a str>,
    /// What the call would do outside the sandbox, for an exit.
    exit: Option<&'a ExitInfo>,
}

#[cfg(test)]
mod tests;
