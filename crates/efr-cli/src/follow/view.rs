//! What a followed turn looks like: the turn's events in, the text to write out.
//!
//! No IO here: the follow loop feeds events and writes what comes back, so the whole
//! appearance of a turn can be tested with events and a fixed screen size.
//!
//! On a terminal, assistant messages stream through an `efr_render::Renderer`: its
//! committed output is written once and its live zone is redrawn in place through a
//! [`LiveZone`]. Notes (tool calls, answers, the end of the turn) are dim lines between
//! the messages, and a pending approval question sits below the live zone. When stdout
//! is not a terminal, the messages are written as raw markdown and everything else
//! goes to stderr, so stdout holds the reply alone.
//!
//! While the turn waits behind another one, the other turn's approvals show too, and so
//! do its running call's last output line and the input it waits for: that turn may be
//! parked on a question nobody else will answer, and the prompt runs only once it is
//! answered. This client tells the daemon that a person here can answer, so it must
//! ask for the running turn's input as well.
//!
//! While a tool call runs, the last line of its output with text in it sits dim in the
//! live zone, cut to the width, and goes when the call completes; it is never
//! committed. When the call's command waits for input and keys can be read, the view
//! asks for an answer line below it: a hidden answer (a password) never reaches the
//! view at all, and a visible one is echoed here as the user types it, unless its
//! prompt looks like a password prompt behind another program (`looks_secret`).
//!
//! A shell call of this turn that reports no wait and has printed nothing for a while
//! gets one dim line that offers `Ctrl+\` ([`TurnView::silence`], [`TurnView::silent`]).
//! The view reads no key for it: text typed meanwhile stays typeahead for the user's
//! shell. Only `Ctrl+\` opens an answer line ([`TurnView::manual`]), which is shown as it
//! is typed and goes as a manual answer.

use std::collections::{HashMap, HashSet};

use efr_protocol::{ApprovalDecision, CallId, ErrorBody, Event, InputWait, Origin, TurnId};
use efr_render::{RenderOptions, Renderer, render, render_trace};
use unicode_width::UnicodeWidthChar as _;

use crate::format::{self, Block, Spacing, Tone};
use crate::live::{LiveZone, Measured, effective_width};
use crate::terminal::{Size, at_width};

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

/// The line under a shell call that has printed nothing for a while and reports no
/// wait; the follow loop shows it after [`SILENCE`](crate::follow::SILENCE).
const SILENCE_HINT: &str = "no output for 10 s; press Ctrl+\\ to type an input for the command";

/// The note when a command waits for hidden input and no key can be read here.
const HIDDEN_INPUT_ELSEWHERE: &str =
    "the command waits for hidden input, such as a password; efr cannot ask for it here";

/// The note when a command waits for visible input and no key can be read here.
const VISIBLE_INPUT_ELSEWHERE: &str = "the command waits for input; efr cannot ask for it here";

/// The note after an answer reached the command.
const ANSWER_SENT: &str = "answer sent";

/// The note when the daemon refused an answer because the wait was over.
const ANSWER_REFUSED: &str = "the command no longer waits for that input; nothing was sent";

/// The name of the tool whose silent calls offer `Ctrl+\`.
const SHELL: &str = "shell";

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
    /// Text for stdout.
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
    /// A line the user asked for with `Ctrl+\` while the command reported no wait: shown
    /// as it is typed, sent as a visible, manual answer.
    Manual,
}

impl AnswerKind {
    /// True when what is typed is shown as it is typed.
    pub(crate) fn shown(self) -> bool {
        matches!(self, AnswerKind::Visible | AnswerKind::Manual)
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
            AnswerKind::Visible | AnswerKind::Manual => VISIBLE_INPUT,
            AnswerKind::Masked => SECRET_INPUT,
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
    /// `call_id` completes. It asked for a hidden answer and may ask again, as `sudo`
    /// does after a wrong password, and a password typed again meanwhile must neither
    /// show nor stay queued for the user's shell.
    Discard(CallId),
}

/// The tool call whose output is arriving now.
#[derive(Debug)]
struct Running {
    call_id: CallId,
    /// The last line of its output with text in it.
    tail: String,
    /// What its command waits for, as the daemon said last.
    wait: InputWait,
    /// The answer the user is being asked for now, if any.
    asking: Option<AnswerKind>,
    /// It asked for an answer that may be a password and still runs: keys are read and
    /// thrown away while it asks nothing.
    guarding: bool,
    /// What the user typed so far, for a visible answer only.
    typed: String,
    /// A shell call of the followed turn, which may offer `Ctrl+\` when it is silent.
    shell: bool,
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
            wait: InputWait::None,
            asking: None,
            guarding: false,
            typed: String::new(),
            shell: false,
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

