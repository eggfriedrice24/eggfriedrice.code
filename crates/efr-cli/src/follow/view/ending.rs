//! The start and the end of a turn in the view: `turn_started`, the end events and
//! `conversation_compacted`, the lines before the end, and the end itself.
//! The turn slice of the terminal look owns this file: an `impl TurnView` block next to
//! `view.rs`.

use efr_protocol::{Event, FileChanges, Mode, Scope, TurnUndoInfo};
use efr_render::RenderOptions;

use super::input::{Input, user_message};
use super::reasoning::Reasoning;
use super::{FULL_STARTED, RESENT, Running, Step, TurnEnd, TurnView, rows};
use crate::format::changes::HomeChange;
use crate::format::context::{self as gauge, GaugedLine};
use crate::format::{self, Block, Tone, sandbox};
use crate::progress;
use crate::terminal::Size;

impl TurnView {
    /// The start of the turn: the status row starts, a queued prompt shows as the
    /// user's message, a call or a question of the turn ahead is over, and the fallback
    /// note and the full-mode line show.
    pub(super) fn turn_started(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnStarted { scope, settings, .. } = event else {
            return Step::default();
        };
        self.queued = false;
        self.started_at = self.event_at;
        self.reasoning = Reasoning::default();
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

    /// The end of a completed turn: why the model stopped, the files it changed and the
    /// end-of-turn line.
    pub(super) fn turn_completed(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnCompleted { usage, changes, context, stop, undo, .. } = event else {
            return Step::default();
        };
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

    /// The end of a failed turn: the error is the command's, or a note when a queued
    /// prompt runs next.
    pub(super) fn turn_failed(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnFailed { error, changes, undo, .. } = event else {
            return Step::default();
        };
        self.ended_with(changes.as_ref(), undo.as_ref());
        self.end(TurnEnd::Failed(error.clone()), &[], size)
    }

    /// The end of an interrupted turn: its time and its gauge.
    pub(super) fn turn_interrupted(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnInterrupted { context, changes, undo, .. } = event else {
            return Step::default();
        };
        self.ended_with(changes.as_ref(), undo.as_ref());
        let line = if self.summary() {
            format::turn_interrupted(self.took(), context.as_ref())
        } else {
            GaugedLine::from("interrupted".to_owned())
        };
        self.end(TurnEnd::Interrupted, &[line], size)
    }

    /// The end of a turn that never ran because the daemon restarted.
    pub(super) fn turn_cancelled(&mut self, size: Size) -> Step {
        self.end(TurnEnd::Cancelled, &[], size)
    }

    /// A compaction of the conversation: the gauge goes down and one muted line says what
    /// it kept.
    pub(super) fn compacted(&mut self, event: &Event, size: Size) -> Step {
        let Event::ConversationCompacted(compaction) = event else {
            return Step::default();
        };
        self.context(gauge::after(compaction));
        let before = self.finish_message();
        let rows = |options: &RenderOptions| gauge::compacted_rows(compaction, options);
        self.rendered_after(before, rows, size)
    }

    pub(super) fn end(&mut self, end: TurnEnd, notes: &[GaugedLine], size: Size) -> Step {
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

    /// The whole lines before the end of the turn: one per file outside the roots that
    /// the turn changed and that has a line of its own, then, for a turn that `efr
    /// undo` can take back, its line and what no snapshot covers. A file that efr could
    /// not copy takes its reason from the names of what no snapshot covers, so the
    /// reason shows once.
    pub(super) fn end_lines(&mut self) -> Vec<String> {
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

    /// Keeps what the turn's end event says it changed and what `efr undo` can take
    /// back, for the lines before the end; true when undo can take the turn back now.
    pub(super) fn ended_with(
        &mut self,
        changes: Option<&FileChanges>,
        undo: Option<&TurnUndoInfo>,
    ) -> bool {
        self.ended_undo = undo.map(|undo| Box::new((changes.cloned(), undo.clone())));
        undo.is_some_and(|undo| undo.undoable)
    }
}

/// What the lines at the end of a turn name, gathered while it runs.
#[derive(Debug, Default)]
pub(super) struct Ending {
    /// The files outside the project that the turn changed (`home_file_changed`), one
    /// entry for each path.
    pub(super) home_changes: Vec<HomeChange>,
    /// What full mode let through in this turn (`exit_judged` with the judge `full`),
    /// each action once.
    pub(super) full_exits: Vec<String>,
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
