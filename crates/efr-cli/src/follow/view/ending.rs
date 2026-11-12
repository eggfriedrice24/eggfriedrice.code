//! The frame of a turn in the view (`DECISIONS.md` "The frame of a turn", TM-Endings,
//! TM-Scope 19): the event lines at its start, the notices inside it, and the lines of
//! its end. The turn slice of the terminal look owns this file: an `impl TurnView` block
//! next to `view.rs`.
//!
//! A reply has no first line. A line comes only when something changed: `↪` when the
//! terminal's conversation changed (from the `prompt.send` result, [`TurnView::event_row`]),
//! then `⇄` when the turn runs on another model, provider or account
//! (`model_switched`). The `⇄` line waits in the live zone until the next block: when a
//! compaction of the turn comes first, the line carries its counts and the compaction
//! has no notice of its own (spec 3.4).
//!
//! The end: one blank line, then the stop reason, the changes line or the line of
//! `efr undo`, the lines of the files at home, and the outcome line, with no blank line
//! between them. Under a failure, 2 columns in, the reason and its fix; under an
//! interrupt, what goes again.

use efr_protocol::{
    EffectiveSettings, Event, FileChanges, Mode, Scope, SwitchedModel, TurnUndoInfo,
};
use efr_render::RenderOptions;

use super::input::{Input, user_message};
use super::reasoning::Reasoning;
use super::{Running, Step, TurnEnd, TurnView, WITHDRAWN};
use crate::error::turn_failure;
use crate::format::changes::HomeChange;
use crate::format::context::{self as gauge};
use crate::format::ending::{self as end_lines, UNDER};
use crate::format::route::{self, Pieces};
use crate::format::{self, Block, Tone, sandbox};
use crate::progress;
use crate::terminal::Size;

impl TurnView {
    /// The start of the turn: the status row starts, a queued prompt shows as the
    /// user's message, a call or a question of the turn ahead is over, and the fallback
    /// note shows. Full mode has no line here: `,mode full` said it once, and the
    /// status row keeps `full` (`DECISIONS.md`).
    pub(super) fn turn_started(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnStarted { scope, settings, cwd, .. } = event else {
            return Step::default();
        };
        self.queued = false;
        self.cwd = Some(cwd.clone());
        self.started_at = self.event_at;
        self.reasoning = Reasoning::default();
        let full = settings.as_ref().is_some_and(|settings| settings.mode == Mode::Full);
        self.ending.full_exits.clear();
        self.ending.switch = None;
        if settings.is_some() {
            self.ending.settings.clone_from(settings);
        }
        if let Some(status) = &mut self.status {
            status.turn_started();
            status.set_full(full);
            // NOTE: the model of a turn that waited behind another one shows from its
            // start (the live slice).
            if let Some(settings) = settings {
                status.set_model(Some(super::status::Model::of(settings)));
            }
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
            let noted = self.row(&[(note, Tone::Dim)], size);
            step.out.push_str(&noted.out);
            step.err.push_str(&noted.err);
        }
        step
    }

    /// The settings that the `prompt.send` result names for the followed turn, before
    /// its `turn_started` comes: the status row can name the model from its first
    /// frame (TM-Sequence 1).
    pub(crate) fn with_settings(mut self, settings: Option<EffectiveSettings>) -> TurnView {
        self.ending.settings = settings;
        self
    }

    /// The settings of the followed turn: from its `turn_started`, else from the
    /// `prompt.send` result.
    // NOTE: the live slice of the terminal look names the model in the status row.
    #[cfg_attr(not(test), expect(dead_code, reason = "the status row names the model"))]
    pub(super) fn turn_settings(&self) -> Option<&EffectiveSettings> {
        self.ending.settings.as_ref()
    }

    /// An event line of efr's own, such as the `↪` line of a change of conversation or
    /// the repo hint: one row, wrapped at the screen's width.
    pub(crate) fn event_row(&mut self, pieces: &[(String, Tone)], size: Size) -> Step {
        self.row(pieces, size)
    }

    /// One row of the design (`Block::Row`): on a terminal to the scrollback, otherwise
    /// to stderr.
    fn row(&mut self, pieces: &[(String, Tone)], size: Size) -> Step {
        let pieces = pieces.to_vec();
        self.rendered_as(Block::Row, |options| end_lines::rows(&pieces, 0, options), size)
    }