    /// True when the call is a shell call that waits for nothing the daemon reported and
    /// nothing reads keys for it.
    fn quiet_shell(&self) -> bool {
        self.shell && self.wait == InputWait::None && !self.reads_keys()
    }
}

/// The assistant message that is streaming now.
#[derive(Debug)]
struct Message {
    index: u32,
    renderer: Renderer,
    /// The text pushed into the renderer so far.
    pushed: String,
    /// The renderer's live zone after the last push, and its height.
    live: String,
    measured: Option<Measured>,
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
    /// When stdout is not a terminal: stderr's last line is the echo of a visible answer
    /// (`> ` and what is typed), without its newline.
    echo_line: bool,
}

impl TurnView {
    /// A view of turn `turn` rendered with `options`.
    pub(crate) fn new(turn: TurnId, options: RenderOptions) -> TurnView {
        TurnView {
            turn,
            options,
            message: None,
            next_index: 0,
            tools: HashMap::new(),
            live: LiveZone::default(),
            spacing: Spacing::default(),
            raw_messages: 0,
            asking: None,
            answered: None,
            queued: false,
            blocking: HashSet::new(),
            refused: HashSet::new(),
            running: None,
            echo_line: false,
        }
    }

    /// The turn waits behind the running one: until it starts, the running turn's
    /// approvals show and can be answered here.
    pub(crate) fn queue(&mut self) {
        self.queued = true;
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

    /// Takes one event of the conversation. Events of other turns change nothing,
    /// except approvals while this turn waits behind another one. `can_ask` says
    /// whether one-key answers can be read.
    pub(crate) fn event(&mut self, event: &Event, size: Size, can_ask: bool) -> Step {
        if event.turn_id() != Some(self.turn) {
            return self.other_turn(event, size, can_ask);
        }
        match event {
            Event::TurnStarted { .. } => {
                self.queued = false;
                // A call of the turn ahead that is still shown is over now.
                match self.running.take() {
                    Some(running) => {
                        Step { settled: running.reads_keys(), ..self.commit(String::new(), size) }
                    }
                    None => Step::default(),
                }
            }
            Event::AssistantMessageUpdated { index, offset, delta, .. } => {
                self.message_delta(*index, *offset, delta, size)
            }
            Event::AssistantMessageCompleted { index, text, .. } => {
                self.message_text(*index, text, true, size)
            }
            Event::ToolCallStarted { call_id, tool, input, .. } => {
                self.tools.insert(*call_id, tool.clone());
                // A shell call is followed from its start, so a command that never prints
                // can still offer `Ctrl+\`.
                let mut settled = false;
                if tool == SHELL {
                    let (running, replaced) = self.running(*call_id);
                    running.shell = true;
                    settled = replaced;
                }
                // The model writes a tool call after the text it belongs to, so the
                // message before it is complete.
                let before = self.finish_message();
                Step { settled, ..self.note_after(before, &format::tool_call(tool, input), size) }
            }
            Event::ToolCallOutputUpdated { call_id, tail, .. } => self.output(*call_id, tail, size),
            Event::ToolCallInputChanged { call_id, input, looks_secret, .. } => {
                self.input_changed(*call_id, *input, *looks_secret, size, can_ask)
            }
            Event::ToolCallCompleted { call_id, is_error, exit_code, .. } => {
                let settled = self.call_ended(*call_id);
                let tool = self.tools.get(call_id).map_or("the tool", String::as_str);
                let line = format::tool_result(tool, *is_error, *exit_code)
                    .filter(|_| !self.refused.contains(call_id));
                let step = match line {
                    Some(line) => self.note(&line, size),
                    // The live tail goes either way.
                    None => self.commit(String::new(), size),
                };
                Step { settled, ..step }
            }
            Event::ApprovalRequested { call_id, summary, diff_preview, .. } => {
                let request = Request { heading: APPROVAL, summary, diff: diff_preview.as_deref() };
                self.approval(*call_id, &request, size, can_ask)
            }
            Event::ApprovalResolved { call_id, decision, origin, .. } => {
                self.resolved(*call_id, *decision, *origin, size)
            }
            Event::ApprovalExpired { call_id, .. } => self.expired(*call_id, size),
            Event::TurnSteered { text, .. } => {
                self.note(&format!("steered: {}", format::one_line(text)), size)
            }
            Event::TurnInterruptRequested { origin, .. } => {
                self.note(&format!("interrupt requested from {}", format::origin(*origin)), size)
            }
            Event::TurnCompleted { .. } => self.end(TurnEnd::Completed, None, size),
            Event::TurnFailed { error, .. } => self.end(TurnEnd::Failed(error.clone()), None, size),
            Event::TurnInterrupted { .. } => {
                self.end(TurnEnd::Interrupted, Some("interrupted"), size)
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
            Event::ApprovalRequested { call_id, summary, diff_preview, .. } => {
                self.blocking.insert(*call_id);
                let request =
                    Request { heading: BLOCKING_APPROVAL, summary, diff: diff_preview.as_deref() };
                self.approval(*call_id, &request, size, can_ask)
            }
            Event::ApprovalResolved { call_id, decision, origin, .. }
                if self.blocking.contains(call_id) =>
            {
                self.resolved(*call_id, *decision, *origin, size)
            }
            Event::ApprovalExpired { call_id, .. } if self.blocking.contains(call_id) => {
                self.expired(*call_id, size)
            }
            Event::ToolCallOutputUpdated { call_id, tail, .. } => self.output(*call_id, tail, size),
            Event::ToolCallInputChanged { call_id, input, looks_secret, .. } => {
                self.input_changed(*call_id, *input, *looks_secret, size, can_ask)
            }
            Event::ToolCallCompleted { call_id, .. }
                if self.running.as_ref().is_some_and(|running| running.call_id == *call_id) =>
            {
                let settled = self.call_ended(*call_id);
                Step { settled, ..self.commit(String::new(), size) }
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
        Step { out: self.redraw(&committed, size), ..Step::default() }
    }

    /// The output of call `call_id` grew and now ends in `tail`.
    fn output(&mut self, call_id: CallId, tail: &str, size: Size) -> Step {
        let (running, settled) = self.running(call_id);
        running.tail = last_line(tail);
        running.stirred();
        Step { settled, ..self.commit(String::new(), size) }
    }

    /// The shell call that may offer `Ctrl+\` once it has been silent long enough, with
    /// its activity count, which changes at every sign of life: a shell call of this
    /// turn that runs, reports no wait, while no key is read for it and no approval
    /// waits, and whose line that offers `Ctrl+\` is not shown yet.
    pub(crate) fn silence(&self) -> Option<(CallId, u64)> {
        let running = self.running.as_ref()?;
        let quiet = running.quiet_shell() && !running.hinted && self.asking.is_none();
        quiet.then_some((running.call_id, running.activity))
    }

    /// The call whose line that offers `Ctrl+\` is shown now; the follow loop waits for
    /// the key exactly while there is one.
    pub(crate) fn manual_offer(&self) -> Option<CallId> {
        let running = self.running.as_ref()?;
        let offered = running.quiet_shell() && running.hinted && self.asking.is_none();
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
            self.commit(String::new(), size)
        } else {
            self.note(SILENCE_HINT, size)
        }
    }

    /// The user pressed `Ctrl+\` while the line for call `call_id` was shown: asks for a
    /// line that is shown as it is typed and goes as a manual answer.
    pub(crate) fn manual(&mut self, call_id: CallId, size: Size) -> Step {
        if self.manual_offer() != Some(call_id) {
            return Step::default();
        }
        if let Some(running) = &mut self.running {
            running.hinted = false;
        }
        self.ask_for(call_id, AnswerKind::Manual, size)
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
        let approval_pending = self.asking.is_some();
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
                let mut step = self.commit(String::new(), size);
                step.ask = Some(Ask::Discard(call_id));
                return step;
            }
            _ => return Step { settled, ..self.commit(String::new(), size) },
        };
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
            self.commit(String::new(), size)
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

    /// Call `call_id` completed: its tail and any question for it go. True when keys
    /// were read for it.
    fn call_ended(&mut self, call_id: CallId) -> bool {
        match self.running.take() {
            Some(running) if running.call_id == call_id => running.reads_keys(),
            other => {
                self.running = other;
                false
            }
        }
    }

    /// What the user typed so far for a shown answer, echoed below the prompt: in the
    /// live zone on a terminal, otherwise on stderr after `> `, where a removed
    /// character is erased with Backspace. An answer that is not shown is never passed
    /// here, and text passed while no shown answer is asked for is dropped.
    pub(crate) fn typed(&mut self, text: &str, size: Size) -> Step {
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
            return self.commit(String::new(), size);
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
        let mut settled = false;
        if let Some(running) = &mut self.running {
            running.typed.clear();
            if running.asking.is_some_and(AnswerKind::manual) {
                running.asking = None;
                running.stirred();
                settled = true;
            }
        }
        Step { settled, ..self.note(text, size) }
    }

    /// The user answered the approval `call_id` with a key.
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
        self.note(format::decision(decision), size)
    }

    /// Ends the view early: commits what the current message has so far and clears the
    /// live zone, before an error or an interrupt is reported.
    pub(crate) fn close(&mut self, size: Size) -> Step {
        self.asking = None;
        self.running = None;
        let committed = self.finish_message();
        // An echo line left open would carry what is written after the view.
        let err = self.raw_err(String::new());
        Step { err, ..self.commit(committed, size) }
    }

    /// An update of message `index`: `delta` at byte `offset` of its text. An update
    /// that starts past what this view holds, because it joined in the middle of the
    /// message, is skipped; the completed message fills the gap.
    fn message_delta(&mut self, index: u32, offset: u64, delta: &str, size: Size) -> Step {
        let held = self
            .message
            .as_ref()
            .filter(|message| message.index == index)
            .map_or("", |message| message.pushed.as_str());
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
            let options = self.options_at(size);
            self.message = Some(Message {
                index,
                renderer: Renderer::new(options),
                pushed: String::new(),
                live: String::new(),
                measured: None,
            });
        }
        if let Some(message) = &mut self.message {
            match text.strip_prefix(message.pushed.as_str()) {
                Some("") => {}
                Some(delta) => {
                    let update = message.renderer.push(delta);
                    committed.push_str(update.committed());
                    message.live = update.live().to_owned();
                    let width = message.renderer.options().width();
                    message.measured = Some(Measured { rows: update.live_rows(), width });
                    message.pushed.push_str(delta);
                }
                // Committed output cannot be taken back, so a message that rewrites text
                // it already sent keeps what is on the screen.
                None => tracing::debug!(index, "an assistant message rewrote sent text"),
            }
        }
        if complete {
            committed.push_str(&self.finish_message());
        }
        self.commit(committed, size)
    }

