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
//! While a tool call of this turn runs on a terminal, its row (`call`) sits in the
//! live zone with the spinner and its time, in place of the status row, and the last
//! three lines of its output with text in them follow, muted and cut to the width. When
//! the call ends, one row is written once in its place that says how it ended, and a
//! failed call keeps its last lines of output. Reads that follow each other fold into
//! one row (`fold`). When the call's command waits for input and
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
//! preparing a tool call or running one. While the model reasons, one muted line in
//! italic above the status row shows the newest words of the reasoning, and a phase of
//! reasoning of 2 s or more leaves one row in the scrollback, such as `▸ Reading the
//! logs  thought 6s` (`reasoning`). Drafts ([`TurnView::draft`]), the part of a
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
//! What a call changed comes with its end: a file write's row names its file and its
//! counts, and the first lines of its diff follow (`render.diff_lines`, and
//! `render.diff_lines_long` from the 6th call of a turn on), a patch the first lines of
//! each file's diff, and a call that changed files without a diff shown gets one muted
//! row of them under it (`format::changes`). A turn that changed files ends with one
//! line that counts them and names three, right before the line of its time and
//! tokens. Before both, however the turn ended, comes one muted line for
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
//! starts a job says `(background)`, and the row under it names the job. A turn that a job
//! notice started is marked once. Ctrl+X asks whether to stop the newest running job
//! ([`TurnView::stop_question`]).
//!
//! Ctrl+B moves the running shell call of the followed turn to the background
//! ([`TurnView::background_call`]): efrd moves a call in the sandbox at once, and for a
//! call outside the sandbox it answers a preview, which the view asks about with one
//! key ([`TurnView::move_question`]). The empty input row names the key once such a
//! call has run for [`BACKGROUND_AFTER`]. A moved call's row says `moved to job j3`,
//! and the line of the job, from its `job_started`, follows it.

mod asking;
mod call;
mod calls;
mod ending;
mod fold;
mod input;
mod left;
mod message;
mod newest;
mod reasoning;
mod status;
mod waits;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use efr_config::DiffGroups;
use efr_protocol::{
    ApprovedBy, CallId, ContextUse, Draft, DraftPart, EffectiveSettings, ErrorBody, Event,
    EventEnvelope, ExitInfo, FileChanges, Grant, InputWait, JobId, JudgeKind, Launch, Presence,
    QuestionId, Seq, ShellCommandEnd, TurnId, TurnInterruptResult, TurnUndoInfo, Verdict,
};
use efr_render::{RenderOptions, WidthMethod, render_trace};
use jiff::Timestamp;
use unicode_width::UnicodeWidthChar as _;

use crate::format::card::{self, Card, Footer};
use crate::format::changes::HomeChange;
use crate::format::{self, Block, Spacing, Tone, jobs, sandbox};
use crate::keys::Key;
use crate::live::{LiveZone, effective_width};
use crate::progress;
use crate::row::Action;
use crate::terminal::{Size, at_width};
use call::Call;
use ending::Ending;
use fold::Fold;
use input::{Input, Queued, user_message};
use left::Left;
use message::Message;
use newest::ToolInput;
use reasoning::Reasoning;
use status::{Model, State, Status};
use waits::{HIDDEN_INPUT_SANDBOXED, echo_prefix, program, silence_hint};

pub(crate) use status::{TICK, spinner};

/// Hides the cursor while the status row shows.
const HIDE_CURSOR: &str = "\x1b[?25l";

/// Shows the cursor again.
const SHOW_CURSOR: &str = "\x1b[?25h";

/// The bell, once when a question card shows (`render.bell`).
const BELL: char = '\x07';

/// Turns bracketed paste on while the input row shows: a pasted newline never sends.
const PASTE_ON: &str = "\x1b[?2004h";

/// Turns bracketed paste off again.
const PASTE_OFF: &str = "\x1b[?2004l";

/// How the row under an interrupted turn starts when the interrupt sent the unread
/// steers again as a new prompt: `your steer "use TurnState" goes again as a new
/// prompt` (TM-Endings 20, `format::ending::resent_row`). Tests look for it.
#[cfg(test)]
const RESENT: &str = "your steer";

