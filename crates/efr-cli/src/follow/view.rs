//! What a followed turn looks like: the turn's events in, the text to write out.
//!
//! No IO here: the follow loop feeds events and writes what comes back, so the whole
//! appearance of a turn can be tested with events and a fixed screen size.
//!
//! On a terminal, assistant messages stream through an `efr_render::Renderer`: its
//! committed output is written once and its live zone is redrawn in place through a
//! [`LiveZone`]. A turn is a list of blocks with one blank line between them: prose,
//! tool calls (`call`), questions and notes. A pending question is a card in the live
//! zone (`format::card`): when it is answered, it gives its place to one line, such as
//! `✓ allowed`, so what runs is written once, in the block of the call. A card taller
//! than the screen is written to the scrollback instead, and its keys stay live. Every
//! colour goes through a role of the palette in the render options. When stdout is not
//! a terminal, the messages are written as raw markdown and everything else goes to
//! stderr in the same blocks and in the same order as a terminal keeps them: a
//! question and its answer before the rows of its call. stdout holds the reply alone.
//!
//! While the turn waits behind another one, the other turn's approvals show too, and so
//! do its running call's last output line and the input it waits for: that turn may be
//! parked on a question nobody else will answer, and the prompt runs only once it is
//! answered. This client tells the daemon that a person here can answer, so it must
//! ask for the running turn's input as well.
//!
//! While a tool call of this turn runs on a terminal, its block (`call`) sits in the
//! live zone with the spinner and its time, in place of the status row, and the last
//! three lines of its output with text in them follow, muted and cut to the width. When
//! the call ends, its block is written once in its place, with its result, and a failed
//! call keeps its last lines of output. When the call's command waits for input and
//! keys can be read, the view
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
//! shell, or go to the input row.
//!
//! With the input row, a running call whose open last line of output (one without a
//! line break at its end) looks like a password
//! prompt (`efr_protocol::looks_secret`) holds the keys the same way until the daemon
//! reports its wait ([`Running::held`]): the daemon reports a wait only after a quiet
//! time, and a password typed meanwhile must never land in the row or go as a steer. A
//! new last line, or the call's end, lets them go, and the pending text is thrown away.
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
//! The model's context shows as a gauge, `ctx 43%` in the colour of its level
//! (`format::context`): in the status row and in the running call's line, from the
//! newest `context` draft, and in the end-of-turn line, from the end event. While the
//! turn compacts the context (a `compacting` draft), the status row says `compacting
//! context`; each `conversation_compacted` of the turn ends that and leaves one muted
//! line in the scrollback, such as `context compacted (auto): 231k -> 24k tokens, kept
//! 3 turns, summary 3.2k`.
//!
//! What a call changed comes with its end: a file write shows the first lines of its
//! diff in its block (`render.diff_lines`), a patch the first lines of each file's
//! diff, and a call that changed files without a
//! diff shown gets one muted row of them under its result (`format::changes`). A turn
//! that changed files ends with one muted line that counts them, right before the line
//! of its time and tokens. Before both, however the turn ended, comes one muted line for
//! each file outside the project that runs code later and that the turn changed, such
//! as `changed ~/.zshrc; efr undo reverts it` (or, in an event from before the home
//! store, `changed ~/.zshrc; backup at <path>`), and for each file outside the roots
//! that efr could not copy, shown whole. An `auto` turn that `efr undo` can take back
//! says so in its line instead, also when it failed or was interrupted: `changed 4
//! files (+12 -3) in <project> and 2 in ~; efr undo reverts this turn`, then a line
//! `not covered: ...` for what no snapshot holds.
//!
//! In `auto`, a turn with a call that ran in the sandbox ends with one muted line that
//! says where the sandbox can write; the result of a failed call names no sandbox.
//! An approval for an exit shows the whole line of the call, what a "yes" allows and
//! why, the full-rights warning and every program word of a line that runs outside the
//! sandbox (with the untrusted mark for a program that the sandbox wrote), efr's own
//! facts and the model's reason, labelled as the model's. While the reviewer of `auto`
//! judges an exit, the status row says `reviewing: <line>`; its allow or refusal is
//! one muted line before the call's rows, and an unsure verdict is a row of the card
//! that follows. An exit question that offers an always rule (phase E) shows `a` with
//! the rule in its keys row; after an `a`, the row asks to confirm the rule
//! ([`TurnView::confirm_always`]). The line of what the rule became follows the answer,
//! and a call that a rule covered gets one muted line, as a reviewer's allow does. A
//! turn whose mode fell back says so at its start. When a call changed git settings
//! that run programs, the
//! quarantine question ([`Ask::Surface`]) asks whether to keep them, with its own
//! question id: it is not an approval of a call.
//!
//! A command that a shell call left running in the hidden shell (`left`) belongs to the
//! conversation too: the call's block says that it still runs, its waits ask for an
//! answer line that takes the place of the input row at once and whose Ctrl+C stops the
//! command (`shell.interrupt`), and its end is a note. At the end of the last followed
//! turn one line names it; while it waits for an answer here, `efr` stays open with that
//! line ([`TurnView::lingers`]) until the wait ends or Esc leaves it.
//!
//! A background job belongs to the conversation, not to a turn: the end of a job, a
//! line that matched its `watch` pattern and a notice that did not start a turn show
//! while the view follows any turn of the conversation (`format::jobs`). A call that
//! starts a job says `(background)`, and its block names the job. A turn that a job
//! notice started is marked once. Ctrl+X asks whether to stop the newest running job
//! ([`TurnView::stop_question`]).

mod call;
mod input;
mod left;
mod message;
mod status;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use efr_config::DiffGroups;
use efr_protocol::{
    ApprovalDecision, ApprovedBy, CallId, ContextUse, Draft, DraftPart, ErrorBody, Event,
    EventEnvelope, ExitInfo, FileChanges, Grant, InputWait, JobId, JobInfo, JudgeKind, Launch,
    Mode, Origin, Presence, QuestionId, Risk, Scope, Seq, ShellCommandEnd, SurfaceChange, TurnId,
    TurnInterruptResult, TurnUndoInfo, Verdict, looks_secret,
};
use efr_render::{RenderOptions, render_trace};
use jiff::Timestamp;
use unicode_width::UnicodeWidthChar as _;

use crate::format::card::{self, Card, Footer};
use crate::format::changes::HomeChange;
use crate::format::context::{self as gauge, GaugedLine};
use crate::format::{self, Block, Spacing, Tone, jobs, sandbox};
use crate::keys::Key;
use crate::live::{LiveZone, Measured, effective_width, fits, rows_of};
use crate::progress;
use crate::row::Action;
use crate::terminal::{Size, at_width};
use call::{Call, Outcome};
use input::{Input, Queued, user_message};
use left::Left;
use message::Message;
use status::{State, Status};

pub(crate) use status::{TICK, spinner};

/// Hides the cursor while the status row shows.
const HIDE_CURSOR: &str = "\x1b[?25l";

/// Shows the cursor again.
const SHOW_CURSOR: &str = "\x1b[?25h";

/// Turns bracketed paste on while the input row shows: a pasted newline never sends.
const PASTE_ON: &str = "\x1b[?2004h";

/// Turns bracketed paste off again.
const PASTE_OFF: &str = "\x1b[?2004l";

/// The note when an interrupt sent the unread steers again as a new prompt.
const RESENT: &str = "interrupted to send your message";

/// The note when the followed prompt was taken back before it ran.
const WITHDRAWN: &str = "the prompt was taken back before it ran";

/// The line at the start of a turn in full mode, in the error role. Undo covers the
/// project and the files outside it that a line named, not what a script did.
const FULL_STARTED: &str =
    "full mode: no sandbox, no questions; efr undo reverts the project and the files efr saw";

/// What starts the title of an approval of the turn that the followed one waits
/// behind.
const BLOCKING: &str = "the running turn asks: ";

/// The mark of a good answer.
const YES: &str = "\u{2713} ";

/// The mark of a refusal or of a failure.
const NO: &str = "\u{2717} ";

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

/// The note when a contained call waits for hidden input, or for a visible answer whose
/// prompt looks like a password prompt: efr stops it and never asks (efr's auto spec,
/// sections 3.12 and 14.7).
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

/// How much further a row of a line of efr's own goes on than its first row, as a row
/// of a question's card does.
const GOES_ON: usize = 2;

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
    /// Its prompt was taken back before it started (`prompt_withdrawn`).
    Withdrawn,
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
    /// The question of the stop key, answered with one key: `y` stops the job that the
    /// view asks about ([`TurnView::stop_answered`]), `n` lets it run.
    StopJob,
}

/// What a question on the screen asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum About {
    Approval(CallId),
    Surface(QuestionId),
    StopJob,
}

/// The question on a terminal, as a card in the live zone until it is answered.
#[derive(Debug)]
struct Shown {
    card: Card,
    about: About,
    footer: Footer,
    /// The card is in the scrollback, because it did not fit on the screen; only its
    /// keys stay in the live zone.
    committed: bool,
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
    /// Its last line of output looks like a password prompt and no wait was reported
    /// yet: with the input row, the keys wait in a line that is never shown or sent
    /// ([`Ask::Retain`]), so a password typed now never lands in the row.
    held: bool,
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
            held: false,
            typed: String::new(),
            takes_manual: false,
            activity: 0,
            hinted: false,
        }
    }

    /// True while keys are read for it, to answer, to hold or to throw away.
    fn reads_keys(&self) -> bool {
        self.asking.is_some() || self.guarding || self.held
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
        // NOTE: keys held for a prompt that looks like a password prompt still let the
        // user type a manual answer for it.
        self.takes_manual && self.wait == InputWait::None && self.asking.is_none() && !self.guarding
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
    /// The lines of a file write's diff that show under its call; 0 shows the row of
    /// the files instead (`render.diff_lines`).
    pub(crate) diff_lines: usize,
}

/// What a call changed in the files of the turn's roots, from its
/// `tool_call_completed`.
#[derive(Debug, Clone, Copy, Default)]
struct Changed<'a> {
    changes: Option<&'a FileChanges>,
    /// The unified diff of a file tool's write.
    diff: Option<&'a str>,
}

/// The wait of a command left running in the hidden shell, from its event.
struct Wait<'a> {
    input: InputWait,
    looks_secret: bool,
    prompt: &'a str,
}