    /// Text that `render` makes at the screen's width, as a block of `kind`: on a
    /// terminal to the scrollback, otherwise to stderr.
    fn rendered_as(
        &mut self,
        kind: Block,
        render: impl FnOnce(&RenderOptions) -> String,
        size: Size,
    ) -> Step {
        let options = self.options_at(size);
        if !self.terminal() {
            let text = format!("{}{}", self.err_spacing.before(kind), render(&options));
            let err = self.raw_err(text);
            return Step { err, ..Step::default() };
        }
        let mut committed = self.spacing.before(kind).to_owned();
        committed.push_str(&render(&options));
        self.stage(&committed);
        Step::default()
    }

    /// The turn runs on another model, provider or account than the turn before
    /// (`model_switched`, TM-Endings 5 to 7): the `⇄` line. On a terminal it waits in the
    /// live zone until the next block, so a compaction that comes first stands on it.
    pub(super) fn model_switched(&mut self, event: &Event, size: Size) -> Step {
        let Event::ModelSwitched { from, to, .. } = event else {
            return Step::default();
        };
        let mut to = to.clone();
        // NOTE: a daemon before the effort of a switch: the effort of the turn's own
        // settings, from its `turn_started` in the same batch.
        if to.effort.is_none() {
            to.effort = self.ending.settings.as_ref().and_then(|settings| {
                settings.effort.clone().or_else(|| settings.default_effort.clone())
            });
        }
        // NOTE: the status row names the model that the turn runs on now.
        let model = super::status::Model::switched_to(&to);
        if let Some(status) = &mut self.status {
            status.set_model(Some(model.clone()));
        }
        self.kept.model = Some(model);
        if !self.terminal() {
            let line = route::model_line(from, &to, None, self.options.glyphs());
            return self.row(&line, size);
        }
        // NOTE: the line takes its place now, after the `↪` line, so what comes after it
        // has its blank line as after a row.
        let separator = self.spacing.before(Block::Row);
        self.ending.switch = Some(Box::new(Switch { from: from.clone(), to, separator }));
        self.dirty = true;
        Step::default()
    }

    /// The `⇄` line that waits in the live zone, with its blank line; empty when none
    /// waits.
    pub(super) fn switch_live(&self, options: &RenderOptions) -> String {
        match &self.ending.switch {
            Some(switch) => switch.render(None, options),
            None => String::new(),
        }
    }

    /// Commits the `⇄` line that waits, before any other block: it goes to the text
    /// that the next frame writes.
    pub(super) fn release_switch(&mut self) {
        if let Some(switch) = self.ending.switch.take() {
            let options = self.options_now();
            self.pending.push_str(&switch.render(None, &options));
            self.dirty = true;
        }
    }

    /// The end of a completed turn: why the model stopped, the files it changed and the
    /// outcome line.
    pub(super) fn turn_completed(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnCompleted { usage, changes, context, stop, undo, .. } = event else {
            return Step::default();
        };
        let glyphs = self.options.glyphs();
        // NOTE: why the model stopped shows also without the outcome line: a turn with
        // no answer must not end in silence.
        let mut before: Vec<Pieces> =
            stop.iter().map(|stop| end_lines::stop_line(stop, glyphs)).collect();
        // NOTE: the files that the turn changed stand right before its end, also
        // without the outcome line: they say what the turn left behind. A turn that
        // undo can take back says so in its own line instead.
        let undoable = self.ended_with(changes.as_ref(), undo.as_ref());
        let changed = changes
            .as_ref()
            .filter(|_| !undoable)
            .map(|changes| format::changes::turn_line(changes, &self.options_now()))
            .filter(|pieces| !pieces.is_empty());
        // NOTE: the changes line comes as pieces (TM-Endings 15: the count in `text`,
        // the counts in colour, the names muted).
        before.extend(changed);
        let outcome = self
            .summary()
            .then(|| end_lines::done_line(self.took(), usage.as_ref(), context.as_ref(), glyphs));
        self.end(TurnEnd::Completed, before, outcome, Vec::new(), size)
    }