/// The note when the followed prompt was taken back before it ran.
const WITHDRAWN: &str = "the prompt was taken back before it ran";

/// What starts the title of an approval of the turn that the followed one waits
/// behind.
const BLOCKING: &str = "the running turn asks: ";

/// The mark of a refusal or of a failure.
const NO: &str = "\u{2717} ";

/// How much further a row of a line of efr's own goes on than its first row, as a row
/// of a question's card does.
const GOES_ON: usize = 2;

/// How long a shell call runs before the empty input row names Ctrl+B: a short call
/// ends before anyone could move it, so the hint would only flash.
pub(crate) const BACKGROUND_AFTER: Duration = Duration::from_secs(2);

/// The answer line when the call ended while its move question waited.
const MOVE_ENDED: &str = "the command ended; nothing moved";

/// The answer line when the call began to wait for input while its move question
/// waited: a call that waits for input cannot move.
const MOVE_WAITS: &str = "the command waits for input; nothing moved";

/// The name of the shell tool, whose running call the background key moves.
const SHELL_TOOL: &str = "shell";

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

    /// The answer line under the prompt of a command whose first program is `program`,
    /// as pieces: `? the command waits for input  enter answers · ctrl+c stops the
    /// command` (TM-Live 21-23).
    fn line(self, program: Option<&str>, glyphs: &efr_render::Glyphs) -> Vec<(String, Tone)> {
        waits::answer_line(self, program, glyphs)
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
    /// The question of the background key about call `call_id`, which runs outside the
    /// sandbox, answered with one key: `y` moves it with the preview's `token`
    /// ([`TurnView::move_answered`]), `n` leaves it in the turn.
    Move { call_id: CallId, token: u64 },
}

/// What a question on the screen asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum About {
    Approval(CallId),
    Surface(QuestionId),
    StopJob,
    Move,
}

/// The question on a terminal, as a card in the live zone until it is answered.
#[derive(Debug)]
struct Shown {
    card: Card,
    about: About,
    footer: Footer,
    /// A frame showed the card, with its bell when `render.bell` is on.
    rung: bool,
    /// The card is in the scrollback, because it did not fit on the screen; only its
    /// keys stay in the live zone.
    committed: bool,
    /// The card with the footer as the live zone showed it last, and the options and
    /// the footer it was rendered with. Each frame shows the card, and most frames
    /// change neither. In a box, so the view stays small in the futures that hold it.
    rendered: RefCell<Option<Box<(RenderOptions, Footer, String)>>>,
}

impl Shown {
    fn new(card: Card, about: About, footer: Footer) -> Shown {
        Shown { card, about, footer, rung: false, committed: false, rendered: RefCell::new(None) }
    }

    /// Adds the card with its footer at `options` to `out`. It is rendered again only
    /// when the options or the footer changed.
    fn push_card(&self, out: &mut String, options: &RenderOptions) {
        let mut rendered = self.rendered.borrow_mut();
        match rendered.as_deref() {
            Some((at, footer, text)) if at == options && *footer == self.footer => {
                out.push_str(text);
            }
            _ => {
                let text = self.card.render(Some(&self.footer), options);
                out.push_str(&text);
                *rendered = Some(Box::new((options.clone(), self.footer.clone(), text)));
            }
        }
    }
}

/// What the live slice keeps beside the view's own fields.
#[derive(Debug, Default)]
struct Kept {
    /// The model of the turn, for the status row.
    model: Option<Model>,
    /// What the question shown last asked about, with what it asked about and when it
    /// showed on the daemon's clock, for the line that takes its place.
    asked: Option<(About, Option<String>, Option<Timestamp>)>,
    /// The line of an `a` answer to the approval of a call, on a terminal, while it waits
    /// for the line of what the rule became: the two make one line.
    held: Option<(CallId, String)>,
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
    /// The lines of a file write's diff from the 6th call of a turn on, in place of
    /// `diff_lines` (`render.diff_lines_long`). The calls slice reads it.
    pub(crate) diff_lines_long: usize,
    /// One BEL with the frame that first shows a question card (`render.bell`).
    pub(crate) bell: bool,
}