/// What a view keeps of background jobs.
#[derive(Debug, Default)]
struct Jobs {
    /// The line of each job that a call of this turn started (`job_started`), for the
    /// call's block when it ends.
    lines: HashMap<CallId, String>,
    /// The turns that a job notice started, whose mark showed.
    woken: HashSet<TurnId>,
    /// The job that the question of the stop key asks about, while it waits for a key.
    stopping: Option<JobId>,
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
    /// The blocks on stderr when stdout is not a terminal.
    err_spacing: Spacing,
    /// The question on a terminal's screen.
    question: Option<Shown>,
    /// The size of the screen at the last event or frame.
    size: Size,
    /// Raw markdown messages written so far, to separate them by a blank line.
    raw_messages: u32,
    /// The approval waiting for a key.
    asking: Option<CallId>,
    /// The rule that an `a` answer to the approval [`asking`](Self::asking) saves, in
    /// words and safe to print, when its question offers one (phase E).
    offered: Option<String>,
    /// The call whose question got an `a` answer, until the line of what it saved: when
    /// stdout is not a terminal, the first rows of the call wait for that line, as they
    /// wait for an answer.
    saving: Option<CallId>,
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
    /// Calls whose approval said that they may wait for input at the terminal, such as
    /// for a password: when one is left running in the hidden shell, the keys wait for
    /// its first wait.
    flagged: HashSet<CallId>,
    /// The command that a shell call left running in the conversation's hidden shell,
    /// until it ends.
    // NOTE: boxed, so a view that follows a turn keeps the future of `efr send` small.
    left: Option<Box<Left>>,
    /// The home directory, which the sandbox's lines print as `~`.
    home: Option<PathBuf>,
    /// The scope of the prompt, such as `app` or `○ ~/notes`, at the end of the status
    /// row (DV52).
    scope: Option<String>,
    /// The turn runs in a registered project, which its sandbox can write.
    in_project: bool,
    /// A call of this turn runs in the sandbox: the end of the turn says where it can
    /// write.
    traced: bool,
    /// A contained call of this turn had the network view: the end of the turn says so.
    traced_net_view: bool,
    /// The most network that a contained call of this turn had (efr's proxy, also for a
    /// job's start, or the whole network): the end of the turn says so.
    traced_network: sandbox::TraceNetwork,
    /// Calls that run in the sandbox, whose failure says so, each with whether it has
    /// the network view (`Grant::NetView`) and the whole network (`Grant::OpenNetwork`).
    contained: HashMap<CallId, (bool, bool)>,
    /// The exit of each call, for the lines of its approval and of the reviewer.
    exits: HashMap<CallId, sandbox::Requested>,
    /// The row of what the reviewer thought of a call's exit before it handed the
    /// question over, for the card when the question does not carry it.
    reviewed: HashMap<CallId, String>,
    /// The command of each shell call, whose approval shows each of its lines.
    commands: HashMap<CallId, String>,
    /// efr counts the user away from the conversation (`presence_changed`): the status
    /// row says so, and so does the card of a question, which waits only a short time.
    away: bool,
    /// What the lines at the end of the turn name, gathered while it runs.
    // NOTE: boxed, so a view that follows a turn keeps the future of `efr send` small.
    ending: Box<Ending>,
    /// What the turn changed and what `efr undo` can take back of it, from its end
    /// event, for the lines before the end.
    // NOTE: boxed, so a view that follows a turn keeps the future of `efr send` small.
    ended_undo: Option<Box<(Option<FileChanges>, TurnUndoInfo)>>,
    /// The quarantine question waiting for a key.
    surface: Option<QuestionId>,
    /// The quarantine question that this client answered, whose answer needs no second
    /// note.
    surface_answered: Option<QuestionId>,
    /// Every quarantine question shown, of this turn or of the turn it waits behind.
    surfaces: HashSet<QuestionId>,
    /// What the view keeps of background jobs.
    // NOTE: boxed, so a view that follows a turn keeps the future of `efr send` small.
    jobs: Box<Jobs>,
    /// What `config.toml` switches.
    look: Look,
    /// The groups of `[diff.groups]`, for the line of the files at the end of a turn.
    groups: DiffGroups,
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
    /// The daemon's time of the event that is being taken, and of the turn's start.
    event_at: Option<Timestamp>,
    started_at: Option<Timestamp>,
    /// The sequence number of the newest event of this turn that ends what a draft
    /// shows ([`ends_drafts`]); a draft made before it is old. Other events, such as a
    /// steer that the conversation records and not the turn, move nothing: the drafts
    /// after them still name the turn's own last event.
    draft_boundary: Option<Seq>,
    /// The sequence number of the event that is being taken.
    event_seq: Option<Seq>,
    /// The input row, on a terminal whose keys the follow loop reads for it.
    input: Option<Input>,
    /// This view asked to interrupt the followed turn (Esc).
    interrupting: bool,
    /// The interrupt of this view sent its unread steers again: the note that says so
    /// waits for the end of the followed turn, so that it comes after what the turn
    /// still shows, such as the call that the interrupt stopped.
    resent_note: bool,
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
            err_spacing: Spacing::default(),
            question: None,
            size: Size::default(),
            raw_messages: 0,
            asking: None,
            offered: None,
            saving: None,
            answered: None,
            queued: false,
            blocking: HashSet::new(),
            refused: HashSet::new(),
            running: None,
            call: None,
            echo_line: false,
            interactive: HashSet::new(),
            retained: None,
            flagged: HashSet::new(),
            left: None,
            home: None,
            scope: None,
            in_project: false,
            traced: false,
            traced_net_view: false,
            traced_network: sandbox::TraceNetwork::None,
            contained: HashMap::new(),
            exits: HashMap::new(),
            reviewed: HashMap::new(),
            commands: HashMap::new(),
            away: false,
            ending: Box::default(),
            ended_undo: None,
            surface: None,
            surface_answered: None,
            surfaces: HashSet::new(),
            jobs: Box::default(),
            look: Look::default(),
            groups: DiffGroups::default(),
            pending: String::new(),
            dirty: false,
            status: None,
            ended: false,
            end_progress: None,
            progress_sent: None,
            cursor_hidden: false,
            event_at: None,
            started_at: None,
            draft_boundary: None,
            event_seq: None,
            input: None,
            interrupting: false,
            resent_note: false,
        }
    }

    /// The same view with `look`.
    pub(crate) fn with_look(mut self, look: Look) -> TurnView {
        self.look = look;
        self
    }

    /// The same view with the groups of `[diff.groups]`.
    pub(crate) fn with_groups(mut self, groups: DiffGroups) -> TurnView {
        self.groups = groups;
        self
    }

    /// The same view with the input row, on a terminal: the follow loop reads its keys.
    pub(crate) fn with_input(mut self) -> TurnView {
        if self.terminal() {
            self.input = Some(Input::default());
        }
        self
    }

    /// The turn that the view follows now. It changes when the turn ends and a prompt
    /// that this view queued runs next.
    pub(crate) fn turn(&self) -> TurnId {
        self.turn
    }

    /// True while the input row can take keys: it exists and the view did not end.
    pub(crate) fn has_row(&self) -> bool {
        self.input.is_some() && !self.ended
    }

    /// Keys come to the input row (`shown`), or something else takes them meanwhile:
    /// a question or an answer line. The row hides then and keeps its text.
    pub(crate) fn show_row(&mut self, shown: bool) {
        if let Some(input) = &mut self.input
            && input.shown != shown
        {
            input.shown = shown;
            self.dirty = true;
        }
    }

    /// Takes a key for the input row and says what it asks for.
    pub(crate) fn row_key(&mut self, key: Key) -> Action {
        let Some(input) = &mut self.input else {
            return Action::None;
        };
        let action = input.line.key(key);
        if action == Action::Edited {
            self.dirty = true;
        }
        action
    }

    /// True while a bracketed paste into the input row has started and not ended.
    pub(crate) fn row_pasting(&self) -> bool {
        self.input.as_ref().is_some_and(|input| input.line.pasting())
    }

    /// The keys come back to the input row after a question: an escape sequence that
    /// was cut by the question is dropped, and a paste goes on.
    pub(crate) fn row_resumed(&mut self) {
        if let Some(input) = &mut self.input {
            input.line.drop_partial();
        }
    }

    /// Ends a paste into the input row whose end did not come in time.
    pub(crate) fn row_end_paste(&mut self) {
        if let Some(input) = &mut self.input {
            input.line.end_paste();
            self.dirty = true;
        }
    }

    /// The text of the input row; empty without one.
    pub(crate) fn row_text(&self) -> &str {
        self.input.as_ref().map_or("", |input| input.line.text())
    }

    /// True when the input row holds text that is not blank, which Enter and Tab send.
    pub(crate) fn row_ready(&self) -> bool {
        self.input.as_ref().is_some_and(|input| !input.line.is_blank())
    }

    /// Takes the text of the input row and leaves it empty.
    pub(crate) fn row_take(&mut self) -> String {
        self.dirty = true;
        self.input.as_mut().map(|input| input.line.take()).unwrap_or_default()
    }

    /// Clears the input row; true when it held text.
    pub(crate) fn row_clear(&mut self) -> bool {
        let cleared = self.input.as_mut().is_some_and(|input| input.line.clear());
        self.dirty |= cleared;
        cleared
    }

    /// Puts `text` into the input row after its text, on a line of its own.
    pub(crate) fn row_append(&mut self, text: &str) {
        if let Some(input) = &mut self.input {
            input.line.append(text);
            self.dirty = true;
        }
    }

    /// The daemon recorded `text` as steer `seq` of this view: it waits for a model call
    /// to read it.
    pub(crate) fn steered(&mut self, seq: Seq, text: String) {
        if let Some(input) = &mut self.input {
            input.steered(seq, text);
            self.dirty = true;
        }
    }

    /// The daemon queued `text` as the prompt of `turn` for this view. `late` for a
    /// steer that came after its turn stopped reading steers.
    pub(crate) fn queued_prompt(&mut self, turn: TurnId, text: String, late: bool) {
        if let Some(input) = &mut self.input {
            input.queued(turn, text, late);
            self.dirty = true;
        }
    }

    /// The unread steers of this view, by sequence number.
    pub(crate) fn unread_steers(&self) -> Vec<Seq> {
        self.input.as_ref().map(Input::unread).unwrap_or_default()
    }

    /// The prompts that this view queued and that did not start, in queue order.
    pub(crate) fn queued_turns(&self) -> Vec<TurnId> {
        self.input.as_ref().map(Input::queued_turns).unwrap_or_default()
    }

    /// The newest prompt that this view queued and that did not start.
    pub(crate) fn newest_queued(&self) -> Option<TurnId> {
        self.input.as_ref().and_then(Input::newest)
    }

    /// The prompt of `turn` left the queue: it started, or someone took it back. Its
    /// text stays where it is.
    pub(crate) fn drop_queued(&mut self, turn: TurnId) {
        if let Some(input) = &mut self.input
            && input.remove(turn).is_some()
        {
            self.dirty = true;
        }
    }

    /// The prompt of `turn`, which this view queued, was taken back with `text`: it
    /// leaves the list, and its text comes into the input row after the text there.
    pub(crate) fn withdrawn(&mut self, turn: TurnId, text: &str) {
        self.drop_queued(turn);
        self.row_append(text);
    }

    /// What an interrupt from the input row (Esc) did: the prompts it took back come
    /// into the row, and the unread steers that it sent again are now a prompt that
    /// runs next, which this view follows, with a note that says so. The note comes
    /// at the end of the interrupted turn, after the call that the interrupt stopped;
    /// when that turn already ended, it comes now.
    pub(crate) fn interrupt_result(&mut self, result: &TurnInterruptResult, size: Size) -> Step {
        let following = result.turn_id == self.turn && !self.ended;
        // The interrupt is this view's own: its request needs no note.
        if following {
            self.interrupting = true;
        }
        for withdrawn in &result.withdrawn {
            self.withdrawn(withdrawn.turn_id, &withdrawn.text);
        }
        let Some(resent) = &result.resent else {
            return self.commit(String::new());
        };
        if let Some(input) = &mut self.input {
            input.resent(resent.turn_id, &resent.steers);
        }
        if following {
            self.resent_note = true;
            return self.commit(String::new());
        }
        self.note(RESENT, size)
    }

    /// The connection to the daemon ended for good: the prompt that the view follows
    /// and that did not start, the steers that no model call read and the prompts that
    /// this view queued come back into the input row, oldest first, because nothing
    /// says whether they will run. True when any came back.
    pub(crate) fn give_back_pending(&mut self) -> bool {
        let queued = self.queued;
        let Some(input) = &mut self.input else {
            return false;
        };
        let mut texts: Vec<String> = Vec::new();
        if queued {
            texts.extend(input.prompt.take());
        }
        texts.extend(input.take_unread());
        while let Some(prompt) = input.next() {
            texts.push(prompt.text);
        }
        for text in &texts {
            input.line.append(text);
        }
        self.dirty = true;
        !texts.is_empty()
    }

    /// What an interrupt with Ctrl+C took back: the steers and the prompts that the
    /// daemon names come into the input row, in that order, and leave the lists. A
    /// steer that it does not name stays unread: a model call read it, or it stays
    /// part of the turn, so it does not come back.
    pub(crate) fn taken_back(&mut self, result: &TurnInterruptResult) {
        if let Some(input) = &mut self.input {
            let seqs: Vec<Seq> = result.withdrawn_steers.iter().map(|steer| steer.seq).collect();
            input.delivered(&seqs);
            for steer in &result.withdrawn_steers {
                input.line.append(&steer.text);
            }
            self.dirty = true;
        }
        for withdrawn in &result.withdrawn {
            self.withdrawn(withdrawn.turn_id, &withdrawn.text);
        }
    }

    /// Starts the status row, on a terminal: the prompt's line was just sent, and the
    /// row shows from the next frame until the turn ends.
    pub(crate) fn start(&mut self) {
        if self.terminal() && self.status.is_none() {
            let mut status = Status::new(self.look.motion);
            if self.queued {
                status.set(State::Queued);
            }
            status.set_away(self.away);
            status.set_scope(self.scope.clone());
            self.status = Some(status);
            self.dirty = true;
        }
    }

    /// The same view, whose status row ends with the scope of the prompt, such as `app`
    /// or `○ ~/notes`.
    pub(crate) fn with_scope(mut self, scope: Option<String>) -> TurnView {
        self.scope = scope;
        self
    }

    /// The same view, which prints paths below `home` as `~`.
    pub(crate) fn with_home(mut self, home: Option<PathBuf>) -> TurnView {
        self.home = home;
        self
    }

    /// True while an approval, the quarantine question or the question of the stop key
    /// waits for a key.
    fn question_pending(&self) -> bool {
        self.asking.is_some() || self.surface.is_some() || self.jobs.stopping.is_some()
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
            if ends_drafts(&envelope.event) {
                self.draft_boundary = Some(envelope.seq);
            }
            self.event_at = Some(envelope.at);
        }
        self.event_seq = Some(envelope.seq);
        let step = self.event(&envelope.event, size, can_ask);
        self.event_at = None;
        self.event_seq = None;
        step
    }

    /// Takes a draft: the part of this turn that the daemon sends before it records it.
    /// Text merges with the persisted updates of its message; reasoning and the input of
    /// a tool call only change the status row. A draft of another turn, or one older
    /// than an event of this turn that ends what drafts show, changes nothing.
    pub(crate) fn draft(&mut self, draft: &Draft, size: Size) -> Step {
        self.size = size;
        if draft.turn_id != self.turn
            || self.ended
            || self.draft_boundary.is_some_and(|seq| draft.after_seq < seq)
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
            DraftPart::Context(context) => {
                self.context(*context);
                Step::default()
            }
            DraftPart::Compacting { .. } => {
                self.state(State::Compacting);
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

    /// The process runs again after a stop (Ctrl+Z, then `fg`). The shell wrote its
    /// lines below the live zone and showed the cursor, so the next frame starts a new
    /// live zone below them, hides the cursor again and sends the progress bar again.
    pub(crate) fn resumed(&mut self) {
        self.live.forget();
        self.cursor_hidden = false;
        self.progress_sent = None;
        // The shell turns bracketed paste off while it runs a command such as `fg`.
        if let Some(input) = &mut self.input {
            input.paste = false;
        }
        self.dirty = true;
    }

    /// What a sudden way out must write to leave the terminal as it was: the cursor
    /// back, bracketed paste off, and no progress bar.
    pub(crate) fn restore(&self) -> String {
        let mut text = String::new();
        if self.cursor_hidden {
            text.push_str(SHOW_CURSOR);
        }
        if self.input.as_ref().is_some_and(|input| input.paste) {
            text.push_str(PASTE_OFF);
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
        self.size = size;
        if let Some(message) = &mut self.message {
            let committed = message.push_all();
            self.pending.push_str(&committed);
        }
        self.fit_question(size, now);
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
        // The input row is the last part of the live zone, and the cursor waits in it.
        let input = self.input.as_ref().filter(|input| input.shown && !self.ended);
        let tail =
            input.map(|input| input.row(&options, effective_width(size))).unwrap_or_default();
        let hide = self.ticks() && !asking && input.is_none();
        let paste = self.input.is_some() && !self.ended;
        let mut out = String::new();
        if hide && !self.cursor_hidden {
            out.push_str(HIDE_CURSOR);
            self.cursor_hidden = true;
        }
        if let Some(input) = &mut self.input
            && input.paste != paste
        {
            out.push_str(if paste { PASTE_ON } else { PASTE_OFF });
            input.paste = paste;
        }
        out.push_str(&self.live.draw(&committed, &body, measured, &row, &tail, size));
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
            || self.left.as_ref().is_some_and(|left| left.asking.is_some())
    }

    /// True when a turn ends with a line of its time and tokens: on a terminal, where
    /// it closes the turn in the scrollback. Piped output keeps its notes as they were.
    /// The note of a turn that the user stopped with Ctrl+C at `now`: on a terminal it
    /// says how long the turn ran, as the end-of-turn line does.
    pub(crate) fn interrupted(&self, now: Timestamp) -> String {
        let ran = self
            .status
            .as_ref()
            .filter(|_| self.summary())
            .map(|status| status.elapsed(now))
            .filter(|ran| !ran.is_zero());
        match ran {
            Some(ran) => format!("interrupted after {}", format::took(ran)),
            None => "interrupted".to_owned(),
        }
    }

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

    /// The model's context holds `context` now: the gauge shows it, and a count ends
    /// a compaction that the status row shows.
    fn context(&mut self, context: ContextUse) {
        if let Some(status) = &mut self.status {
            status.set_context(context);
            if *status.state() == State::Compacting {
                status.set(State::Model);
            }
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

    /// The options at the size of the last event or frame.
    fn options_now(&self) -> RenderOptions {
        self.options_at(self.size)
    }

    /// Writes the card of the question to the scrollback when the live zone with it
    /// would not fit on the screen: rows that scroll off the top can never be erased,
    /// and the live zone would show only the card's last rows. Its keys stay live.
    fn fit_question(&mut self, size: Size, now: Timestamp) {
        if !self.question.as_ref().is_some_and(|shown| !shown.committed) {
            return;
        }
        let (body, _) = self.live_body(size, now);
        // One row more for a status row under it.
        let rows = rows_of(&body, effective_width(size), self.options.width_method()) + 1;
        if !fits(rows, size) {
            self.commit_question();
        }
    }

    /// Writes the card of the question on the screen to the scrollback once, without
    /// its keys, which stay in the live zone while the question waits.
    fn commit_question(&mut self) {
        let options = self.options_now();
        let text = match &self.question {
            Some(shown) if !shown.committed => shown.card.render(None, &options),
            _ => return,
        };
        let mut committed = self.spacing.before(Block::Question).to_owned();
        committed.push_str(&text);
        if let Some(shown) = &mut self.question {
            shown.committed = true;
        }
        self.stage(&committed);
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
        self.size = size;
        // NOTE: the presence of the conversation belongs to no turn, and the view
        // follows one conversation.
        if let Event::PresenceChanged { presence, .. } = event {
            self.away = *presence == Presence::Away;
            if let Some(status) = &mut self.status {
                status.set_away(self.away);
                self.dirty = true;
            }
            return Step::default();
        }
        // NOTE: a job outlives its turn, so its end, a matched line, a notice that did
        // not start a turn and a port of it belong to the conversation; they show while
        // the view follows any turn of it.
        if let Some(line) = jobs::conversation_line(event) {
            let url = jobs::link_of(event);
            return self.rendered_after(
                String::new(),
                |options| jobs::linked(&rows(&line, Tone::Dim, options), url.as_deref(), options),
                size,
            );
        }
        // NOTE: a command left running in the hidden shell belongs to the conversation
        // too, and the shell takes no other call until it ends.
        if let Some(step) = self.left_event(event, size, can_ask) {
            return step;
        }
        if event.turn_id() != Some(self.turn) {
            return self.other_turn(event, size, can_ask);
        }
        if let Some(status) = &mut self.status {
            status.stir();
        }
        // NOTE: the question of the call and its answer come before its first rows, as
        // on a terminal; anything else of the turn shows the call first.
        let question = match event {
            Event::ExitRequested { call_id, .. }
            | Event::ExitReviewStarted { call_id, .. }
            | Event::ExitJudged { call_id, .. }
            | Event::AlwaysRuleSaved { call_id, .. }
            | Event::ApprovalRequested { call_id, .. }
            | Event::ApprovalResolved { call_id, .. }
            | Event::ApprovalExpired { call_id, .. } => {
                self.call.as_ref().is_some_and(|call| call.call_id == *call_id)
            }
            _ => false,
        };
        let header = if question { String::new() } else { self.due_header(size) };
        let step = self.turn_event(event, size, can_ask);
        Step { err: format!("{header}{}", step.err), ..step }
    }

    /// The first rows of the call of this turn, for stderr, when stdout is not a
    /// terminal and they wait; empty otherwise. They wait from the call's start until
    /// its question is answered or anything else of the turn comes, so a question shows
    /// what runs once, and its answer comes before the call, as on a terminal.
    fn due_header(&mut self, size: Size) -> String {
        if self.terminal() {
            return String::new();
        }
        let options = self.options_at(size);
        // NOTE: the question about the call shows what runs while it waits.
        let Some(call) = self.call.as_mut().filter(|call| call.header_due && !call.awaiting) else {
            return String::new();
        };
        call.header_due = false;
        let header = call.header(&options);
        let mut err = self.err_spacing.before(Block::Call).to_owned();
        err.push_str(&header);
        self.raw_err(err)
    }

    /// One event of this turn.
    fn turn_event(&mut self, event: &Event, size: Size, can_ask: bool) -> Step {
        match event {
            Event::TurnStarted { scope, settings, .. } => {
                self.queued = false;
                self.started_at = self.event_at;
                let full = settings.as_ref().is_some_and(|settings| settings.mode == Mode::Full);
                self.ending.full_exits.clear();
                if let Some(status) = &mut self.status {
                    status.turn_started();
                    status.set_full(full);
                }
                // A prompt that this view queued is the user's message of its turn.
                if let Some(prompt) = self.input.as_mut().and_then(|input| input.prompt.take()) {
                    let mut committed = self.spacing.before(Block::Prompt).to_owned();
                    committed.push_str(&user_message(&prompt, &self.options_at(size)));
                    self.stage(&committed);
                }
                self.in_project = matches!(scope, Scope::Project(_));
                // A call or a question of the turn ahead that is still shown or kept is
                // over now.
                self.call = None;
                self.commit_question();
                self.question = None;
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
                // NOTE: in the error role: commands of this turn run as the user, with
                // no sandbox and no question.
                if full {
                    let rows = |options: &RenderOptions| rows(FULL_STARTED, Tone::Failure, options);
                    let noted = self.rendered_after(String::new(), rows, size);
                    step.out.push_str(&noted.out);
                    step.err.push_str(&noted.err);
                }
                step
            }
            Event::AssistantMessageUpdated { index, offset, delta, .. } => {
                // NOTE: drafts showed most of this text before it was recorded; an update
                // that adds nothing must not take the row back from a newer draft state.
                if *index >= self.next_index && self.adds_text(*index, *offset, delta) {
                    self.state(State::Writing);
                }
                self.message_delta(*index, *offset, delta, size)
            }
            Event::AssistantMessageCompleted { index, text, .. } => {
                // Reasoning or a call's input that drafts showed after the text is newer.
                let drafted = matches!(
                    self.status.as_ref().map(Status::state),
                    Some(State::Thinking(_) | State::Preparing { .. })
                );
                if !drafted {
                    self.state(State::Model);
                }
                self.message_text(*index, text, true, size)
            }
            Event::ToolCallStarted { call_id, tool, input, manual_input, launch, .. } => {
                self.state(State::Tool(format::one_line(tool)));
                self.tools.insert(*call_id, tool.clone());
                if let Some(command) = format::command_of(input) {
                    self.commands.insert(*call_id, command.to_owned());
                }
                if let Some(Launch::Contained { grants }) = launch {
                    let open = grants.contains(&Grant::OpenNetwork);
                    self.contained.insert(*call_id, (grants.contains(&Grant::NetView), open));
                }
                // A call that takes a manual input is followed from its start, so a
                // command that never prints can still offer `Ctrl+\`. A call in the
                // sandbox never does: a manual line may hold a password, and efr types
                // no secret into the sandbox.
                let mut settled = false;
                if *manual_input && !self.contained.contains_key(call_id) {
                    let (running, replaced) = self.running(*call_id);
                    running.takes_manual = true;
                    settled = replaced;
                }
                // The model writes a tool call after the text it belongs to, so the
                // message before it is complete.
                let before = self.finish_message();
                let mut call = Call::new(*call_id, format::call_text(tool, input), self.event_at);
                if input.get("background").and_then(serde_json::Value::as_bool) == Some(true) {
                    call = call.in_background();
                }
                // On a terminal the call's block shows in the live zone while it runs
                // and is written once when it ends; elsewhere its first rows go to
                // stderr after its question and its answer, if it has one
                // (`due_header`), and its result when it ends.
                call.header_due = !self.terminal();
                self.call = Some(call);
                Step { settled, ..self.commit(before) }
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
                changes,
                diff,
                left_running,
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
                let launched = self.contained.remove(call_id);
                let contained =
                    launched.is_some() || summary.as_ref().is_some_and(|summary| summary.confined);
                let setup = summary.as_ref().and_then(|summary| summary.setup_error.as_deref());
                let denied = self.refused.contains(call_id);
                // The end of the turn says once where the sandbox can write, when a call
                // ran in it.
                let ran = contained && !denied && refusal.is_none() && setup.is_none();
                self.traced |= ran;
                let (net_view, open) = launched.unwrap_or_default();
                self.traced_net_view |= net_view && ran;
                // NOTE: the whole network wins over the proxy, which wins over none: the
                // trace names the most that a call of the turn had.
                let network = if open {
                    sandbox::TraceNetwork::Full
                } else if summary.as_ref().is_some_and(|summary| summary.proxied) {
                    sandbox::TraceNetwork::Proxy
                } else {
                    sandbox::TraceNetwork::None
                };
                if ran {
                    self.traced_network = self.traced_network.max(network);
                }
                // NOTE: the call answered, but its command goes on in the hidden shell,
                // and its `shell_command_detached` may come later. Until it does, the
                // keys of a call outside the sandbox that asked for input here, or that
                // may ask for a password, are held: they may be a password.
                if *left_running && !self.left.as_ref().is_some_and(|left| left.call_id == *call_id)
                {
                    let command = self.commands.get(call_id).map_or("", String::as_str);
                    let mut left = Left::new(*call_id, command, contained, None);
                    left.can_ask = can_ask;
                    left.holding = !contained && (settled || self.flagged.contains(call_id));
                    self.left = Some(Box::new(left));
                }
                // The job that the call started, first: it runs on after the call.
                let mut notes: Vec<String> = self.jobs.lines.remove(call_id).into_iter().collect();
                if let Some(summary) = summary {
                    notes.extend(sandbox::blocked_lines(&summary.blocked, summary.blocked_more));
                    notes.extend(sandbox::bus_refused_lines(
                        &summary.bus_refused,
                        summary.bus_refused_more,
                    ));
                    notes.extend(sandbox::background_stopped(&summary.background_stopped));
                    notes.extend(sandbox::survivors(&summary.survivors));
                }
                let step = match self.call.take().filter(|call| call.call_id == *call_id) {
                    Some(call) if denied && refusal.is_none() => {
                        self.denied_block(&call, &notes, size)
                    }
                    Some(call) => {
                        let outcome = match (refusal, setup, *exit_code) {
                            (Some(reason), _, _) => Outcome::Refused(reason),
                            (None, Some(reason), _) => Outcome::NotStarted(reason),
                            (None, None, _) if *left_running => Outcome::LeftRunning,
                            (None, None, Some(code)) if code != 0 => Outcome::Exited(code),
                            (None, None, None) if *is_error => Outcome::Failed,
                            _ => Outcome::Ran,
                        };
                        let changed = Changed { changes: changes.as_ref(), diff: diff.as_deref() };
                        self.call_block(&call, outcome, &lines, &notes, changed, size)
                    }
                    None => {
                        // The start of the call was not seen: its end is a note. A
                        // refused call never ran, in the sandbox or out of it.
                        let line = match refusal {
                            Some(reason) => Some(format::refused(tool, reason)),
                            None => format::tool_result(tool, *is_error, *exit_code)
                                .filter(|_| !denied && setup.is_none() && !*left_running),
                        };
                        let mut all: Vec<String> =
                            line.into_iter().map(|line| format!("{NO}{line}")).collect();
                        if *left_running && refusal.is_none() {
                            all.push(format!("{}: {}", format::one_line(tool), call::LEFT_RUNNING));
                        }
                        all.extend(sandbox::setup_failed(setup));
                        all.extend(changes.as_ref().and_then(changes_note));
                        all.extend(notes);
                        self.notes(&all, size)
                    }
                };
                // The live tail goes either way.
                Step { settled, ..step }
            }
            Event::ExitRequested { call_id, kinds, grants, source, record, .. } => {
                let exit = sandbox::Requested::new(kinds, grants, *source, record);
                self.exits.insert(*call_id, exit);
                Step::default()
            }
            Event::ExitReviewStarted { call_id, kinds, .. } => {
                // NOTE: the call waits for the verdict, so the status row shows what
                // the reviewer reviews in place of the call's line.
                if let Some(call) = self.call.as_mut().filter(|call| call.call_id == *call_id) {
                    call.awaiting = true;
                }
                let exit = self.exits.get(call_id);
                let words = sandbox::reviewing(exit, kinds, self.home.as_deref());
                self.state(State::Reviewing(words));
                Step::default()
            }
            Event::ExitJudged {
                call_id,
                judge: JudgeKind::Classifier,
                verdict,
                risk,
                rationale,
                skipped,
                ..
            } => self.judged(*call_id, *verdict, *risk, rationale.as_deref(), skipped, size),
            Event::ExitJudged {
                call_id,
                judge: JudgeKind::Always,
                verdict: Verdict::Allow,
                category,
                rationale,
                ..
            } => {
                let line = format::always::covered(category.as_deref(), rationale.as_deref());
                self.covered(*call_id, &line, size)
            }
            // NOTE: full mode lets an exit through with no review and no question; the
            // end of the turn lists what it let through, once.
            Event::ExitJudged {
                call_id, judge: JudgeKind::Full, verdict: Verdict::Allow, ..
            } => {
                if let Some(exit) = self.exits.get(call_id) {
                    for part in sandbox::exit_parts(exit, self.home.as_deref()) {
                        if !self.ending.full_exits.contains(&part) {
                            self.ending.full_exits.push(part);
                        }
                    }
                }
                Step::default()
            }
            Event::AlwaysRuleSaved { call_id, outcome, .. } => {
                if self.saving == Some(*call_id) {
                    self.saving = None;
                }
                let ours = self.call.as_ref().is_some_and(|call| call.call_id == *call_id);
                let line = format::always::saved(outcome);
                self.verdict_line(ours.then_some(*call_id), &line, size)
            }
            Event::ApprovalRequested {
                call_id,
                summary,
                diff_preview,
                interactive,
                exit,
                expires_in_secs,
                ..
            } => {
                if *interactive {
                    self.interactive.insert(*call_id);
                    self.flagged.insert(*call_id);
                }
                if let Some(call) = self.call.as_mut().filter(|call| call.call_id == *call_id) {
                    call.awaiting = true;
                }
                if !can_ask {
                    self.state(State::Answer);
                }
                let request = Request {
                    blocking: false,
                    summary,
                    diff: diff_preview.as_deref(),
                    exit: exit.as_ref(),
                    expires_in_secs: *expires_in_secs,
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
            Event::ApprovalExpired { call_id, away, .. } => {
                if let Some(call) = self.call.as_mut().filter(|call| call.call_id == *call_id) {
                    call.awaiting = false;
                }
                self.answer_came(*call_id);
                self.expired(*call_id, *away, size)
            }
            Event::HomeFileChanged { .. } => {
                if let Some(change) = HomeChange::of(event) {
                    format::changes::add_home_change(&mut self.ending.home_changes, change);
                }
                Step::default()
            }
            Event::ClassifierPaused { reason, denials, .. } => {
                self.whole_note(&sandbox::paused(*reason, *denials), size)
            }
            Event::JobStarted { call_id, job, .. } => {
                let line = jobs::started(job);
                // NOTE: efrd records the start before the call ends, so the line goes in
                // the call's block, under its result.
                if self.call.as_ref().is_some_and(|call| call.call_id == *call_id) {
                    self.jobs.lines.insert(*call_id, line);
                    return Step::default();
                }
                self.whole_note(&line, size)
            }
            Event::JobNotice { turn_id, woke: true, .. } => self.wake_mark(*turn_id, size),
            Event::JobNotice { job_id, .. } => self.whole_note(&jobs::notice_rides(job_id), size),
            Event::CacheDeleteApplied { paths, freed_bytes, .. } => {
                let line = sandbox::cache_freed(paths, *freed_bytes, self.home.as_deref());
                self.whole_note(&line, size)
            }
            // A steer of this view shows as unread until a model call reads it.
            Event::TurnSteered { .. }
                if self.event_seq.is_some_and(|seq| {
                    self.input.as_ref().is_some_and(|input| input.is_steer(seq))
                }) =>
            {
                Step::default()
            }
            Event::TurnSteered { text, .. } => {
                self.note(&format!("steered: {}", format::one_line(text)), size)
            }
            Event::SteeringDelivered { steers, .. } => {
                let texts = self.input.as_mut().map(|input| input.delivered(steers));
                let options = self.options_at(size);
                let mut committed = String::new();
                for text in texts.unwrap_or_default() {
                    committed.push_str(self.spacing.before(Block::Prompt));
                    committed.push_str(&user_message(&text, &options));
                }
                self.dirty = true;
                self.commit(committed)
            }
            Event::PromptWithdrawn { .. } => {
                self.end(TurnEnd::Withdrawn, &[WITHDRAWN.to_owned().into()], size)
            }
            Event::TurnInterruptRequested { .. } if self.interrupting => Step::default(),
            Event::TurnInterruptRequested { origin, .. } => {
                self.note(&format!("interrupt requested from {}", format::origin(*origin)), size)
            }
            Event::ConversationCompacted(compaction) => {
                self.context(gauge::after(compaction));
                let before = self.finish_message();
                let rows = |options: &RenderOptions| gauge::compacted_rows(compaction, options);
                self.rendered_after(before, rows, size)
            }
            Event::TurnCompleted { usage, changes, context, stop, undo, .. } => {
                // NOTE: why the model stopped shows also without the end's line: a turn
                // with no answer must not end in silence.
                let stopped = stop.as_ref().map(format::turn_stop).map(GaugedLine::from);
                // NOTE: the files that the turn changed stand right before its end, also
                // without the end's line: they say what the turn left behind. A turn
                // that undo can take back says so in the lines before the end.
                let undoable = self.ended_with(changes.as_ref(), undo.as_ref());
                let changed = changes
                    .as_ref()
                    .filter(|_| !undoable)
                    .and_then(|changes| format::changes::turn_line(changes, &self.groups));
                let line = self
                    .summary()
                    .then(|| format::turn_done(self.took(), usage.as_ref(), context.as_ref()));
                let lines: Vec<GaugedLine> =
                    stopped.into_iter().chain(changed.map(GaugedLine::from)).chain(line).collect();
                self.end(TurnEnd::Completed, &lines, size)
            }
            Event::TurnFailed { error, changes, undo, .. } => {
                self.ended_with(changes.as_ref(), undo.as_ref());
                self.end(TurnEnd::Failed(error.clone()), &[], size)
            }
            Event::TurnInterrupted { context, changes, undo, .. } => {
                self.ended_with(changes.as_ref(), undo.as_ref());
                let line = if self.summary() {
                    format::turn_interrupted(self.took(), context.as_ref())
                } else {
                    GaugedLine::from("interrupted".to_owned())
                };
                self.end(TurnEnd::Interrupted, &[line], size)
            }
            Event::TurnCancelled { .. } => self.end(TurnEnd::Cancelled, &[], size),
            _ => Step::default(),
        }
    }

    /// An event of another turn: only the approvals of the turn this one waits behind
    /// and its running call show, and only until this one starts.
    fn other_turn(&mut self, event: &Event, size: Size, can_ask: bool) -> Step {
        if let Some(step) = self.own_prompt(event) {
            return step;
        }
        if !self.queued {
            return Step::default();
        }
        match event {
            // The turn ahead is one that a job notice started, not the user.
            Event::JobNotice { turn_id, woke: true, .. } => self.wake_mark(*turn_id, size),
            Event::ToolCallStarted { call_id, tool, input, launch, .. } => {
                self.tools.insert(*call_id, tool.clone());
                if let Some(command) = format::command_of(input) {
                    self.commands.insert(*call_id, command.to_owned());
                }
                // NOTE: the waits of the turn ahead show here too, so a call of it in
                // the sandbox must never get an answer that may be a secret either.
                if let Some(Launch::Contained { grants }) = launch {
                    let open = grants.contains(&Grant::OpenNetwork);
                    self.contained.insert(*call_id, (grants.contains(&Grant::NetView), open));
                }
                Step::default()
            }
            Event::ExitRequested { call_id, kinds, grants, source, record, .. } => {
                let exit = sandbox::Requested::new(kinds, grants, *source, record);
                self.exits.insert(*call_id, exit);
                Step::default()
            }
            Event::ApprovalRequested {
                call_id,
                summary,
                diff_preview,
                interactive,
                exit,
                expires_in_secs,
                ..
            } => {
                self.blocking.insert(*call_id);
                if *interactive {
                    self.interactive.insert(*call_id);
                    self.flagged.insert(*call_id);
                }
                let request = Request {
                    blocking: true,
                    summary,
                    diff: diff_preview.as_deref(),
                    exit: exit.as_ref(),
                    expires_in_secs: *expires_in_secs,
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
            Event::ApprovalExpired { call_id, away, .. } if self.blocking.contains(call_id) => {
                self.expired(*call_id, *away, size)
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

    /// An event of a prompt that this view queued and does not follow yet; `None` for
    /// any other event. Steers that an interrupt sent again become that prompt. A prompt
    /// that ended before it started leaves the list: the daemon cancelled it at a
    /// restart, which puts its text back into the input row, or someone took it back.
    fn own_prompt(&mut self, event: &Event) -> Option<Step> {
        let input = self.input.as_mut()?;
        match event {
            Event::PromptQueued { turn_id, steers, .. } if !steers.is_empty() => {
                if !input.resent(*turn_id, steers) {
                    return None;
                }
            }
            Event::TurnCancelled { turn_id } => {
                let queued = input.remove(*turn_id)?;
                input.line.append(&queued.text);
            }
            Event::PromptWithdrawn { turn_id, .. }
            | Event::TurnCompleted { turn_id, .. }
            | Event::TurnFailed { turn_id, .. }
            | Event::TurnInterrupted { turn_id, .. } => {
                input.remove(*turn_id)?;
            }
            _ => return None,
        }
        self.dirty = true;
        Some(Step::default())
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
        let good = format::allows(decision);
        if decision == ApprovalDecision::AllowAlways {
            self.saving = Some(call_id);
        }
        Step { settled, ..self.answer_line(About::Approval(call_id), good, &line, size) }
    }

    /// The reviewer judged the exit of call `call_id`. An allow or a refusal is one muted
    /// line, which the call's rows follow at once, as they follow an answer; an unsure
    /// verdict shows on the card of the question that comes next. A verdict without a
    /// risk is one that no reviewer model gave: every step of the chain failed, which
    /// the card says with the steps in `skipped`.
    fn judged(
        &mut self,
        call_id: CallId,
        verdict: Verdict,
        risk: Option<Risk>,
        rationale: Option<&str>,
        skipped: &[String],
        size: Size,
    ) -> Step {
        let ours = self.call.as_ref().is_some_and(|call| call.call_id == call_id);
        let exit = self.exits.get(&call_id);
        let Some(line) = sandbox::reviewed(verdict, risk, rationale, exit, self.home.as_deref())
        else {
            let row = if risk.is_none() {
                sandbox::no_answer_row(skipped)
            } else {
                sandbox::reviewer_row(verdict, rationale.unwrap_or_default())
            };
            self.reviewed.insert(call_id, row);
            return Step::default();
        };
        match verdict {
            Verdict::Allow if ours => {
                if let Some(call) = self.call.as_mut() {
                    call.approved(self.event_at);
                }
                let tool = self
                    .tools
                    .get(&call_id)
                    .map_or_else(String::new, |tool| format::one_line(tool));
                self.state(State::Tool(tool));
            }
            Verdict::Deny => {
                // NOTE: the rows of a call that did not run still show what it was.
                if let Some(call) = self.call.as_mut().filter(|_| ours) {
                    call.awaiting = false;
                }
                self.refused.insert(call_id);
                self.state(State::Model);
            }
            _ => self.state(State::Model),
        }
        self.verdict_line(ours.then_some(call_id), &line, size)
    }

    /// An always rule covered every exit of call `call_id`, which runs with no question
    /// and no review (phase E): one muted `line` before its rows, as a reviewer's allow.
    fn covered(&mut self, call_id: CallId, line: &str, size: Size) -> Step {
        let ours = self.call.as_ref().is_some_and(|call| call.call_id == call_id);
        if ours {
            if let Some(call) = self.call.as_mut() {
                call.approved(self.event_at);
            }
            let tool =
                self.tools.get(&call_id).map_or_else(String::new, |tool| format::one_line(tool));
            self.state(State::Tool(tool));
        }
        self.verdict_line(ours.then_some(call_id), line, size)
    }

    /// The muted line of a verdict: right before the rows of call `call`, which follow
    /// it at once, or a note when it is about no running call, such as the deletes of
    /// a tool cache after the call.
    fn verdict_line(&mut self, call: Option<CallId>, line: &str, size: Size) -> Step {
        let options = self.options_at(size);
        let text = rows(line, Tone::Dim, &options);
        let block = if call.is_some() { Block::Settled } else { Block::Note };
        if !self.terminal() {
            let mut err = self.err_spacing.before(block).to_owned();
            err.push_str(&text);
            let mut err = self.raw_err(err);
            if call.is_some() {
                err.push_str(&self.due_header(size));
            }
            return Step { err, ..Step::default() };
        }
        let mut committed = self.spacing.before(block).to_owned();
        committed.push_str(&text);
        self.stage(&committed);
        Step::default()
    }

    /// The approval of call `call_id` expired; `away` when nobody answered while the
    /// user was away.
    fn expired(&mut self, call_id: CallId, away: bool, size: Size) -> Step {
        self.refused.insert(call_id);
        let settled = self.settle(call_id);
        let line = if away { sandbox::AWAY_EXPIRED } else { "the approval expired" };
        let step = self.answer_line(About::Approval(call_id), false, line, size);
        Step { settled, ..step }
    }

    /// The question about `about` is settled: on a terminal its card gives its place to
    /// one line, such as `✓ allowed` (`good`) or `✗ denied`. A call of this turn follows
    /// the line at once; anything else comes after a blank line.
    fn answer_line(&mut self, about: About, good: bool, line: &str, size: Size) -> Step {
        if self.question.as_ref().is_some_and(|shown| shown.about == about) {
            self.question = None;
            self.dirty = true;
        }
        let options = self.options_at(size);
        let (mark, tone) = if good { (YES, Tone::Success) } else { (NO, Tone::Failure) };
        let text = rows(&format!("{mark}{}", format::one_line(line)), tone, &options);
        let ours = matches!(about, About::Approval(call) if !self.blocking.contains(&call));
        let block = if ours { Block::Settled } else { Block::Answer };
        if !self.terminal() {
            // NOTE: the call's first rows wait for its answer and follow it at once.
            let mut err = self.err_spacing.before(block).to_owned();
            err.push_str(&text);
            let mut err = self.raw_err(err);
            // NOTE: after an `a` answer, the line of what it saved comes first.
            if let About::Approval(call) = about
                && self.call.as_ref().is_some_and(|shown| shown.call_id == call)
                && self.saving != Some(call)
            {
                err.push_str(&self.due_header(size));
            }
            return Step { err, ..Step::default() };
        }
        let mut committed = self.spacing.before(block).to_owned();
        committed.push_str(&text);
        self.stage(&committed);
        Step::default()
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
            let text = format!("{}{text}", self.err_spacing.before(Block::Note));
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
        let card = sandbox::surface_card(changes, self.home.as_deref());
        let mut step = Step { out: before, ..Step::default() };
        let footer = if can_ask {
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
            Footer::Keys(card::KEEP_KEYS)
        } else {
            Footer::Waiting
        };
        self.show_question(step, card, About::Surface(question_id), footer, size)
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
        self.answer_line(About::Surface(question_id), keep, line, size)
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
        Step { settled, ..self.answer_line(About::Surface(question_id), keep, &line, size) }
    }

    /// The mark of turn `turn`, which a job notice started: once, however many notices
    /// it takes.
    fn wake_mark(&mut self, turn: TurnId, size: Size) -> Step {
        if !self.jobs.woken.insert(turn) {
            return Step::default();
        }
        self.whole_note(jobs::WAKE_MARK, size)
    }

    /// The question of the stop key about `job`, the newest running job of the
    /// conversation: `stop job j3 (npm run dev)`, answered with one key. The view holds
    /// the job until [`stop_answered`](Self::stop_answered).
    pub(crate) fn stop_question(&mut self, job: &JobInfo, size: Size) -> Step {
        let before = self.finish_message();
        let card = Card { title: jobs::stop_title(job), rows: Vec::new() };
        let step = Step { out: before, ask: Some(Ask::StopJob), ..Step::default() };
        self.jobs.stopping = Some(job.id.clone());
        // One question at a time: the question's key reader replaces any other.
        self.retained = None;
        if let Some(running) = &mut self.running {
            running.asking = None;
            running.guarding = false;
            running.typed.clear();
            running.hinted = false;
        }
        self.show_question(step, card, About::StopJob, Footer::Keys(jobs::STOP_KEYS), size)
    }

    /// The user answered the question of the stop key: `stop` with `y`. The job to stop
    /// comes back with a `y`; its end shows when its `job_ended` comes.
    pub(crate) fn stop_answered(&mut self, stop: bool, size: Size) -> (Step, Option<JobId>) {
        let Some(job) = self.jobs.stopping.take() else {
            return (Step::default(), None);
        };
        let line = if stop { format!("stopping job {job}") } else { format!("job {job} runs on") };
        let step = self.answer_line(About::StopJob, stop, &line, size);
        (step, stop.then_some(job))
    }

    /// The daemon did not stop `job`: a note says why.
    pub(crate) fn stop_failed(&mut self, job: &JobId, message: &str, size: Size) -> Step {
        let line = format!("job {job} was not stopped: {}", format::one_line(message));
        self.whole_note(&line, size)
    }

    /// Shows `card`, the question about `about`, with `footer`, after `step`'s output:
    /// in the live zone on a terminal until it is answered, on stderr otherwise.
    fn show_question(
        &mut self,
        mut step: Step,
        card: Card,
        about: About,
        footer: Footer,
        size: Size,
    ) -> Step {
        // NOTE: a question of the turn takes the keys from the question of the stop key,
        // which then waits for no key.
        if about != About::StopJob {
            self.jobs.stopping = None;
        }
        if self.terminal() {
            let before = std::mem::take(&mut step.out);
            self.stage(&before);
            // One card at a time: one that still waits goes to the scrollback whole.
            self.commit_question();
            self.question = Some(Shown { card, about, footer, committed: false });
            self.dirty = true;
        } else {
            let mut text = self.err_spacing.before(Block::Question).to_owned();
            text.push_str(&card.render(Some(&footer), &self.options_at(size)));
            step.err = self.raw_err(text);
        }
        step
    }

    /// A dim note line, such as `queued behind the running turn`.
    pub(crate) fn note(&mut self, text: &str, size: Size) -> Step {
        self.note_after(String::new(), text, size)
    }

    /// A dim note that a terminal shows whole: a row wider than the screen goes on in
    /// the next one, where a note is cut. For what the user must read to the end, such
    /// as why the reviewer paused or where a backup is.
    fn whole_note(&mut self, text: &str, size: Size) -> Step {
        self.rendered_after(String::new(), |options| rows(text, Tone::Dim, options), size)
    }

    /// A note written after `before`, the end of a message.
    fn note_after(&mut self, before: String, text: &str, size: Size) -> Step {
        self.line_after(before, &GaugedLine::from(text.to_owned()), size)
    }

    /// A muted line, with its gauge in the colour of its level, written after `before`.
    fn line_after(&mut self, before: String, line: &GaugedLine, size: Size) -> Step {
        self.rendered_after(before, |options| line.render(options), size)
    }

    /// A note that `render` makes at the screen's width, written after `before`: on a
    /// terminal to the scrollback, otherwise to stderr.
    fn rendered_after(
        &mut self,
        before: String,
        render: impl FnOnce(&RenderOptions) -> String,
        size: Size,
    ) -> Step {
        let options = self.options_at(size);
        if !self.terminal() {
            let text = format!("{}{}", self.err_spacing.before(Block::Note), render(&options));
            let err = self.raw_err(text);
            return Step { out: before, err, ..Step::default() };
        }
        let mut committed = before;
        committed.push_str(self.spacing.before(Block::Note));
        committed.push_str(&render(&options));
        self.stage(&committed);
        Step::default()
    }

    /// The output of call `call_id` grew and now ends in `tail`. With the input row,
    /// an open last line that looks like a password prompt holds the keys until the
    /// daemon reports the wait ([`Running::held`]), and a new last line lets them go.
    /// A line that ends with a line break is not a prompt: the daemon too tests only
    /// the line that the cursor is on.
    fn output(&mut self, call_id: CallId, tail: &str) -> Step {
        let free = self.input.is_some() && !self.question_pending();
        let retained = self.retained == Some(call_id);
        let (running, settled) = self.running(call_id);
        running.tail = last_line(tail);
        running.lines = call::last_lines(tail);
        running.stirred();
        let quiet = running.asking.is_none() && !running.guarding && !retained;
        let hold = free && quiet && looks_secret(open_line(tail));
        let mut step = Step { settled, ..self.commit(String::new()) };
        let Some(running) = self.running.as_mut() else {
            return step;
        };
        if hold && !running.held {
            running.held = true;
            step.ask = Some(Ask::Retain(call_id));
        } else if !hold && running.held {
            running.held = false;
            step.settled = true;
        }
        step
    }

    /// The call that may offer `Ctrl+\` once it has been silent long enough, with its
    /// activity count, which changes at every sign of life: a call of this turn that
    /// takes a manual input, runs and reports no wait, while no key is read for it and
    /// no approval waits, and whose line that offers `Ctrl+\` is not shown yet.
    pub(crate) fn silence(&self) -> Option<(CallId, u64)> {
        let running = self.running.as_ref()?;
        let quiet = running.quiet()
            && !running.hinted
            && !self.question_pending()
            && !self.left_reads_keys();
        quiet.then_some((running.call_id, running.activity))
    }

    /// The call whose line that offers `Ctrl+\` is shown now; the follow loop waits for
    /// the key exactly while there is one.
    pub(crate) fn manual_offer(&self) -> Option<CallId> {
        let running = self.running.as_ref()?;
        let offered = running.quiet()
            && running.hinted
            && !self.question_pending()
            && !self.left_reads_keys();
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
        if self.terminal() {
            return self.commit(String::new());
        }
        let header = self.due_header(size);
        let step = self.note(SILENCE_HINT, size);
        Step { err: format!("{header}{}", step.err), ..step }
    }

    /// The user pressed `Ctrl+\` while the line for call `call_id` was shown: asks for a
    /// line that is not shown as it is typed and goes as a manual answer.
    pub(crate) fn manual(&mut self, call_id: CallId, size: Size) -> Step {
        // NOTE: a manual line may hold a password, which never goes into the sandbox.
        if self.manual_offer() != Some(call_id) || self.contained.contains_key(&call_id) {
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
        // NOTE: a prompt in the sandbox can be a fake, so efr asks for no answer that may
        // be a secret there: not a hidden one, and not one whose prompt looks like a
        // password prompt. efr-shell stops such a command at once.
        if kind != AnswerKind::Visible && self.contained.contains_key(&call_id) {
            return Step { settled, ..self.note(HIDDEN_INPUT_SANDBOXED, size) };
        }
        if !can_ask {
            let note = if kind.hidden() { HIDDEN_INPUT_ELSEWHERE } else { VISIBLE_INPUT_ELSEWHERE };
            return Step { settled, ..self.note(note, size) };
        }
        // One question at a time, so an approval and an input never overlap. Tool calls
        // run one after another, so an approval does not come while a command runs.
        // The line of a command left running in the hidden shell keeps the keys too.
        if approval_pending || self.left_reads_keys() {
            return Step { settled, ..Step::default() };
        }
        self.ask_for(call_id, kind, size)
    }

    /// An event of a command that a shell call left running in the hidden shell;
    /// `None` for any other event.
    fn left_event(&mut self, event: &Event, size: Size, can_ask: bool) -> Option<Step> {
        let step = match event {
            Event::ShellCommandDetached {
                call_id,
                command,
                contained,
                input,
                looks_secret,
                prompt,
                approved_by,
                ..
            } => {
                let wait = Wait { input: *input, looks_secret: *looks_secret, prompt };
                self.left_detached(
                    *call_id,
                    command,
                    *contained,
                    *approved_by,
                    &wait,
                    can_ask,
                    size,
                )
            }
            Event::ShellCommandInputChanged { call_id, input, contained, looks_secret, prompt } => {
                let wait = Wait { input: *input, looks_secret: *looks_secret, prompt };
                self.left_input(*call_id, *contained, &wait, can_ask, size)
            }
            Event::ShellCommandEnded { call_id, exit_code, how } => {
                self.left_ended(*call_id, *exit_code, *how, size)
            }
            _ => return None,
        };
        Some(step)
    }

    /// The command of call `call_id` still runs in the hidden shell after its call
    /// answered the model. A call of this view whose approval said it may wait for input
    /// and that runs outside the sandbox holds the keys until its first wait is known,
    /// so a password typed for it never reaches the input row.
    #[expect(clippy::too_many_arguments, reason = "the fields of one event")]
    fn left_detached(
        &mut self,
        call_id: CallId,
        command: &str,
        contained: bool,
        approved_by: Option<ApprovedBy>,
        wait: &Wait<'_>,
        can_ask: bool,
        size: Size,
    ) -> Step {
        let mut settled = self.left_gone();
        // NOTE: the detach and the call's completion come in either order. The line of
        // the running call goes, and the command's own line takes its place.
        if self.running.as_ref().is_some_and(|running| running.call_id == call_id) {
            settled |= self.running.take().is_some_and(|running| running.reads_keys());
        }
        if self.retained == Some(call_id) {
            self.retained = None;
            settled = true;
        }
        let mut left = Left::new(call_id, command, contained, approved_by);
        left.waits(wait.input, wait.looks_secret, wait.prompt);
        left.can_ask = can_ask;
        left.holding =
            !contained && wait.input == InputWait::None && self.flagged.contains(&call_id);
        self.left = Some(Box::new(left));
        let step = self.left_wait(size);
        Step { settled: settled || step.settled, ..step }
    }

    /// The command left running by call `call_id` began or stopped waiting for input.
    fn left_input(
        &mut self,
        call_id: CallId,
        contained: bool,
        wait: &Wait<'_>,
        can_ask: bool,
        size: Size,
    ) -> Step {
        let mut settled = false;
        if !self.left.as_ref().is_some_and(|left| left.call_id == call_id) {
            // The view did not see it start: it knows the call, not the command.
            settled = self.left_gone();
            self.left = Some(Box::new(Left::new(call_id, "", contained, None)));
        }
        let Some(left) = self.left.as_deref_mut() else {
            return Step::default();
        };
        if left.asking.is_some_and(AnswerKind::guards) {
            left.guarding = true;
        }
        settled |= left.reads_keys();
        left.asking = None;
        left.held = false;
        left.holding = false;
        left.typed.clear();
        left.contained = contained;
        left.can_ask = can_ask;
        left.waits(wait.input, wait.looks_secret, wait.prompt);
        let step = self.left_wait(size);
        Step { settled: settled || step.settled, ..step }
    }

    /// The command left running by call `call_id` ended: a note says how, and its line
    /// goes.
    fn left_ended(
        &mut self,
        call_id: CallId,
        exit_code: Option<i32>,
        how: ShellCommandEnd,
        size: Size,
    ) -> Step {
        let Some(left) = self.left.take_if(|left| left.call_id == call_id) else {
            return Step::default();
        };
        let note = format::hidden_shell::ended(&left.command(), exit_code, how);
        let step = self.note(&note, size);
        Step { settled: left.reads_keys(), ..step }
    }

    /// The command left running drops out of the view; true when keys were read for it.
    fn left_gone(&mut self) -> bool {
        self.left.take().is_some_and(|left| left.reads_keys())
    }

    /// What the wait of the command left running asks for now: a note in the sandbox
    /// or without keys, else its answer line when nothing else asks.
    fn left_wait(&mut self, size: Size) -> Step {
        let Some(left) = self.left.as_deref() else {
            return Step::default();
        };
        if left.wait != InputWait::None {
            if left.contained && left.secret_wait() {
                return self.note(HIDDEN_INPUT_SANDBOXED, size);
            }
            if !left.can_ask {
                let note = left.elsewhere_note();
                return self.note(&note, size);
            }
        }
        match self.left_offer() {
            Some(step) => step,
            None => self.commit(String::new()),
        }
    }

    /// True while keys are read for the command left running in the hidden shell.
    fn left_reads_keys(&self) -> bool {
        self.left.as_ref().is_some_and(|left| left.reads_keys())
    }

    /// The command left running in the hidden shell whose line reads the keys now, to
    /// answer or to hold: Ctrl+C stops that command (`shell.interrupt`), not the turn.
    pub(crate) fn left_keys(&self) -> Option<CallId> {
        self.left.as_ref().filter(|left| left.reads_keys()).map(|left| left.call_id)
    }

    /// True when `ask` is the line of the command left running in the hidden shell.
    pub(crate) fn left_asks(&self, ask: &Ask) -> bool {
        let call = match ask {
            Ask::Input { call_id, .. } | Ask::Discard(call_id) => *call_id,
            _ => return false,
        };
        self.left.as_ref().is_some_and(|left| left.call_id == call && left.reads_keys())
    }

    /// Another question took the keys from the line of the command left running: the
    /// line closes, and [`left_offer`](Self::left_offer) opens it again afterwards.
    pub(crate) fn left_yield(&mut self) {
        if let Some(left) = self.left.as_deref_mut().filter(|left| left.reads_keys()) {
            left.asking = None;
            left.held = false;
            left.typed.clear();
            self.dirty = true;
        }
    }

    /// The answer line of the command left running, or the hold of the keys for it,
    /// when it needs the keys, they can be read, and nothing else asks; `None`
    /// otherwise. The line takes the place of the input row at once.
    pub(crate) fn left_offer(&mut self) -> Option<Step> {
        if self.question_pending() || self.running.as_ref().is_some_and(Running::reads_keys) {
            return None;
        }
        let left = self.left.as_deref_mut()?;
        if left.reads_keys() || !left.can_ask {
            return None;
        }
        let ask = match left.kind() {
            Some(kind) => {
                left.asking = Some(kind);
                left.holding = false;
                Ask::Input { call_id: left.call_id, kind }
            }
            // NOTE: before its first wait is known, or after it asked for a password,
            // what is typed may be a password: it never goes to the row.
            None if left.wait == InputWait::None && (left.holding || left.guarding) => {
                left.held = true;
                Ask::Discard(left.call_id)
            }
            None => return None,
        };
        Some(self.left_shown(ask))
    }

    /// `ask` for the line of the command left running, with what shows it: the live zone
    /// on a terminal, otherwise its rows on stderr.
    fn left_shown(&mut self, ask: Ask) -> Step {
        let Some(left) = self.left.as_deref() else {
            return Step::default();
        };
        let shown = left.asking.is_some_and(AnswerKind::shown);
        let raw = left.raw();
        let mut step = if self.terminal() {
            self.commit(String::new())
        } else {
            let spacing = self.err_spacing.before(Block::Note);
            let mut err = self.raw_err(format!("{spacing}{raw}"));
            if shown {
                err.push_str(ECHO_PREFIX);
                self.echo_line = true;
            }
            Step { err, ..Step::default() }
        };
        step.ask = Some(ask);
        step
    }

    /// The line under the end of the last followed turn while a command still runs in
    /// the hidden shell. When it waits for input that can be answered here, `efr` stays
    /// open with its answer line ([`lingers`](Self::lingers)); otherwise the line says
    /// where to answer or stop it.
    fn left_at_end(&mut self, size: Size) -> Step {
        let Some(left) = self.left.as_deref_mut() else {
            return Step::default();
        };
        let kind = left.kind().filter(|_| left.can_ask);
        let line = left.end_line(kind.is_some());
        left.held = false;
        left.holding = false;
        left.guarding = false;
        let ask = match kind {
            Some(kind) => {
                left.lingering = true;
                left.asking = Some(kind);
                Some(Ask::Input { call_id: left.call_id, kind })
            }
            None => {
                left.asking = None;
                None
            }
        };
        let mut step = self.whole_note(&line, size);
        if let Some(ask) = ask {
            let shown = self.left_shown(ask);
            step.err.push_str(&shown.err);
            step.ask = shown.ask;
        }
        step
    }

    /// True while the followed turns ended but `efr` stays open for the answer line of a
    /// command that waits in the hidden shell.
    pub(crate) fn lingers(&self) -> bool {
        self.left.as_ref().is_some_and(|left| left.lingering && left.asking.is_some())
    }

    /// Esc at the answer line of `call_id` after the turn: `efr` leaves the command
    /// waiting and ends. `None` when that line does not stay open after the turn.
    pub(crate) fn left_leave(&mut self, call_id: CallId, size: Size) -> Option<Step> {
        let left = self.left.as_deref_mut().filter(|left| left.call_id == call_id)?;
        if !left.lingering {
            return None;
        }
        left.lingering = false;
        left.asking = None;
        left.typed.clear();
        let step = self.note(left::LEFT_WAITING, size);
        Some(Step { settled: true, ..step })
    }

    /// The daemon did not stop the command left running, for the reason `message`.
    pub(crate) fn left_not_stopped(&mut self, message: &str, size: Size) -> Step {
        let line = format!("the command was not stopped: {}", format::one_line(message));
        self.note(&line, size)
    }

    /// Asks for an answer line of `kind` for the running call `call_id`, below its last
    /// output line.
    fn ask_for(&mut self, call_id: CallId, kind: AnswerKind, size: Size) -> Step {
        let Some(running) = self.running.as_mut().filter(|running| running.call_id == call_id)
        else {
            return Step::default();
        };
        running.asking = Some(kind);
        running.held = false;
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
        // NOTE: the line of a command left running in the hidden shell and the line of
        // the running call never ask at the same time.
        let (asking, typed) = match (self.left.as_deref_mut(), self.running.as_mut()) {
            (Some(left), _) if left.asking.is_some() => (left.asking, &mut left.typed),
            (_, Some(running)) => (running.asking, &mut running.typed),
            _ => return Step::default(),
        };
        if !asking.is_some_and(AnswerKind::shown) {
            return Step::default();
        }
        if terminal {
            text.clone_into(typed);
            return self.commit(String::new());
        }
        // A note since the last key ended the echo line; a new one starts empty.
        let (mut err, shown) =
            if echo_line { (String::new(), typed.as_str()) } else { (ECHO_PREFIX.to_owned(), "") };
        err.push_str(&echo_edit(shown, text));
        text.clone_into(typed);
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

    /// The answer of `kind` reached the command. A password, hidden or behind a relay,
    /// gets no note: the line under its prompt said how it goes, and the command's own
    /// output shows what came of it. Any other answer gets one, because its echo goes.
    pub(crate) fn answer_sent(&mut self, kind: AnswerKind, size: Size) -> Step {
        let note = (!kind.guards()).then_some(ANSWER_SENT);
        self.answer_note(note, size)
    }

    /// The daemon refused the answer because the command no longer waits for it.
    pub(crate) fn answer_refused(&mut self, size: Size) -> Step {
        self.answer_note(Some(ANSWER_REFUSED), size)
    }

    /// The daemon refused the answer for another reason, which `message` gives.
    pub(crate) fn answer_failed(&mut self, message: &str, size: Size) -> Step {
        let line = format!("the answer was not sent: {}", format::one_line(message));
        self.answer_note(Some(&line), size)
    }

    /// A note about an answer, if any; the echo of what was typed goes with it.
    /// A manual answer asks once: after it, the keys stop and the call's silence starts
    /// again.
    fn answer_note(&mut self, text: Option<&str>, size: Size) -> Step {
        if let Some(left) = &mut self.left {
            left.typed.clear();
        }
        let mut freed = None;
        if let Some(running) = &mut self.running {
            running.typed.clear();
            if running.asking.is_some_and(AnswerKind::manual) {
                running.asking = None;
                running.stirred();
                freed = Some(running.call_id);
            }
        }
        let step = match text {
            Some(text) => self.note(text, size),
            None => self.commit(String::new()),
        };
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
        let good = format::allows(decision);
        if decision == ApprovalDecision::AllowAlways {
            self.saving = Some(call_id);
        }
        let step =
            self.answer_line(About::Approval(call_id), good, format::decision(decision), size);
        if good && self.interactive.contains(&call_id) {
            self.retained = Some(call_id);
            return Step { ask: Some(Ask::Retain(call_id)), ..step };
        }
        step
    }

    /// True when the approval `call_id` waits for a key here and its question offers a
    /// rule for an `a` answer (phase E).
    pub(crate) fn offers_always(&self, call_id: CallId) -> bool {
        self.asking == Some(call_id) && self.offered.is_some()
    }

    /// The user pressed `a` at the approval `call_id`: the last row of its card asks to
    /// confirm the rule, `save rule "write under ~/notes"? y/n`. Only `y` then saves it.
    pub(crate) fn confirm_always(&mut self, call_id: CallId, size: Size) -> Step {
        match self.offered.clone().filter(|_| self.asking == Some(call_id)) {
            Some(rule) => self.set_footer(call_id, Footer::Confirm(rule), size),
            None => Step::default(),
        }
    }

    /// A key other than `y` at the confirm of an `a` answer: the question of the approval
    /// `call_id` asks again with its keys, and nothing is answered.
    pub(crate) fn always_cancelled(&mut self, call_id: CallId, size: Size) -> Step {
        match self.offered.clone().filter(|_| self.asking == Some(call_id)) {
            Some(rule) => self.set_footer(call_id, Footer::Always(rule), size),
            None => Step::default(),
        }
    }

    /// The daemon did not take the `a` answer to the approval `call_id`, which stays
    /// open: a note says why, and the question asks again without the `a`.
    pub(crate) fn always_refused(&mut self, call_id: CallId, message: &str, size: Size) -> Step {
        self.offered = None;
        let line = format!("the answer was not taken: {}", format::one_line(message));
        let mut step = self.note(&line, size);
        let keys = self.set_footer(call_id, Footer::Keys(card::ALLOW_KEYS), size);
        step.err.push_str(&keys.err);
        step
    }

    /// The last row of the card of the approval `call_id` becomes `footer`: in the live
    /// zone on a terminal, else the row again on stderr.
    fn set_footer(&mut self, call_id: CallId, footer: Footer, size: Size) -> Step {
        if self.terminal() {
            let about = About::Approval(call_id);
            if let Some(shown) = self.question.as_mut().filter(|shown| shown.about == about) {
                shown.footer = footer;
                self.dirty = true;
            }
            return Step::default();
        }
        let text = card::footer_row(&footer, &self.options_at(size));
        Step { err: self.raw_err(text), ..Step::default() }
    }

    /// Ends the view early: commits what the current message has so far and clears the
    /// live zone, before an error or an interrupt is reported.
    pub(crate) fn close(&mut self) -> Step {
        self.ended = true;
        self.end_progress.get_or_insert(progress::CLEAR);
        self.wrap_up()
    }

    /// Ends what the followed turn shows: commits what the current message has so far,
    /// a question that nobody answered and the sandbox's line, and drops the running
    /// call and the questions.
    fn wrap_up(&mut self) -> Step {
        // The rows of a call that still wait show what ran.
        let header = self.due_header(self.size);
        self.asking = None;
        self.saving = None;
        self.surface = None;
        self.running = None;
        self.call = None;
        self.retained = None;
        let committed = self.finish_message();
        // An echo line left open would carry what is written after the view.
        let mut err = header;
        err.push_str(&self.raw_err(String::new()));
        let mut step = Step { err, ..self.commit(committed) };
        // A question that nobody answered stays in the scrollback.
        self.commit_question();
        self.question = None;
        let net_view = std::mem::take(&mut self.traced_net_view);
        let network = std::mem::take(&mut self.traced_network);
        if std::mem::take(&mut self.traced) {
            // NOTE: the roots that the turn changed, from its end event, which comes
            // before this; without it the trace says where the calls could write (C2.3).
            let wrote: Vec<String> = self
                .ended_undo
                .as_ref()
                .map(|ended| {
                    ended
                        .1
                        .roots
                        .iter()
                        .map(|root| root.shown.clone())
                        .filter(|shown| !shown.starts_with("$SCRATCH"))
                        .collect()
                })
                .unwrap_or_default();
            let trace = sandbox::contained_trace(self.in_project, net_view, network, &wrote);
            // Whole, not cut to the width as a note is: it says where the calls wrote.
            let noted = self.dim_block(&[trace], self.size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        step
    }

    /// An update of message `index`: `delta` at byte `offset` of its text. An update
    /// that starts past what this view holds, because it joined in the middle of the
    /// message, is skipped; the completed message fills the gap.
    /// True when the text of message `index` from `offset` on reaches past the text held.
    fn adds_text(&self, index: u32, offset: u64, delta: &str) -> bool {
        let held = self
            .message
            .as_ref()
            .filter(|message| message.index == index)
            .map_or(0, |message| message.received().len());
        usize::try_from(offset).map_or(true, |offset| offset.saturating_add(delta.len()) > held)
    }

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
        let tool = self.tools.get(&call_id).map(String::as_str);
        let command = self.commands.get(&call_id).map(String::as_str);
        let mut card = card::approval(request.summary, tool, command);
        // NOTE: an exit shows the whole line from its record, never a shortened summary.
        if let Some(exit) = request.exit {
            let record = self.exits.get(&call_id).map(|exit| &exit.record);
            let reviewed = self.reviewed.get(&call_id).map(String::as_str);
            // A question that full mode still asks keeps the line that says why first.
            let wake = card::strip_wake(request.summary);
            let guard = format::full_guard(wake.unwrap_or(request.summary)).map(|(guard, _)| guard);
            let skip = usize::from(guard.is_some());
            // NOTE: the exit's card says why on its own row, so the subject is only what
            // runs: the lines of the command, else the summary's first row.
            let commands: Vec<card::Row> = card
                .rows
                .iter()
                .filter(|row| matches!(row, card::Row::Command(_)))
                .cloned()
                .collect();
            let subject = if commands.is_empty() {
                card.rows.into_iter().skip(skip).take(1).collect()
            } else {
                commands
            };
            card = sandbox::exit_card(exit, record, self.home.as_deref(), subject, reviewed);
            if let Some(guard) = guard {
                card.rows.insert(0, card::guard_row(guard));
            }
            if wake.is_some() {
                card::mark_wake(&mut card);
            }
        }
        // NOTE: a patch shows the diff of each of its files after a row that names it,
        // so a delete or a move stands out before the user answers. An edit is a patch
        // of one file.
        match (request.diff, tool) {
            (Some(diff), Some(format::patch::TOOL | format::EDIT_TOOL)) => {
                card.rows.extend(card::patch_rows(diff));
            }
            (Some(diff), _) => card.rows.push(card::Row::Diff(diff.to_owned())),
            (None, _) => {}
        }
        if let Some(secs) = request.expires_in_secs {
            card.rows.push(card::expires_row(secs));
        }
        if self.away {
            card.rows.push(card::Row::text(sandbox::AWAY_ROW, Tone::Dim));
        }
        if request.blocking {
            card.title = format!("{BLOCKING}{}", card.title);
        }
        let mut step = Step { out: before, ..Step::default() };
        // Calls run one after another, so a call that kept keys is over by now.
        self.retained = None;
        let footer = if can_ask {
            self.asking = Some(call_id);
            step.ask = Some(Ask::Approval(call_id));
            // One question at a time: the approval's key reader replaces the input's.
            if let Some(running) = &mut self.running {
                running.asking = None;
                running.guarding = false;
                running.typed.clear();
                running.hinted = false;
            }
            // NOTE: only a daemon that knows the `a` answer fills `always` (phase E), so
            // an older one never gets it.
            self.offered = request
                .exit
                .and_then(|exit| exit.always.as_deref())
                .map(format::one_line)
                .filter(|rule| !rule.trim().is_empty());
            match &self.offered {
                Some(rule) => Footer::Always(rule.clone()),
                None => Footer::Keys(card::ALLOW_KEYS),
            }
        } else {
            Footer::Waiting
        };
        self.show_question(step, card, About::Approval(call_id), footer, size)
    }

    /// Clears the question when it was about `call_id`; true when it was.
    fn settle(&mut self, call_id: CallId) -> bool {
        let settled = self.asking == Some(call_id);
        if settled {
            self.asking = None;
        }
        settled
    }

    /// Keeps what the turn's end event says it changed and what `efr undo` can take
    /// back, for the lines before the end; true when undo can take the turn back now.
    fn ended_with(&mut self, changes: Option<&FileChanges>, undo: Option<&TurnUndoInfo>) -> bool {
        self.ended_undo = undo.map(|undo| Box::new((changes.cloned(), undo.clone())));
        undo.is_some_and(|undo| undo.undoable)
    }

    /// The whole lines before the end of the turn: one per file outside the roots that
    /// the turn changed and that has a line of its own, then, for a turn that `efr
    /// undo` can take back, its line and what no snapshot covers. A file that efr could
    /// not copy takes its reason from the names of what no snapshot covers, so the
    /// reason shows once.
    fn end_lines(&mut self) -> Vec<String> {
        let home = self.home.as_deref();
        let ended = self.ended_undo.take();
        let mut not_covered =
            ended.as_ref().map(|ended| ended.1.not_covered.clone()).unwrap_or_default();
        let mut lines: Vec<String> =
            sandbox::full_let_through(&std::mem::take(&mut self.ending.full_exits))
                .into_iter()
                .collect();
        // NOTE: a covered file that has a line of its own (`changed ~/.zshrc; efr undo
        // reverts it`) does not count again in the line of the turn, which then shows
        // only when something else changed.
        let own: u32 = self
            .ending
            .home_changes
            .iter()
            .filter(|change| change.backup.is_none() && change.covered && change.floor)
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        for change in std::mem::take(&mut self.ending.home_changes) {
            let real = change.real.as_deref();
            if let Some(backup) = change.backup.as_deref() {
                lines.push(sandbox::home_changed(&change.path, real, Some(backup), home));
                continue;
            }
            let why = if change.covered {
                None
            } else {
                take_reason(&mut not_covered, &sandbox::tilde(&change.path, home))
            };
            lines.extend(sandbox::home_undo_line(
                &change.path,
                real,
                change.floor,
                change.covered,
                why.as_deref(),
                home,
            ));
        }
        if let Some((changes, mut undo)) =
            ended.map(|ended| *ended).filter(|(_, undo)| undo.undoable)
        {
            undo.home_files = undo.home_files.saturating_sub(own);
            lines.extend(format::changes::undo_line(changes.as_ref(), &undo, &self.groups));
            lines.extend(format::changes::not_covered_line(&not_covered, undo.not_covered_more));
        }
        lines
    }

    fn end(&mut self, end: TurnEnd, notes: &[GaugedLine], size: Size) -> Step {
        let input = self.running.as_ref().is_some_and(Running::reads_keys);
        let settled = self.asking.take().is_some()
            || self.surface.take().is_some()
            || input
            || self.retained.is_some()
            || self.left_reads_keys();
        // A prompt that this view queued runs next: the view follows it.
        let next = self.input.as_mut().and_then(Input::next);
        let mut step = if next.is_some() {
            self.wrap_up()
        } else {
            let failed = matches!(end, TurnEnd::Failed(_));
            self.end_progress = Some(if failed { progress::FAILED } else { progress::CLEAR });
            self.close()
        };
        // NOTE: the files outside the project that the turn changed stand before its end
        // however it ended, also after a failure, and so does what undo can take back.
        for line in self.end_lines() {
            let noted = self.whole_note(&line, size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        let mut notes = notes.to_vec();
        // The last turn's failure is the command's error; an earlier one is a note.
        if next.is_some() {
            notes.extend(ended_note(&end).map(GaugedLine::from));
        }
        // The steers that this view's interrupt sent again run next: the note says so
        // after all that the interrupted turn showed, in the order that it happened.
        if std::mem::take(&mut self.resent_note) {
            notes.push(RESENT.to_owned().into());
        }
        for note in &notes {
            let noted = self.line_after(String::new(), note, size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        if let Some(input) = &mut self.input {
            // A prompt that never ran because the daemon restarted comes back to the
            // row; one that was taken back is already where it went.
            let prompt = input.prompt.take();
            if end == TurnEnd::Cancelled
                && let Some(prompt) = prompt
            {
                input.line.append(&prompt);
            }
            // Steers that no model call read come back too, so nothing typed is lost.
            for text in input.take_unread() {
                input.line.append(&text);
            }
        }
        match next {
            Some(next) => {
                self.advance(next);
                Step { settled, ..step }
            }
            None => {
                let still = self.left_at_end(size);
                step.out.push_str(&still.out);
                step.err.push_str(&still.err);
                Step { settled, end: Some(end), ask: still.ask, ..step }
            }
        }
    }

    /// Follows `next`, a prompt that this view queued, after the turn before it ended:
    /// it waits behind the running turn until it starts.
    fn advance(&mut self, next: Queued) {
        self.turn = next.turn;
        self.queued = true;
        self.message = None;
        self.next_index = 0;
        self.blocking.clear();
        self.interactive.clear();
        self.in_project = false;
        self.started_at = None;
        self.draft_boundary = None;
        self.interrupting = false;
        self.resent_note = false;
        self.ending.home_changes.clear();
        self.ending.full_exits.clear();
        if let Some(input) = &mut self.input {
            input.prompt = Some(next.text);
        }
        if self.terminal() {
            // The next turn has the same conversation, so the same context until it
            // counts again.
            let context = self.status.as_ref().and_then(|status| status.context().copied());
            let mut status = Status::new(self.look.motion);
            status.set(State::Queued);
            status.set_away(self.away);
            status.set_scope(self.scope.clone());
            if let Some(context) = context {
                status.set_context(context);
            }
            self.status = Some(status);
        }
        self.dirty = true;
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

    /// The block of call `call`, which ended as `outcome` at the time of the event being
    /// taken, written once, with `notes` under its result; a failed call keeps `lines`,
    /// the last of its output. A file write's diff stands above the result; the files
    /// that a call without a diff shown changed stand under it. When stdout is not a
    /// terminal, its first rows went to stderr before this event (`due_header`), and
    /// the rest follows now.
    fn call_block(
        &mut self,
        call: &Call,
        outcome: Outcome<'_>,
        lines: &[String],
        notes: &[String],
        changed: Changed<'_>,
        size: Size,
    ) -> Step {
        let options = self.options_at(size);
        let mut block = String::new();
        if outcome.keeps_tail() {
            block.push_str(&call::tail(lines, false, &options));
        }
        let limit = self.look.diff_lines;
        let diff = changed.diff.filter(|diff| limit > 0 && !diff.trim().is_empty());
        if let Some(diff) = diff {
            // NOTE: the line of a patch's call is its files with their counts, no path:
            // each file's diff names its own. The line of an `edit` call names its one
            // file, as a write's does.
            let patch =
                self.tools.get(&call.call_id).is_some_and(|tool| tool == format::patch::TOOL);
            let path = if patch { None } else { call.subject() };
            block.push_str(&call::diff(diff, path, limit, &options));
        }
        block.push_str(&call.result(self.event_at, outcome, &options));
        if let Some(changes) = changed.changes.filter(|_| diff.is_none()) {
            block.push_str(&call::changes(changes, &options));
        }
        block.push_str(&call::notes(notes, &options));
        if !self.terminal() {
            return Step { err: self.raw_err(block), ..Step::default() };
        }
        let mut committed = self.spacing.before(Block::Call).to_owned();
        committed.push_str(&call.header(&options));
        committed.push_str(&block);
        self.stage(&committed);
        Step::default()
    }

    /// The rows of call `call`, whose approval was denied or expired, with `notes`
    /// under them. It never ran, so it has no result. On a terminal its card gave its
    /// place to the answer line, so these rows follow that line and the scrollback
    /// keeps what did not run. When stdout is not a terminal, the rows went to stderr
    /// after the answer line, as on a terminal.
    fn denied_block(&mut self, call: &Call, notes: &[String], size: Size) -> Step {
        if !self.terminal() {
            return self.notes(notes, size);
        }
        let options = self.options_at(size);
        let mut committed = self.spacing.before(Block::Call).to_owned();
        committed.push_str(&call.header(&options));
        committed.push_str(&call::notes(notes, &options));
        self.stage(&committed);
        Step::default()
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
            live.push_str(self.spacing.peek(Block::Call));
            let spinner = match &self.status {
                Some(status) => status.spinner(now),
                None => spinner(self.look.motion, 0),
            };
            let context = self.status.as_ref().and_then(Status::context);
            let full = self.status.as_ref().is_some_and(Status::full);
            live.push_str(&call.running(spinner, now, full, context, &options));
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
        // NOTE: also after the end of the turn, while `efr` stays open for it.
        if let Some(left) = &self.left {
            live.push_str(&left.live(&options));
        }
        if let Some(shown) = &self.question {
            if shown.committed {
                live.push_str(&card::footer_row(&shown.footer, &options));
            } else {
                live.push_str(self.spacing.peek(Block::Question));
                live.push_str(&shown.card.render(Some(&shown.footer), &options));
            }
        }
        if let Some(input) = self.input.as_ref().filter(|_| !self.ended) {
            live.push_str(&input.pending(&options, effective_width(size)));
        }
        if live.len() != before {
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

/// `text` painted as `tone`, as rows with their newlines: on a terminal, a row wider
/// than the screen goes on in the next one, indented [`GOES_ON`] columns more, so a long
/// line of efr's own never wraps where the terminal chooses.
fn rows(text: &str, tone: Tone, options: &RenderOptions) -> String {
    let pieces = [(text.to_owned(), tone)];
    let rows = match format::columns(options) {
        Some((width, method)) => {
            format::wrap_spans(&pieces, width.max(1), width.saturating_sub(GOES_ON).max(1), method)
        }
        None => vec![pieces.to_vec()],
    };
    let mut out = String::new();
    for (at, row) in rows.iter().enumerate() {
        if at > 0 {
            out.push_str(&" ".repeat(GOES_ON));
        }
        for (text, tone) in row {
            out.push_str(&format::paint(text, *tone, options));
        }
        out.push('\n');
    }
    out
}

/// The last line of `tail` with text in it, without the spaces around it; empty when
/// there is none.
fn last_line(tail: &str) -> String {
    tail.lines().rev().map(str::trim).find(|line| !line.is_empty()).unwrap_or("").to_owned()
}

/// The line of `tail` that the cursor is on: the text after its last line break,
/// trimmed. Empty when `tail` ends with a line break, as the output of `grep password`
/// or `cat` does.
fn open_line(tail: &str) -> &str {
    tail.rsplit('\n').next().unwrap_or_default().trim()
}

/// An approval request as the view shows it.
struct Request<'a> {
    /// It is an approval of the turn that the followed one waits behind.
    blocking: bool,
    summary: &'a str,
    diff: Option<&'a str>,
    /// What the call would do outside the sandbox, for an exit.
    exit: Option<&'a ExitInfo>,
    /// How long the question waits before the call does not run, when it has a fixed
    /// limit, such as a delete that full mode guards.
    expires_in_secs: Option<u32>,
}

/// What the lines at the end of a turn name, gathered while it runs.
#[derive(Debug, Default)]
struct Ending {
    /// The files outside the project that the turn changed (`home_file_changed`), one
    /// entry for each path.
    home_changes: Vec<HomeChange>,
    /// What full mode let through in this turn (`exit_judged` with the judge `full`),
    /// each action once.
    full_exits: Vec<String>,
}

/// The note for a turn that ended as `end` while a prompt of this view waits after it:
/// the command goes on, so a failure is not its error.
/// The reason that `names` (what no snapshot of the turn covers, such as
/// `~/.aws/config (masked)`) gives for `shown`, taken out of `names`; `None` when it
/// names `shown` with no reason, or not at all.
fn take_reason(names: &mut Vec<String>, shown: &str) -> Option<String> {
    let at = names.iter().position(|name| {
        name.strip_prefix(shown).is_some_and(|rest| rest.starts_with(" (") && rest.ends_with(')'))
    })?;
    let name = names.remove(at);
    let rest = &name[shown.len() + 2..name.len() - 1];
    Some(rest.to_owned())
}

fn ended_note(end: &TurnEnd) -> Option<String> {
    match end {
        TurnEnd::Failed(body) => {
            Some(format!("the turn failed with {}: {}", body.code, format::one_line(&body.message)))
        }
        TurnEnd::Cancelled => {
            Some("the turn was cancelled because the daemon restarted".to_owned())
        }
        TurnEnd::Completed | TurnEnd::Interrupted | TurnEnd::Withdrawn => None,
    }
}

/// The row of the files that a call changed as one plain note, for a call whose block
/// is not shown; `None` when it changed none.
fn changes_note(changes: &FileChanges) -> Option<String> {
    let row: String =
        format::changes::call_row(changes).into_iter().map(|(text, _)| text).collect();
    (!row.is_empty()).then_some(row)
}

/// True for an event after which the drafts made before it show nothing new: the end
/// of the text of a message, the start of a tool call (after the whole answer of the
/// model), and the end of a turn. The daemon drops old drafts by the same rule.
fn ends_drafts(event: &Event) -> bool {
    matches!(
        event,
        Event::AssistantMessageCompleted { .. }
            | Event::ToolCallStarted { .. }
            | Event::TurnCompleted { .. }
            | Event::TurnFailed { .. }
            | Event::TurnInterrupted { .. }
            | Event::TurnCancelled { .. }
    )
}

#[cfg(test)]
mod tests;