    /// The end of a failed turn (TM-Endings 21, 22): `✗ failed` with its time, then the
    /// reason and what fixes it. The command then ends with no message of its own.
    pub(super) fn turn_failed(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnFailed { error, changes, undo, .. } = event else {
            return Step::default();
        };
        self.ended_with(changes.as_ref(), undo.as_ref());
        let took = self.took().filter(|_| self.summary());
        let outcome = end_lines::failed_line(took, self.options.glyphs());
        // NOTE: a login fix names the login of the provider that ran the turn.
        let provider = self
            .ending
            .settings
            .as_ref()
            .and_then(|settings| settings.provider.clone())
            .unwrap_or_else(|| efr_config::DEFAULT_PROVIDER.to_owned());
        let (reason, fix) = turn_failure(error, &provider);
        let under = end_lines::failure_rows(&reason, fix.as_ref());
        self.end(TurnEnd::Failed(error.clone()), Vec::new(), Some(outcome), under, size)
    }

    /// The end of an interrupted turn (TM-Endings 20): `■ interrupted` with its time
    /// and its gauge.
    pub(super) fn turn_interrupted(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnInterrupted { context, changes, undo, .. } = event else {
            return Step::default();
        };
        self.ended_with(changes.as_ref(), undo.as_ref());
        let glyphs = self.options.glyphs();
        let line = if self.summary() {
            end_lines::interrupted_line(self.took(), context.as_ref(), glyphs)
        } else {
            end_lines::interrupted_line(None, None, glyphs)
        };
        self.end(TurnEnd::Interrupted, Vec::new(), Some(line), Vec::new(), size)
    }

    /// The end of a turn that never ran because the daemon restarted.
    pub(super) fn turn_cancelled(&mut self, size: Size) -> Step {
        self.end(TurnEnd::Cancelled, Vec::new(), None, Vec::new(), size)
    }

    /// The end of the followed prompt, taken back before it ran.
    pub(super) fn withdrawn_end(&mut self, size: Size) -> Step {
        let line = vec![(WITHDRAWN.to_owned(), Tone::Dim)];
        self.end(TurnEnd::Withdrawn, vec![line], None, Vec::new(), size)
    }

    /// A compaction of the conversation: the gauge goes down and one muted line says what
    /// it kept (TM-Endings 10). A `⇄` line that waits takes the counts instead
    /// (TM-Endings 5): `· context compacted 86k → 6k`.
    pub(super) fn compacted(&mut self, event: &Event, size: Size) -> Step {
        let Event::ConversationCompacted(compaction) = event else {
            return Step::default();
        };
        self.context(gauge::after(compaction));
        let before = self.finish_message();
        if let Some(switch) = self.ending.switch.take() {
            let counts = Some((compaction.tokens_before, compaction.tokens_after));
            let options = self.options_at(size);
            let mut committed = before;
            committed.push_str(&switch.render(counts, &options));
            self.stage(&committed);
            return Step::default();
        }
        let rows = |options: &RenderOptions| gauge::compacted_rows(compaction, options);
        self.rendered_after(before, rows, size)
    }