impl Look {
    /// The look that `render` sets, with the progress bar when `progress` (already
    /// checked against the terminal).
    pub(crate) fn of(render: &efr_config::RenderSettings, progress: bool) -> Look {
        let lines = |count: u32| usize::try_from(count).unwrap_or(usize::MAX);
        Look {
            motion: render.motion,
            summary: render.turn_summary,
            progress,
            diff_lines: lines(render.diff_lines),
            diff_lines_long: lines(render.diff_lines_long),
            bell: render.bell,
        }
    }
}

/// What a call changed in the files of the turn's roots, from its
/// `tool_call_completed`.
#[derive(Debug, Clone, Copy, Default)]
struct Changed<'a> {
    changes: Option<&'a FileChanges>,
    /// The unified diff of a file tool's write.
    diff: Option<&'a str>,
}

/// What efrd's preview of a move to the background says, for the question of the
/// background key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Preview<'a> {
    /// What `confirm` sends back after a `y`.
    pub(crate) token: u64,
    /// The command line of the call.
    pub(crate) command: &'a str,
    /// The time limit of the job, in minutes.
    pub(crate) max_minutes: u32,
    /// The exits of the job decision.
    pub(crate) exits: &'a [ExitInfo],
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
    /// The call that the question of the background key asks about, while it waits for
    /// a key.
    moving: Option<CallId>,
    /// The calls of this turn that the user moved to a job, with the job.
    moved: HashMap<CallId, JobId>,
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
    /// The reasoning of the turn on a terminal: its words for the status row and its row.
    reasoning: Reasoning,
    /// The input of the tool call that the status row shows while the model writes it,
    /// for the newest words at the end of the row. In a box, as the reasoning is.
    tool_input: Option<Box<ToolInput>>,
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
    /// What the live slice keeps of the model, the question shown last and an answer
    /// that waits.
    // NOTE: boxed, so a view that follows a turn keeps the future of `efr send` small.
    kept: Box<Kept>,
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
    /// The reads that fold into one row (`fold`).
    // NOTE: boxed, so a view that follows a turn keeps the future of `efr send` small.
    fold: Box<Fold>,
    /// The tool calls of the turn so far: from the 6th on, a diff shows
    /// `render.diff_lines_long` lines.
    calls: usize,
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
            live: live_zone(options.width_method()),
            options,
            message: None,
            next_index: 0,
            reasoning: Reasoning::default(),
            tool_input: None,
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
            kept: Box::default(),
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
            fold: Box::default(),
            calls: 0,
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

    /// The same view with frames that write only what changed, as outside the tests.
    #[cfg(test)]
    pub(crate) fn writing_changes(mut self) -> TurnView {
        self.live = LiveZone::new(self.options.width_method());
        self
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
        self.resent_late(size)
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
            status.set_model(self.kept.model.clone());
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

    /// The same view, whose status row names the model of the turn's `settings`, such
    /// as `work/claude-opus-5-5 high`.
    pub(crate) fn with_model(mut self, settings: Option<&EffectiveSettings>) -> TurnView {
        self.kept.model = settings.map(Model::of);
        self
    }

    /// The same view, which prints paths below `home` as `~`.
    pub(crate) fn with_home(mut self, home: Option<PathBuf>) -> TurnView {
        self.home = home;
        self
    }

    /// True while an approval, the quarantine question or the question of the stop key
    /// or of the background key waits for a key.
    fn question_pending(&self) -> bool {
        self.asking.is_some()
            || self.surface.is_some()
            || self.jobs.stopping.is_some()
            || self.jobs.moving.is_some()
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
    /// Text merges with the persisted updates of its message; the input of a tool call
    /// only changes the status row. Reasoning changes the status row, until text, a tool
    /// call or the end of the turn ends its phase. A
    /// draft of another turn, or one older than an event of this turn that ends what
    /// drafts show, changes nothing.
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
                self.end_reasoning();
                self.state(State::Writing);
                self.message_delta(*index, *offset, delta, size)
            }
            DraftPart::Reasoning { offset, delta, title } => {
                let title =
                    title.as_deref().map(format::one_line).filter(|title| !title.trim().is_empty());
                if self.terminal() && self.reasoning.take(*offset, delta, title.clone()) {
                    self.dirty = true;
                }
                self.state(State::Thinking(title));
                Step::default()
            }
            DraftPart::ToolInput { call, tool, bytes, tail } => {
                self.end_reasoning();
                // The calls of one answer come one after another: the newest one shows.
                let shown = match self.status.as_ref().map(Status::state) {
                    Some(State::Preparing { call: shown, .. }) => Some(*shown),
                    _ => None,
                };
                if shown.is_some_and(|shown| *call < shown) {
                    return Step::default();
                }
                // NOTE: the input of the call that shows grows; any other starts anew,
                // as does call 0 of the next answer, which comes after another state.
                let same = shown == Some(*call)
                    && self.tool_input.as_ref().is_some_and(|input| input.call() == *call);
                if !same {
                    self.tool_input = None;
                }
                if let Some(tail) = tail.as_deref().filter(|_| self.terminal()) {
                    self.tool_input
                        .get_or_insert_with(|| Box::new(ToolInput::new(*call)))
                        .take(*bytes, tail);
                }
                let tool = format::one_line(tool);
                self.state(State::Preparing { call: *call, tool, bytes: *bytes, words: None });
                Step::default()
            }
            DraftPart::Context(context) => {
                self.context(*context);
                Step::default()
            }
            DraftPart::Compacting { .. } => {
                self.end_reasoning();
                self.state(State::Compacting);
                Step::default()
            }
            DraftPart::Connecting => {
                // NOTE: only a turn that waits for the model connects; the next attempt
                // after a wait keeps the words of the wait. The row says `connecting`
                // only after a while, so the tick shows it and this needs no frame.
                if let Some(status) = &mut self.status
                    && *status.state() == State::Model
                {
                    status.set(State::Connecting);
                }
                Step::default()
            }
            DraftPart::Connected => {
                match self.status.as_ref().map(Status::state) {
                    Some(State::Retrying { .. }) => self.state(State::Model),
                    // A fast connect showed nothing, and a slow one goes at the next tick.
                    Some(State::Connecting) => {
                        if let Some(status) = &mut self.status {
                            status.set(State::Model);
                        }
                    }
                    _ => {}
                }
                Step::default()
            }
            DraftPart::Retrying { attempt, wait_ms, reason, .. } => {
                self.end_reasoning();
                let wait = Duration::from_millis(*wait_ms);
                let reason = format::one_line(reason);
                self.state(State::Retrying { attempt: *attempt, wait, reason });
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

    /// True while text of the answer that arrived waits to show: the next frame shows
    /// new text.
    pub(crate) fn text_waits(&self) -> bool {
        self.message.as_ref().is_some_and(Message::waiting)
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
        self.reasoning.at(now);
        let options = self.options_at(size);
        if let Some(input) = &mut self.tool_input {
            input.at(now);
            if let Some(status) = &mut self.status {
                status.set_input_words(
                    input.call(),
                    input.words(options.glyphs().more, options.width_method()),
                );
            }
        }
        // NOTE: a reasoning without a title puts its newest words in the place of the
        // title. The state is set at each frame, because the words come with the
        // frames: a phase shows none in its first second.
        if let Some(words) = self.reasoning.words(options.glyphs().more, options.width_method())
            && let Some(status) = &mut self.status
            && matches!(status.state(), State::Thinking(_))
        {
            status.set(State::Thinking(Some(words)));
        }
        let body = self.live_body(size, now);
        // A running call's line carries the spinner instead of the row.
        let row = match &self.status {
            Some(status) if !self.ended && !asking && !call_shown => status.row(now, &options),
            _ => String::new(),
        };
        // NOTE: the hint names Ctrl+B only once the call has run for a moment, so a
        // short call does not make it flash.
        let background = call_shown
            && self.background_call().is_some()
            && self.call.as_ref().is_some_and(|call| call.ran_for(now) >= BACKGROUND_AFTER);
        if let Some(input) = &mut self.input {
            input.background = background;
        }
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
        out.push_str(&self.live.draw(&committed, &body, &row, &tail, size));
        // NOTE: one BEL with the frame that first shows a card, so most terminals mark
        // the tab (`render.bell`, off by default).
        if let Some(shown) = self.question.as_mut().filter(|shown| !shown.rung) {
            shown.rung = true;
            if self.look.bell {
                out.push(BELL);
            }
        }
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
        if ends_drafts(event) {
            self.end_reasoning();
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
        // NOTE: the line of an `a` answer waits only for the line of what it saved.
        let held = match event {
            Event::AlwaysRuleSaved { .. } => None,
            _ => self.release_answer(None, size),
        };
        let held = held.map(|step| step.err).unwrap_or_default();
        let step = self.turn_event(event, size, can_ask);
        Step { err: format!("{held}{header}{}", step.err), ..step }
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
            Event::TurnStarted { .. } => self.turn_started(event, size),
            Event::ModelSwitched { .. } => self.model_switched(event, size),
            Event::AssistantMessageUpdated { index, offset, delta, .. } => {
                // NOTE: drafts showed most of this text before it was recorded; an update
                // that adds nothing must not take the row back from a newer draft state.
                if *index >= self.next_index && self.adds_text(*index, *offset, delta) {
                    self.end_reasoning();
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
            Event::ToolCallStarted { .. } => self.tool_call_started(event),
            Event::ToolCallOutputUpdated { call_id, tail, .. } => self.output(*call_id, tail),
            Event::ToolCallInputChanged { call_id, input, looks_secret, .. } => {
                self.input_changed(*call_id, *input, *looks_secret, size, can_ask)
            }
            Event::ToolCallCompleted { .. } => self.tool_call_completed(event, size, can_ask),
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
                if self.kept.held.as_ref().is_some_and(|(held, _)| held == call_id) {
                    return self.release_answer(Some(&line), size).unwrap_or_default();
                }
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
                if job.moved_from.is_some() {
                    self.jobs.moved.insert(*call_id, job.id.clone());
                }
                let line = jobs::started(job, self.options.glyphs());
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
                    committed.push_str(self.spacing.before(Block::Steer));
                    committed.push_str(&user_message(&text, &options));
                    // NOTE: the blank line below a steer is written with it, so the live
                    // zone stands one row below it at once (TM-Sequence 12); the block
                    // after it then needs no blank line of its own.
                    committed.push('\n');
                    self.spacing = Spacing::default();
                }
                self.dirty = true;
                self.commit(committed)
            }
            Event::PromptWithdrawn { .. } => self.withdrawn_end(size),
            Event::TurnInterruptRequested { .. } if self.interrupting => Step::default(),
            Event::TurnInterruptRequested { origin, .. } => {
                self.note(&format!("interrupt requested from {}", format::origin(*origin)), size)
            }
            Event::ConversationCompacted { .. } => self.compacted(event, size),
            Event::TurnCompleted { .. } => self.turn_completed(event, size),
            Event::TurnFailed { .. } => self.turn_failed(event, size),
            Event::TurnInterrupted { .. } => self.turn_interrupted(event, size),
            Event::TurnCancelled { .. } => self.turn_cancelled(size),
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

    /// The call that the background key (Ctrl+B) can move: the running `shell` call of
    /// the followed turn, which does not start a job, waits for no approval and was not
    /// moved yet. With no call running, the call whose command was left running in the
    /// hidden shell, while that command waits for nothing (OQS16): efrd moves the
    /// command by its call's id. `None` while the followed prompt waits behind another
    /// turn: the view may not have seen the start of that turn's call, such as of a turn
    /// that a job notice started.
    pub(crate) fn background_call(&self) -> Option<CallId> {
        if self.queued {
            return None;
        }
        let Some(call) = self.call.as_ref() else {
            let left = self.left.as_deref()?;
            let movable = left.wait == InputWait::None
                && left.asking.is_none()
                && !self.jobs.moved.contains_key(&left.call_id);
            return movable.then_some(left.call_id);
        };
        let shell = self.tools.get(&call.call_id).is_some_and(|tool| tool == SHELL_TOOL);
        let movable = shell
            && !call.awaiting
            && !call.background()
            && !self.ended
            && !self.jobs.moved.contains_key(&call.call_id);
        movable.then_some(call.call_id)
    }

    /// efrd moved call `call_id` to job `job_id`. The call's block says so when it ends;
    /// its `job_started` names the job too.
    pub(crate) fn moved(&mut self, call_id: CallId, job_id: JobId) {
        self.jobs.moved.insert(call_id, job_id);
        self.dirty = true;
    }

    /// efrd did not move call `call_id`: a note says why, and the keys go back to the
    /// call when it held them.
    pub(crate) fn not_moved(&mut self, call_id: CallId, reason: &str, size: Size) -> Step {
        let step = self.whole_note(&jobs::not_moved(reason), size);
        self.keys_back(call_id, step)
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

    /// A note written after `before`, the end of a message: muted and cut to the
    /// screen on a terminal, plain otherwise.
    fn note_after(&mut self, before: String, text: &str, size: Size) -> Step {
        self.rendered_after(before, |options| render_trace(text, options), size)
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
        let secret_in_sandbox = left.contained && left.secret_wait();
        let note = format::hidden_shell::ended(&left.command(), exit_code, how, secret_in_sandbox);
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
        let raw = left.raw(&self.options);
        let mut step = if self.terminal() {
            self.commit(String::new())
        } else {
            let spacing = self.err_spacing.before(Block::Note);
            let mut err = self.raw_err(format!("{spacing}{raw}"));
            if shown {
                err.push_str(&echo_prefix(self.options.glyphs()));
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
        let line = left.end_line(kind.is_some(), self.options.glyphs());
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
        self.end_reasoning();
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
        self.flush_fold();
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

    /// True when the text of message `index` from `offset` on reaches past the text held.
    fn adds_text(&self, index: u32, offset: u64, delta: &str) -> bool {
        let held = self
            .message
            .as_ref()
            .filter(|message| message.index == index)
            .map_or(0, |message| message.received().len());
        usize::try_from(offset).map_or(true, |offset| offset.saturating_add(delta.len()) > held)
    }

    /// An update of message `index`: `delta` at byte `offset` of its text. An update
    /// that starts past what this view holds, because it joined in the middle of the
    /// message, is skipped; the completed message fills the gap. It costs the delta,
    /// not the text that arrived before it.
    fn message_delta(&mut self, index: u32, offset: u64, delta: &str, size: Size) -> Step {
        let held = self
            .message
            .as_ref()
            .filter(|message| message.index == index)
            .map_or("", Message::received);
        let Some(offset) =
            usize::try_from(offset).ok().filter(|offset| held.get(..*offset).is_some())
        else {
            return Step::default();
        };
        self.message_with(index, false, size, |message| message.receive_at(offset, delta))
    }

    fn message_text(&mut self, index: u32, text: &str, complete: bool, size: Size) -> Step {
        self.message_with(index, complete, size, |message| message.receive(text))
    }

    /// Message `index` takes text through `receive`, after the message before it ends.
    fn message_with(
        &mut self,
        index: u32,
        complete: bool,
        size: Size,
        receive: impl FnOnce(&mut Message),
    ) -> Step {
        if index < self.next_index {
            return Step::default();
        }
        // NOTE: a `⇄` line that waits in the live zone stands before the reply.
        self.release_switch();
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
            receive(message);
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
        // the output that comes ahead of it. The fold of reads stands before the message.
        self.flush_fold();
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

    /// Clears the question when it was about `call_id`; true when it was.
    fn settle(&mut self, call_id: CallId) -> bool {
        let settled = self.asking == Some(call_id);
        if settled {
            self.asking = None;
        }
        settled
    }

    /// Follows `next`, a prompt that this view queued, after the turn before it ended:
    /// it waits behind the running turn until it starts.
    fn advance(&mut self, next: Queued) {
        self.turn = next.turn;
        self.queued = true;
        self.message = None;
        self.next_index = 0;
        self.reasoning = Reasoning::default();
        self.blocking.clear();
        self.interactive.clear();
        self.in_project = false;
        self.started_at = None;
        self.draft_boundary = None;
        self.interrupting = false;
        self.resent_note = false;
        self.calls = 0;
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

    /// Ends the phase of reasoning that runs: a phase that took long enough leaves its
    /// row in the scrollback. The row sticks to the text or the
    /// tool call that follows it, so the blank line before that goes above the row; any
    /// other block, such as a question or the end line, keeps its blank line. Inside a
    /// message that still streams there is no row, because it would cut the text of the
    /// message in two.
    fn end_reasoning(&mut self) {
        if !self.reasoning.running() {
            return;
        }
        self.dirty = true;
        let options = self.options_now();
        let Some(row) = self.reasoning.end(&options).filter(|_| self.message.is_none()) else {
            return;
        };
        let mut committed = self.spacing.before(Block::Message).to_owned();
        committed.push_str(&row);
        self.spacing = Spacing::stuck();
        self.stage(&committed);
    }

    /// Keeps `committed` for the next frame, after the text that the current message
    /// still holds back, and marks the live zone for that frame.
    fn stage(&mut self, committed: &str) {
        // NOTE: a `⇄` line that waits in the live zone stands before any other block.
        self.release_switch();
        // NOTE: the fold of reads ends with anything that is written after it.
        if !committed.is_empty() {
            self.flush_fold();
        }
        if !committed.is_empty()
            && let Some(message) = &mut self.message
        {
            let held = message.push_all();
            self.pending.push_str(&held);
        }
        self.pending.push_str(committed);
        self.dirty = true;
    }

    /// The live zone above the status row at `now`: the current message's live text,
    /// the running call's line, the last lines of its output
    /// and the input it waits for, then the question when one is pending.
    fn live_body(&self, size: Size, now: Timestamp) -> String {
        let options = self.options_at(size);
        let mut live = self.switch_live(&options);
        live.push_str(self.message.as_ref().map_or("", |message| message.live()));
        live.push_str(&self.fold.live(&options));
        if self.call_shown()
            && let Some(call) = &self.call
        {
            live.push_str(self.spacing.peek(Block::Call));
            let spinner = match &self.status {
                Some(status) => status.spinner(now, options.glyphs()),
                None => spinner(self.look.motion, 0, options.glyphs()),
            };
            let context = self.status.as_ref().and_then(Status::context);
            let full = self.status.as_ref().is_some_and(Status::full);
            live.push_str(&call.running(spinner, now, full, context, &options));
        }
        if let Some(running) = &self.running {
            live.push_str(&call::tail(&running.lines, running.asking.is_some(), &options));
            if let Some(kind) = running.asking {
                let command = self.commands.get(&running.call_id).map(String::as_str);
                let line = kind.line(command.and_then(program), options.glyphs());
                live.push_str(&waits::piece_row(&line, &options));
                live.push_str(&waits::echo_row(
                    kind.shown().then_some(running.typed.as_str()),
                    &options,
                ));
            } else if running.hinted && !self.question_pending() {
                live.push_str(&waits::piece_row(
                    &[(silence_hint(options.glyphs()), Tone::Dim)],
                    &options,
                ));
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
                shown.push_card(&mut live, &options);
            }
        }
        if let Some(input) = self.input.as_ref().filter(|_| !self.ended) {
            live.push_str(&input.pending(&options, effective_width(size)));
        }
        live
    }
}

/// The live zone of a view: its frames write only what changed.
#[cfg(not(test))]
fn live_zone(method: WidthMethod) -> LiveZone {
    LiveZone::new(method)
}

/// The live zone of a view in the tests: its frames erase the whole live zone and write
/// it again. NOTE: the view tests read the text of a frame or play it on screens that
/// know only such frames. The frames that write only what changed leave the same screen
/// (`live::tests`, `tests/changes.rs`), and [`TurnView::writing_changes`] gives a test
/// those.
#[cfg(test)]
fn live_zone(method: WidthMethod) -> LiveZone {
    LiveZone::whole(method)
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