    /// Finishes the current message and returns the rest of its output.
    fn finish_message(&mut self) -> String {
        let Some(message) = self.message.take() else {
            return String::new();
        };
        self.next_index = message.index.saturating_add(1);
        let mut rest = message.renderer.finish();
        if !self.terminal() {
            self.raw_messages += 1;
            if !message.pushed.is_empty() && !message.pushed.ends_with('\n') {
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
        let mut text = format!(
            "{} {}\n",
            format::paint(request.heading, Tone::Attention, &options),
            format::one_line(request.summary)
        );
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
        if self.terminal() {
            let mut committed = std::mem::take(&mut step.out);
            committed.push_str(self.spacing.before(Block::Approval));
            committed.push_str(&text);
            step.out = self.redraw(&committed, size);
        } else {
            step.err = self.raw_err(text);
        }
        step
    }

    /// Clears the question when it was about `call_id`; true when it was.
    fn settle(&mut self, call_id: CallId) -> bool {
        let settled = self.asking == Some(call_id);
        if settled {
            self.asking = None;
        }
        settled
    }

    fn end(&mut self, end: TurnEnd, note: Option<&str>, size: Size) -> Step {
        let input = self.running.as_ref().is_some_and(Running::reads_keys);
        let settled = self.asking.take().is_some() || input;
        let mut step = self.close(size);
        if let Some(note) = note {
            let noted = self.note(note, size);
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
    fn commit(&mut self, committed: String, size: Size) -> Step {
        if self.terminal() {
            Step { out: self.redraw(&committed, size), ..Step::default() }
        } else {
            Step { out: committed, ..Step::default() }
        }
    }

    /// Redraws the live zone with `committed` written above it: the current message's
    /// live text, the running call's tail and the input it waits for, then the
    /// question when one is pending.
    fn redraw(&mut self, committed: &str, size: Size) -> String {
        let (mut live, mut measured) = match &self.message {
            Some(message) => (message.live.clone(), message.measured),
            None => (String::new(), None),
        };
        if let Some(running) = &self.running {
            let options = self.options_at(size);
            let before = live.len();
            if !running.tail.is_empty() {
                live.push_str(&render_trace(&format::one_line(&running.tail), &options));
            }
            if let Some(kind) = running.asking {
                live.push_str(&format::paint(kind.line(), Tone::Attention, &options));
                live.push('\n');
                if kind.shown() {
                    live.push_str(ECHO_PREFIX);
                    live.push_str(&format::one_line(&running.typed));
                    live.push('\n');
                }
            } else if running.hinted && self.asking.is_none() {
                live.push_str(&format::paint(SILENCE_HINT, Tone::Dim, &options));
                live.push('\n');
            }
            if live.len() != before {
                measured = None;
            }
        }
        if self.asking.is_some() {
            let options = self.options_at(size);
            live.push_str(&format::paint(QUESTION, Tone::Dim, &options));
            live.push('\n');
            measured = None;
        }
        self.live.redraw(committed, &live, measured, size)
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
}

#[cfg(test)]
mod tests;
