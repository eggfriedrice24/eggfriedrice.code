//! The questions of a turn in the view: an approval and its card, the answer line that
//! takes its place, the reviewer's verdicts, the always rules, and the quarantine, stop
//! and move questions. The live slice of the terminal look owns this file: an `impl
//! TurnView` block next to `view.rs`.

use efr_protocol::{
    ApprovalDecision, CallId, ExitKind, JobId, JobInfo, Launch, Origin, QuestionId, Risk,
    SurfaceChange, Verdict,
};
use jiff::Timestamp;

use super::status::State;
use super::{About, Ask, BLOCKING, GOES_ON, Preview, Request, Shown, Step, TurnView, rows};
use crate::format::card::{self, Card, Footer};
use crate::format::{self, Block, Tone, jobs, sandbox};
use crate::live::{effective_width, fits, rows_of};
use crate::terminal::Size;

impl TurnView {
    pub(super) fn approval(
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
        // A question that full mode still asks has the line that says why first.
        let wake = card::strip_wake(request.summary);
        let guarded = format::full_guard(wake.unwrap_or(request.summary)).is_some();
        // NOTE: an exit shows the whole line from its record, never a shortened summary.
        if let Some(exit) = request.exit {
            let record = self.exits.get(&call_id).map(|exit| &exit.record);
            let reviewed = self.reviewed.get(&call_id).map(String::as_str);
            // A question that full mode still asks keeps the line that says why first.
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
            let glyphs = self.options.glyphs();
            card =
                sandbox::exit_card(exit, record, self.home.as_deref(), subject, reviewed, glyphs);
            if let Some(guard) = guard {
                card.rows.insert(0, card::guard_row(guard));
            }
            if wake.is_some() {
                card::mark_wake(&mut card);
            }
        }
        // NOTE: a question with no exit and no `why` row of its summary says why the
        // mode asks in the daemon's words, after what runs (TM-Questions 6, TM-Live 20:
        // `why` in a plain approval).
        if request.exit.is_none()
            && !card.rows.iter().any(|row| row.value_of(card::WHY).is_some())
            && let Some(reason) = request.reason.map(format::one_line)
            && !reason.trim().is_empty()
        {
            let at = card.rows.iter().rposition(|row| matches!(row, card::Row::Command(_)));
            let at = at.map_or(card.rows.len().min(1), |at| at + 1);
            let reason = reason.trim().trim_end_matches('.').to_owned();
            card.rows.insert(at, card::Row::labelled(card::WHY, reason));
        }
        // The why row names a path below the home directory with `~`, as the rows of
        // the calls do. The line of the answer takes its subject from this row.
        if let Some(home) = self.home.as_deref() {
            for row in &mut card.rows {
                if let Some(value) = row.value_of(card::WHY) {
                    let short = sandbox::tilde_in(&value, home);
                    if short != value {
                        *row = card::Row::labelled(card::WHY, short);
                    }
                }
            }
        }
        // NOTE: a call that may ask for a password says so, but never one in the sandbox,
        // where efr types no secret, and a privilege exit says it in its why row.
        let privilege = request.exit.is_some_and(|exit| exit.kinds.contains(&ExitKind::Privilege));
        let contained =
            request.exit.is_some_and(|exit| matches!(exit.launch, Launch::Contained { .. }));
        if self.interactive.contains(&call_id) && !privilege && !contained {
            card.rows.push(card::Row::text(sandbox::PASSWORD_ROW, Tone::Dim));
        }
        // NOTE: a patch shows the diff of each of its files after a row that names it,
        // so a delete or a move stands out before the user answers. An edit is a patch
        // of one file.
        let diff = request.diff.map(|diff| {
            format::patch::short_paths(diff, self.kept.cwd.as_deref(), self.home.as_deref())
        });
        let diff_rows = match (diff.as_deref(), tool) {
            (Some(diff), Some(format::patch::TOOL | format::EDIT_TOOL)) => card::patch_rows(diff),
            (Some(diff), _) => vec![card::Row::Diff(diff.to_owned())],
            (None, _) => Vec::new(),
        };
        // NOTE: the diff of a file tool's write names the file with its counts, so it
        // takes the place of the row that names the file, right under the title and the
        // line of full mode; why it asks follows it (TM-Questions 2).
        let file_tool =
            matches!(tool, Some("write_file" | format::patch::TOOL | format::EDIT_TOOL));
        if file_tool && !diff_rows.is_empty() {
            let first = usize::from(guarded);
            if card.rows.len() > first {
                card.rows.remove(first);
            }
            let at = first.min(card.rows.len());
            card.rows.splice(at..at, diff_rows);
        } else {
            card.rows.extend(diff_rows);
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

    /// Shows `card`, the question about `about`, with `footer`, after `step`'s output:
    /// in the live zone on a terminal until it is answered, on stderr otherwise.
    pub(super) fn show_question(
        &mut self,
        mut step: Step,
        card: Card,
        about: About,
        footer: Footer,
        size: Size,
    ) -> Step {
        // NOTE: a question of the turn takes the keys from the question of the stop key
        // or of the background key, which then waits for no key.
        if about != About::StopJob {
            self.jobs.stopping = None;
        }
        if about != About::Move {
            self.jobs.moving = None;
        }
        self.kept.asked = Some((about, card.subject(), self.event_at));
        if self.terminal() {
            let before = std::mem::take(&mut step.out);
            self.stage(&before);
            // One card at a time: one that still waits goes to the scrollback whole.
            self.commit_question();
            self.question = Some(Shown::new(card, about, footer));
            self.dirty = true;
        } else {
            let mut text = self.err_spacing.before(Block::Question).to_owned();
            text.push_str(&card.render(Some(&footer), &self.options_at(size)));
            step.err = self.raw_err(text);
        }
        step
    }

    /// Writes the card of the question to the scrollback when the live zone with it
    /// would not fit on the screen: rows that scroll off the top can never be erased,
    /// and the live zone would show only the card's last rows. Its keys stay live.
    pub(super) fn fit_question(&mut self, size: Size, now: Timestamp) {
        if !self.question.as_ref().is_some_and(|shown| !shown.committed) {
            return;
        }
        let body = self.live_body(size, now);
        // One row more for a status row under it.
        let rows = rows_of(&body, effective_width(size), self.options.width_method()) + 1;
        if !fits(rows, size) {
            self.commit_question();
        }
    }

    /// Writes the card of the question on the screen to the scrollback once, without
    /// its keys, which stay in the live zone while the question waits.
    pub(super) fn commit_question(&mut self) {
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

    /// An `a` answer to the question about `about`: on a terminal the card goes, and its
    /// line `line` waits for the line of what the rule became, so the two make one line,
    /// such as `✓ allowed always · saved as sandbox.allow[2]: network index.crates.io for
    /// cargo` (TM-Questions 7). `None` elsewhere, where the line goes at once.
    pub(super) fn hold_answer(&mut self, about: About, line: &str) -> Option<Step> {
        let About::Approval(call) = about else {
            return None;
        };
        if !self.terminal() {
            return None;
        }
        if self.question.as_ref().is_some_and(|shown| shown.about == about) {
            self.question = None;
            self.dirty = true;
        }
        self.kept.held = Some((call, line.to_owned()));
        Some(Step::default())
    }

    /// The held line of an `a` answer, with `saved` after it when the line of what the
    /// rule became came, as one line; `None` when no line waits.
    pub(super) fn release_answer(&mut self, saved: Option<&str>, size: Size) -> Option<Step> {
        let (call, line) = self.kept.held.take()?;
        let line = match saved {
            Some(saved) => format!("{line}{}{saved}", options_separator(self)),
            None => line,
        };
        Some(self.answer_line(About::Approval(call), true, &line, size))
    }

    /// What the question about `about` on the screen asked about, such as `network
    /// (github.com)`, for the line that takes its place.
    pub(super) fn asked_about(&self, about: About) -> Option<String> {
        let (asked, subject, _) = self.kept.asked.as_ref()?;
        if *asked == about { subject.clone() } else { None }
    }

    /// The question about `about` is settled: on a terminal its card gives its place to
    /// one line, such as `✓ allowed · network (github.com)` (`good`) or `✗ denied ·
    /// network (github.com)`: the mark in the `success` or the `error` role, the rest
    /// muted (TM-Questions 7). A call of this turn follows the line at once; anything
    /// else comes after a blank line.
    pub(super) fn answer_line(&mut self, about: About, good: bool, line: &str, size: Size) -> Step {
        if self.question.as_ref().is_some_and(|shown| shown.about == about) {
            self.question = None;
            self.dirty = true;
        }
        let text = answer_text(good, line, &self.options_at(size));
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

    pub(super) fn resolved(
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
        let separator = options_separator(self);
        let line =
            format!("{}{separator}from {}", format::decision(decision), format::origin(origin));
        let good = format::allows(decision);
        if decision == ApprovalDecision::AllowAlways {
            self.saving = Some(call_id);
            if let Some(step) = self.hold_answer(About::Approval(call_id), &line) {
                return Step { settled, ..step };
            }
        }
        Step { settled, ..self.answer_line(About::Approval(call_id), good, &line, size) }
    }

    /// The reviewer judged the exit of call `call_id`. An allow or a refusal is one muted
    /// line, which the call's rows follow at once, as they follow an answer; an unsure
    /// verdict shows on the card of the question that comes next. A verdict without a
    /// risk is one that no reviewer model gave: every step of the chain failed, which
    /// the card says with the steps in `skipped`.
    pub(super) fn judged(
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
    pub(super) fn covered(&mut self, call_id: CallId, line: &str, size: Size) -> Step {
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
    pub(super) fn verdict_line(&mut self, call: Option<CallId>, line: &str, size: Size) -> Step {
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
    pub(super) fn expired(&mut self, call_id: CallId, away: bool, size: Size) -> Step {
        self.refused.insert(call_id);
        let settled = self.settle(call_id);
        let separator = options_separator(self);
        // NOTE: the event names no time; the card knows when it showed, on the daemon's
        // clock.
        let waited = self
            .kept
            .asked
            .as_ref()
            .filter(|(asked, _, _)| *asked == About::Approval(call_id))
            .and_then(|(_, _, at)| *at)
            .zip(self.event_at)
            .and_then(|(asked, now)| std::time::Duration::try_from(now.duration_since(asked)).ok());
        let line = match (away, waited) {
            (true, _) => format!("{EXPIRED_AWAY}{separator}{NOT_RUN}"),
            (false, Some(waited)) => {
                format!("{EXPIRED}{separator}nobody answered in {}; {NOT_RUN}", minutes(waited))
            }
            (false, None) => format!("{EXPIRED}{separator}nobody answered; {NOT_RUN}"),
        };
        let step = self.answer_line(About::Approval(call_id), false, &line, size);
        Step { settled, ..step }
    }

    /// The quarantine question `question_id` about `changes`: a git setting that runs
    /// programs, which the last call changed and the launcher moved to quarantine. Like
    /// an approval it waits below the live zone for one key, but no call waits for it.
    pub(super) fn surface_question(
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
    pub(super) fn surface_resolved(
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

    /// The question of the background key about `call_id`, which runs outside the
    /// sandbox: efrd's preview names its `command`, the time limit of the job in
    /// minutes and the exits of the job decision. `y` moves the call with `token`.
    pub(crate) fn move_question(
        &mut self,
        call_id: CallId,
        preview: &Preview<'_>,
        size: Size,
    ) -> Step {
        let before = self.finish_message();
        let mut rows = card::command_rows(preview.command);
        rows.push(card::Row::text(jobs::move_risk(preview.max_minutes), Tone::Attention));
        for exit in preview.exits {
            rows.push(card::Row::text(jobs::move_exit(exit, self.home.as_deref()), Tone::Dim));
        }
        let card = Card { title: jobs::MOVE_TITLE.to_owned(), rows };
        let ask = Ask::Move { call_id, token: preview.token };
        let step = Step { out: before, ask: Some(ask), ..Step::default() };
        self.jobs.moving = Some(call_id);
        // NOTE: the call keeps what it holds the keys for (a call allowed here that may
        // ask, or one that asked for a password): they go back to it after the answer
        // ([`keys_back`](Self::keys_back)).
        self.show_question(step, card, About::Move, Footer::Keys(jobs::MOVE_KEYS), size)
    }

    /// The user answered the question of the background key: `yes` with `y`. The call
    /// to move comes back with a `y`, while the question still asks about it. The keys
    /// go back to the call when it held them before the question.
    pub(crate) fn move_answered(&mut self, yes: bool, size: Size) -> (Step, Option<CallId>) {
        let Some(call_id) = self.jobs.moving.take() else {
            return (Step::default(), None);
        };
        let line =
            if yes { "moving the command to the background" } else { "the command runs on here" };
        let step = self.answer_line(About::Move, yes, line, size);
        (self.keys_back(call_id, step), yes.then_some(call_id))
    }

    /// `step`, after which the keys go back to call `call_id` when it holds them: kept
    /// for a call that the user allowed here as one that may wait for input, or for an
    /// output line that looks like a password prompt ([`Ask::Retain`]), or thrown away
    /// after it asked for a password ([`Ask::Discard`]). Otherwise they go to the input
    /// row.
    pub(super) fn keys_back(&self, call_id: CallId, step: Step) -> Step {
        let running = self.running.as_ref().filter(|running| running.call_id == call_id);
        let ask = if running.is_some_and(|running| running.guarding) {
            Some(Ask::Discard(call_id))
        } else if self.retained == Some(call_id) || running.is_some_and(|running| running.held) {
            Some(Ask::Retain(call_id))
        } else {
            None
        };
        Step { ask: ask.or(step.ask), ..step }
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
        let mut line = format::decision(decision).to_owned();
        if decision == ApprovalDecision::AllowAlways {
            self.saving = Some(call_id);
        }
        let held = (decision == ApprovalDecision::AllowAlways)
            .then(|| self.hold_answer(About::Approval(call_id), &line))
            .flatten();
        if let Some(subject) = self.asked_about(About::Approval(call_id)) {
            line = format!("{line}{}{subject}", options_separator(self));
        }
        let step = match held {
            Some(step) => step,
            None => self.answer_line(About::Approval(call_id), good, &line, size),
        };
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
    pub(super) fn set_footer(&mut self, call_id: CallId, footer: Footer, size: Size) -> Step {
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
}

/// What the line of an approval that expired starts with.
const EXPIRED: &str = "the approval expired";

/// What the line of an approval that expired while the user was away starts with.
const EXPIRED_AWAY: &str = "expired while you were away";

/// What the line of an approval that expired ends with.
const NOT_RUN: &str = "the call did not run";

/// ` · `, between the parts of an answer line.
fn options_separator(view: &TurnView) -> String {
    format!(" {} ", view.options.glyphs().separator)
}

/// `time` as the line of an expired approval says it: whole minutes from a minute on,
/// such as `10 min`, else seconds.
fn minutes(time: std::time::Duration) -> String {
    let secs = time.as_secs();
    if secs >= 60 { format!("{} min", secs / 60) } else { format::elapsed(time) }
}

/// The line that takes the place of a question: the mark of `good` or not in the
/// `success` or the `error` role, a space, and `line` muted, with its newline; a row
/// wider than the screen goes on in the next one.
pub(super) fn answer_text(good: bool, line: &str, options: &efr_render::RenderOptions) -> String {
    let glyphs = options.glyphs();
    let (mark, tone) =
        if good { (glyphs.ok, Tone::Success) } else { (glyphs.failed, Tone::Failure) };
    let pieces = [
        (mark.to_owned(), tone),
        (" ".to_owned(), Tone::Plain),
        (format::one_line(line), Tone::Dim),
    ];
    piece_rows(&pieces, options)
}

/// `pieces` as rows with their newlines: on a terminal, a row wider than the screen goes
/// on in the next one, indented [`GOES_ON`] columns more, as `view::rows` does.
fn piece_rows(pieces: &[(String, Tone)], options: &efr_render::RenderOptions) -> String {
    let rows = match format::columns(options) {
        Some((width, method)) => {
            format::wrap_spans(pieces, width.max(1), width.saturating_sub(GOES_ON).max(1), method)
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
