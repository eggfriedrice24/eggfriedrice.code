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
//! While the turn waits behind another one, the other turn's approvals show too: that
//! turn may be parked on a question nobody else will answer, and the prompt runs only
//! once it is answered.

use std::collections::{HashMap, HashSet};

use efr_protocol::{ApprovalDecision, CallId, ErrorBody, Event, Origin, TurnId};
use efr_render::{RenderOptions, Renderer, render, render_trace};

use crate::format::{self, Block, Spacing, Tone};
use crate::live::{LiveZone, Measured, effective_width};
use crate::terminal::{Size, at_width};

/// The question under a pending approval.
const QUESTION: &str = "allow? y = yes, n = no";

/// The heading of an approval of the followed turn.
const APPROVAL: &str = "approval needed:";

/// The heading of an approval of the turn that the followed one waits behind.
const BLOCKING_APPROVAL: &str = "the running turn needs approval:";

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
    /// An approval to ask the user about with one key.
    pub(crate) ask: Option<CallId>,
    /// The question that was asked is settled elsewhere; stop reading keys.
    pub(crate) settled: bool,
    /// The turn ended.
    pub(crate) end: Option<TurnEnd>,
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
                Step::default()
            }
            Event::AssistantMessageUpdated { index, offset, delta, .. } => {
                self.message_delta(*index, *offset, delta, size)
            }
            Event::AssistantMessageCompleted { index, text, .. } => {
                self.message_text(*index, text, true, size)
            }
            Event::ToolCallStarted { call_id, tool, input, .. } => {
                self.tools.insert(*call_id, tool.clone());
                // The model writes a tool call after the text it belongs to, so the
                // message before it is complete.
                let before = self.finish_message();
                self.note_after(before, &format::tool_call(tool, input), size)
            }
            Event::ToolCallCompleted { call_id, .. } if self.refused.contains(call_id) => {
                Step::default()
            }
            Event::ToolCallCompleted { call_id, is_error, exit_code, .. } => {
                let tool = self.tools.get(call_id).map_or("the tool", String::as_str);
                match format::tool_result(tool, *is_error, *exit_code) {
                    Some(line) => self.note(&line, size),
                    None => Step::default(),
                }
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
    /// show, and only until this one starts.
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
            return Step { out: before, err: render_trace(text, &options), ..Step::default() };
        }
        let mut committed = before;
        committed.push_str(self.spacing.before(Block::Note));
        committed.push_str(&render_trace(text, &options));
        Step { out: self.redraw(&committed, size), ..Step::default() }
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
        let committed = self.finish_message();
        self.commit(committed, size)
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
            step.ask = Some(call_id);
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
            step.err = text;
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
        let settled = self.asking.take().is_some();
        let mut step = self.close(size);
        if let Some(note) = note {
            let noted = self.note(note, size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        Step { settled, end: Some(end), ..step }
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
    /// live text, then the question when one is pending.
    fn redraw(&mut self, committed: &str, size: Size) -> String {
        let (mut live, mut measured) = match &self.message {
            Some(message) => (message.live.clone(), message.measured),
            None => (String::new(), None),
        };
        if self.asking.is_some() {
            let options = self.options_at(size);
            live.push_str(&format::paint(QUESTION, Tone::Dim, &options));
            live.push('\n');
            measured = None;
        }
        self.live.redraw(committed, &live, measured, size)
    }
}

/// An approval request as the view shows it.
struct Request<'a> {
    heading: &'static str,
    summary: &'a str,
    diff: Option<&'a str>,
}

#[cfg(test)]
mod tests;