    /// The end of the turn as `end`: the lines `before` the outcome line (the stop
    /// reason, the changes line), the lines of what undo can take back and of the files
    /// at home, the `outcome` line, and the rows `under` it, 2 columns in.
    pub(super) fn end(
        &mut self,
        end: TurnEnd,
        before: Vec<Pieces>,
        outcome: Option<Pieces>,
        mut under: Vec<Pieces>,
        size: Size,
    ) -> Step {
        self.release_switch();
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
        // The steers that this view's interrupt sent again run next: the row under the
        // outcome says so, after all that the interrupted turn showed.
        if std::mem::take(&mut self.resent_note) {
            let text = next.as_ref().map_or("", |next| next.text.as_str());
            under.push(end_lines::resent_row(text, self.options.glyphs().more));
        }
        // NOTE: the files outside the project that the turn changed stand before its end
        // however it ended, also after a failure, and so does what undo can take back.
        let mut lines: Vec<(usize, Pieces)> = before.into_iter().map(|line| (0, line)).collect();
        lines.extend(self.end_lines().into_iter().map(|line| (0, line)));
        lines.extend(outcome.map(|line| (0, line)));
        lines.extend(under.into_iter().map(|row| (UNDER, row)));
        // A turn that the daemon's restart cancelled says so when a queued prompt of
        // this view runs next; else it is the command's error.
        if next.is_some() && end == TurnEnd::Cancelled {
            let line = "the turn was cancelled because the daemon restarted".to_owned();
            lines.push((0, vec![(line, Tone::Dim)]));
        }
        if !lines.is_empty() {
            let rows = |options: &RenderOptions| {
                lines.iter().map(|(indent, line)| end_lines::rows(line, *indent, options)).collect()
            };
            let noted = self.rendered_as(Block::End, rows, size);
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

    /// The row under the outcome of an interrupted turn that ended before the answer of
    /// the interrupt came: its unread steers go again as a new prompt (TM-Endings 20).
    pub(super) fn resent_late(&mut self, size: Size) -> Step {
        let row = end_lines::resent_row("", self.options.glyphs().more);
        self.rendered_as(Block::End, |options| end_lines::rows(&row, UNDER, options), size)
    }

    /// The whole lines before the outcome of the turn, each muted (TM-Endings 17): for a
    /// turn that `efr undo` can take back, its line first; then one per file outside
    /// the roots that the turn changed and that has a line of its own; then what no
    /// snapshot covers and what full mode let through. A file that efr could not copy
    /// takes its reason from the names of what no snapshot covers, so the reason shows
    /// once.
    pub(super) fn end_lines(&mut self) -> Vec<Pieces> {
        let home = self.home.as_deref();
        let separator = self.options.glyphs().separator;
        let ended = self.ended_undo.take();
        let mut not_covered =
            ended.as_ref().map(|ended| ended.1.not_covered.clone()).unwrap_or_default();
        let mut lines: Vec<Pieces> = Vec::new();
        let muted = |line: String| vec![(line, Tone::Dim)];
        // NOTE: a covered file that has a line of its own (`changed ~/.zshrc · efr undo
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
        let undoable = ended.map(|ended| *ended).filter(|(_, undo)| undo.undoable);
        if let Some((changes, mut undo)) = undoable.clone() {
            undo.home_files = undo.home_files.saturating_sub(own);
            let line = format::changes::undo_line(changes.as_ref(), &undo, &self.options_now());
            lines.extend(Some(line).filter(|line| !line.is_empty()));
        }
        for change in std::mem::take(&mut self.ending.home_changes) {
            let real = change.real.as_deref();
            if let Some(backup) = change.backup.as_deref() {
                lines.push(muted(sandbox::home_changed(
                    &change.path,
                    real,
                    Some(backup),
                    home,
                    separator,
                )));
                continue;
            }
            let why = if change.covered {
                None
            } else {
                take_reason(&mut not_covered, &sandbox::tilde(&change.path, home))
            };
            lines.extend(
                sandbox::home_undo_line(
                    &change.path,
                    real,
                    change.floor,
                    change.covered,
                    why.as_deref(),
                    home,
                    separator,
                )
                .into_iter()
                .map(muted),
            );
        }
        if let Some((_, undo)) = undoable {
            lines.extend(
                format::changes::not_covered_line(&not_covered, undo.not_covered_more).map(muted),
            );
        }
        let full = sandbox::full_let_through(&std::mem::take(&mut self.ending.full_exits));
        lines.extend(full.into_iter().map(muted));
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

    /// The mark of turn `turn`, which a job notice started: once, however many notices
    /// it takes (spec 1.4: `↪ a job notice started this turn (not your input)`).
    pub(super) fn wake_mark(&mut self, turn: efr_protocol::TurnId, size: Size) -> Step {
        if !self.jobs.woken.insert(turn) {
            return Step::default();
        }
        let line = route::wake_line(self.options.glyphs());
        self.row(&line, size)
    }
}

/// What the lines at the frame of a turn name, gathered while it runs.
#[derive(Debug, Default)]
pub(super) struct Ending {
    /// The files outside the project that the turn changed (`home_file_changed`), one
    /// entry for each path.
    pub(super) home_changes: Vec<HomeChange>,
    /// What full mode let through in this turn (`exit_judged` with the judge `full`),
    /// each action once.
    pub(super) full_exits: Vec<String>,
    /// The settings of the followed turn: from the `prompt.send` result, then from its
    /// `turn_started`.
    settings: Option<EffectiveSettings>,
    /// The `⇄` line of the turn, while it waits in the live zone.
    switch: Option<Box<Switch>>,
}

/// A switch of model that waits in the live zone for the next block.
#[derive(Debug)]
struct Switch {
    from: SwitchedModel,
    to: SwitchedModel,
    /// The blank line above it, taken when the event came.
    separator: &'static str,
}

impl Switch {
    /// The line with its blank line above, with the counts of a compaction that came
    /// first.
    fn render(&self, compacted: Option<(u64, u64)>, options: &RenderOptions) -> String {
        let line = route::model_line(&self.from, &self.to, compacted, options.glyphs());
        format!("{}{}", self.separator, end_lines::rows(&line, 0, options))
    }
}

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
