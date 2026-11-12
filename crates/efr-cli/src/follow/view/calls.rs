//! The events of a tool call in the view: its start, its output and its end, and the
//! block that a call leaves in the scrollback. The calls slice of the terminal look
//! owns this file: an `impl TurnView` block next to `view.rs`.

use efr_protocol::{CallId, Event, FileChanges, Grant, Launch, looks_secret};

use super::call::{Call, Outcome};
use super::left::Left;
use super::status::State;
use super::{
    About, Ask, Changed, MOVE_ENDED, NO, Running, Step, TurnView, call, last_line, open_line,
};
use crate::format::{self, Block, sandbox};
use crate::terminal::Size;

impl TurnView {
    /// The start of a tool call: its block shows in the live zone on a terminal, and its
    /// first rows wait for stderr elsewhere (`due_header`).
    pub(super) fn tool_call_started(&mut self, event: &Event) -> Step {
        let Event::ToolCallStarted { call_id, tool, input, manual_input, launch, .. } = event
        else {
            return Step::default();
        };
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

    /// The end of a tool call: its block is written once with its outcome, its tail, its
    /// notes and what it changed.
    pub(super) fn tool_call_completed(&mut self, event: &Event, size: Size, can_ask: bool) -> Step {
        let Event::ToolCallCompleted {
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
        } = event
        else {
            return Step::default();
        };
        self.state(State::Model);
        let lines = match &self.running {
            Some(running) if running.call_id == *call_id && !running.lines.is_empty() => {
                running.lines.clone()
            }
            _ => call::output_lines(output),
        };
        let mut settled = self.call_ended(*call_id);
        // NOTE: the move question about the call asks nothing any more; its keys
        // go back.
        let ended_question = match self.jobs.moving.take_if(|moving| moving == call_id) {
            Some(_) => {
                settled = true;
                self.answer_line(About::Move, false, MOVE_ENDED, size)
            }
            None => Step::default(),
        };
        let moved = self.jobs.moved.remove(call_id);
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
        if *left_running && !self.left.as_ref().is_some_and(|left| left.call_id == *call_id) {
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
            notes
                .extend(sandbox::bus_refused_lines(&summary.bus_refused, summary.bus_refused_more));
            notes.extend(sandbox::background_stopped(&summary.background_stopped));
            notes.extend(sandbox::survivors(&summary.survivors));
        }
        let step = match self.call.take().filter(|call| call.call_id == *call_id) {
            Some(call) if denied && refusal.is_none() => self.denied_block(&call, &notes, size),
            Some(call) => {
                let outcome = match (refusal, setup, *exit_code, &moved) {
                    (Some(reason), ..) => Outcome::Refused(reason),
                    (None, Some(reason), ..) => Outcome::NotStarted(reason),
                    (None, None, ..) if *left_running => Outcome::LeftRunning,
                    (None, None, _, Some(job)) => Outcome::Moved(job),
                    (None, None, Some(code), None) if code != 0 => Outcome::Exited(code),
                    (None, None, None, None) if *is_error => Outcome::Failed,
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
        let err = format!("{}{}", ended_question.err, step.err);
        Step { settled, err, ..step }
    }

    /// The block of call `call`, which ended as `outcome` at the time of the event being
    /// taken, written once, with `notes` under its result; a failed call keeps `lines`,
    /// the last of its output. A file write's diff stands above the result; the files
    /// that a call without a diff shown changed stand under it. When stdout is not a
    /// terminal, its first rows went to stderr before this event (`due_header`), and
    /// the rest follows now.
    pub(super) fn call_block(
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
    pub(super) fn denied_block(&mut self, call: &Call, notes: &[String], size: Size) -> Step {
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

    /// The output of call `call_id` grew and now ends in `tail`. With the input row,
    /// an open last line that looks like a password prompt holds the keys until the
    /// daemon reports the wait ([`Running::held`]), and a new last line lets them go.
    /// A line that ends with a line break is not a prompt: the daemon too tests only
    /// the line that the cursor is on.
    pub(super) fn output(&mut self, call_id: CallId, tail: &str) -> Step {
        let free = self.input.is_some() && !self.question_pending();
        let retained = self.retained == Some(call_id);
        let (running, settled) = self.running(call_id);
        running.tail = last_line(tail);
        running.lines = call::last_lines(tail);
        running.stirred();
        let quiet = running.asking.is_none() && !running.guarding && !retained;
        let hold = free && quiet && looks_secret(open_line(tail));
        // NOTE: the question of the background key reads the keys; whether the call
        // holds them is settled when it is answered ([`keys_back`](Self::keys_back)).
        if self.jobs.moving.is_some() {
            return self.commit(String::new());
        }
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

    /// Call `call_id` completed: its tail, any question for it and the keys kept for it
    /// go. True when keys were read for it.
    pub(super) fn call_ended(&mut self, call_id: CallId) -> bool {
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

    /// The running call, made `call_id` when it was another one or none; true when
    /// another one was asking for input, which therefore stops.
    pub(super) fn running(&mut self, call_id: CallId) -> (&mut Running, bool) {
        let mut replaced = false;
        if let Some(running) = &self.running
            && running.call_id != call_id
        {
            replaced = running.reads_keys();
            self.running = None;
        }
        (self.running.get_or_insert_with(|| Running::new(call_id)), replaced)
    }
}

/// The row of the files that a call changed as one plain note, for a call whose block
/// is not shown; `None` when it changed none.
fn changes_note(changes: &FileChanges) -> Option<String> {
    let row: String =
        format::changes::call_row(changes).into_iter().map(|(text, _)| text).collect();
    (!row.is_empty()).then_some(row)
}
