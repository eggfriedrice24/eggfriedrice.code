//! Web search in the view (TM-Web, `DECISIONS-web.md` 3): the provider runs the search
//! inside the model call, and the turn records its steps. An `impl TurnView` block next
//! to `view.rs`; the rows are `format::web`'s.
//!
//! - A step adds no row to the live zone: only the status row shows it, `searching`
//!   (`opening` for a page) with the newest query or page as its detail and `+2` when
//!   more run ([`State::Web`]).
//! - A step that ended writes its row once, when every step that started before it has
//!   its row (`format::web::Order`), so the scrollback reads in the order of the model.
//!   A web row never folds, and a find never gets one.
//! - The report of the turn (`turn_web_report`) brings the list of the sources after the
//!   last answer, one blank row before it, and the count of the searches, which the end
//!   line of the turn shows last on its cost; the first turn of a conversation that read
//!   the provider's stored copy of the web gets one muted row under its end line.
//! - When stdout is not a terminal, the rows go to stderr in the same order, plain, as
//!   the rows of the calls do.

use efr_protocol::{CallId, Event};
use jiff::Timestamp;

use super::status::State;
use super::{Step, TurnView};
use crate::format::route::Pieces;
use crate::format::web::{self, End, Ended, Fit, Order};
use crate::format::{Block, Tone};
use crate::terminal::Size;

/// What the view keeps of the web steps of the followed turn.
#[derive(Debug, Default)]
pub(super) struct Web {
    /// The steps that run, in the order of their starts, with when each started on the
    /// daemon's clock: the status row names the newest.
    running: Vec<(CallId, web::Step, Option<Timestamp>)>,
    /// The rows that wait for the rows of the steps that started before them.
    order: Order,
    /// What the report of the turn said: the count of its searches, whether they read
    /// the stored copy, and whether its end line says once what that means.
    report: Option<Report>,
}

/// The part of `turn_web_report` that the end of the turn shows.
#[derive(Debug, Clone, Copy)]
struct Report {
    searches: u32,
    stored_copy: bool,
    note: bool,
}

impl TurnView {
    /// A step of the web tool started: the message before it is complete, and the status
    /// row says what runs. No row shows yet.
    pub(super) fn web_call_started(&mut self, event: &Event) -> Step {
        let Some((call_id, step)) = web::Step::of(event) else {
            return Step::default();
        };
        let before = self.finish_message();
        self.kept.web.order.start(call_id, &step);
        if !self.kept.web.running.iter().any(|(id, ..)| *id == call_id) {
            self.kept.web.running.push((call_id, step, self.event_at));
        }
        self.web_state();
        self.commit(before)
    }

    /// A step of the web tool ended: its row, and the rows that waited for it, are
    /// written in the order of their starts.
    pub(super) fn web_call_completed(&mut self, event: &Event, size: Size) -> Step {
        let (Some((call_id, step)), Some(end)) = (web::Step::of(event), End::of(event)) else {
            return Step::default();
        };
        let at = self.kept.web.running.iter().position(|(id, ..)| *id == call_id);
        let (step, since) = match at.map(|at| self.kept.web.running.remove(at)) {
            Some((_, started, since)) => (started.with(step), since),
            None => (step, None),
        };
        let took =
            since.zip(self.event_at).map(|(since, end)| crate::follow::since_then(since, end));
        let before = self.finish_message();
        let ready = self.kept.web.order.end(Ended { call_id, step, end, took });
        self.web_state();
        let step = self.commit(before);
        let rows = self.web_rows(&ready, size);
        Step {
            err: format!("{}{}", step.err, rows.err),
            out: format!("{}{}", step.out, rows.out),
            ..step
        }
    }

    /// What the turn did on the web, in the batch of its end: the list of sources after
    /// the last answer, and the count and the note for the end line.
    pub(super) fn web_report(&mut self, event: &Event, size: Size) -> Step {
        let Event::TurnWebReport { searches, stored_copy, stored_copy_note, sources, .. } = event
        else {
            return Step::default();
        };
        self.kept.web.report = Some(Report {
            searches: *searches,
            stored_copy: *stored_copy,
            note: *stored_copy_note && *stored_copy,
        });
        let mut step = self.web_flush(size);
        if sources.is_empty() {
            return step;
        }
        let before = self.finish_message();
        let options = self.options_at(size);
        let list = web::sources(sources, &options);
        if !self.terminal() {
            step.out.push_str(&before);
            let text = format!("{}{list}", self.err_spacing.before(Block::Message));
            step.err.push_str(&self.raw_err(text));
            return step;
        }
        // NOTE: the list reads as part of the answer: one blank row before it, as before
        // prose, and one after it before the end lines (TM-Web 10).
        let mut committed = before;
        committed.push_str(self.spacing.before(Block::Message));
        committed.push_str(&list);
        self.stage(&committed);
        step
    }

    /// Writes the rows of the steps that ended and still wait, in order: the turn
    /// ends, and nothing comes after them.
    pub(super) fn web_flush(&mut self, size: Size) -> Step {
        let rest = self.kept.web.order.rest();
        self.kept.web.running.clear();
        self.web_rows(&rest, size)
    }

    /// `line`, the outcome line of the end of the turn, with the count of its searches
    /// last on its cost; and the muted row under it of the first turn of a conversation
    /// that read the stored copy of the web.
    pub(super) fn web_end(&mut self, line: Option<&mut Pieces>) -> Option<Pieces> {
        let report = self.kept.web.report.take()?;
        let line = line?;
        let glyphs = self.options.glyphs();
        let count = web::count(report.searches, report.stored_copy)?;
        web::add_count(line, &count, glyphs);
        report.note.then(|| vec![(web::stored_copy_note(glyphs), Tone::Dim)])
    }

    /// The status row names the newest step that runs, or goes back to the model when
    /// none runs.
    fn web_state(&mut self) {
        let shown = match self.kept.web.running.last() {
            Some((_, step, _)) => Some(State::Web {
                opening: step.opens(),
                detail: step.detail(),
                more: self.kept.web.running.len() - 1,
            }),
            None => None,
        };
        match shown {
            Some(state) => self.state(state),
            None => {
                if self
                    .status
                    .as_ref()
                    .is_some_and(|status| matches!(status.state(), State::Web { .. }))
                {
                    self.state(State::Model);
                }
            }
        }
    }

    /// The rows of `ended`, written once as one block of rows: to the scrollback on a
    /// terminal, otherwise to stderr.
    fn web_rows(&mut self, ended: &[Ended], size: Size) -> Step {
        let options = self.options_at(size);
        let block: String = ended.iter().map(|ended| web::row(ended, Fit::Cut, &options)).collect();
        if block.is_empty() {
            return Step::default();
        }
        if !self.terminal() {
            let text = format!("{}{block}", self.err_spacing.before(Block::Row));
            return Step { err: self.raw_err(text), ..Step::default() };
        }
        let mut committed = self.spacing.before(Block::Row).to_owned();
        committed.push_str(&block);
        self.stage(&committed);
        Step::default()
    }
}
