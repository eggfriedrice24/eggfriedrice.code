//! Job notices in the actor (efr's auto spec, section 3.18, phase C): which turn takes
//! each notice, and when a notice starts a turn of its own (a wake turn).
//!
//! A notice never enters a running turn and is never a prompt or a steer. It waits
//! until a turn starts: the next turn that a prompt starts takes it into its first
//! message, after the prompt. When the conversation is idle, a notice starts a wake
//! turn, within these limits:
//!
//! - the job's turn is known to this actor and was no wake turn itself (a job of a
//!   wake turn never wakes, so wake turns never chain);
//! - the model did not stop the job (it read the output in the tool result);
//! - `jobs.max_wakes_per_hour` of the conversation and the daemon-wide limit
//!   (`Toolbox::wake_allowed`) allow one more.
//!
//! The wake turn runs in the mode of the newest turn that a prompt started (or of the
//! job's turn when no prompt came since), so outside `auto` its calls ask as usual.
//!
//! A notice that could not start a turn records `job_notice_held` once, with the
//! reason, and rides in the next turn. One turn takes every waiting notice, up to the
//! limits of [`wake::take_notices`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use efr_protocol::{Event, HeldReason, JobId, Mode, Origin, Seq, TurnSettings};
use efr_store::Batch;

use super::ConversationActor;
use crate::JobNoticeInput;
use crate::exit::TurnExits;
use crate::turn::{TurnEnd, TurnKind, TurnSpec};
use crate::wake::{self, WakeWindow};

/// How a job's turn ran, for the job's notices.
#[derive(Debug, Clone)]
struct JobStart {
    origin: Origin,
    settings: TurnSettings,
    mode: Mode,
    cwd: PathBuf,
    /// True when job notices started that turn.
    wake: bool,
}

/// The notices of the conversation's jobs, and what decides whether one wakes it.
#[derive(Debug, Default)]
pub(super) struct Notices {
    /// The turn of each job that a turn of this actor started.
    starts: HashMap<JobId, JobStart>,
    /// The notices that no turn took yet, oldest first.
    waiting: Vec<JobNoticeInput>,
    /// The notices of `waiting` that recorded `job_notice_held`, by their source.
    held: HashSet<Seq>,
    /// The wake turns of the last hour.
    window: WakeWindow,
    /// The mode of the newest turn that a prompt started.
    newest_prompt: Option<Mode>,
    /// The counts of the newest wake turn, until a prompt starts a turn.
    carried: Option<TurnExits>,
}

impl Notices {
    /// Notices whose wake turns of the last hour began at `starts`, and whose waiting
    /// notices of the events `held` recorded `job_notice_held` before a restart.
    pub(super) fn seeded(starts: Vec<jiff::Timestamp>, held: Vec<Seq>) -> Self {
        Notices {
            window: WakeWindow::new(starts),
            held: held.into_iter().collect(),
            ..Notices::default()
        }
    }

    /// Keeps `notice` until a turn takes it.
    pub(super) fn arrived(&mut self, notice: JobNoticeInput) {
        self.waiting.push(notice);
    }

    /// Gives a turn that a prompt starts the notices that wait.
    pub(super) fn take_for_prompt(&mut self, spec: &mut TurnSpec) {
        spec.notices = wake::take_notices(&mut self.waiting);
        spec.more_notices = self.waiting.len();
        self.forget_held(&spec.notices);
        // NOTE: a person started this turn: the counts of the wake turns start again.
        self.carried = None;
    }

    /// Notes what the turn of `spec` that ended with `end` tells about jobs: how it ran,
    /// for the notices of the jobs it started, and the notices it did not take.
    pub(super) fn ended(&mut self, spec: &TurnSpec, end: &mut TurnEnd) {
        let wake = spec.is_wake();
        if let Some(mode) = end.mode {
            for job in &end.jobs {
                self.starts.insert(
                    job.id.clone(),
                    JobStart {
                        origin: spec.origin,
                        settings: spec.settings.clone(),
                        mode,
                        cwd: end.cwd.clone(),
                        wake,
                    },
                );
            }
            if !wake {
                self.newest_prompt = Some(mode);
            }
        }
        self.carried = wake.then(|| end.exits.carried());
        let mut untaken = std::mem::take(&mut end.untaken);
        untaken.append(&mut self.waiting);
        self.waiting = untaken;
    }

    /// Forgets the held marks of `taken`, the notices that a turn took. A mark of a
    /// notice that has not arrived yet, as after a restart, stays.
    fn forget_held(&mut self, taken: &[JobNoticeInput]) {
        for notice in taken {
            self.held.remove(&notice.source_seq);
        }
    }

    /// Why `notice` cannot start a turn on its own, whatever the limits; `None` when
    /// it can.
    fn alone(&self, notice: &JobNoticeInput) -> Option<HeldReason> {
        if !notice.wakes {
            return Some(HeldReason::DaemonRestart);
        }
        match self.starts.get(&notice.job.id) {
            // NOTE: a job whose turn this actor did not run started before efrd
            // restarted; its notice never wakes.
            None => Some(HeldReason::DaemonRestart),
            Some(start) if start.wake => Some(HeldReason::WakeTurnJob),
            Some(_) => None,
        }
    }
}

impl ConversationActor {
    /// Starts a wake turn for the waiting notices when the conversation is idle and the
    /// limits allow it; else records `job_notice_held` once for each notice that could
    /// not start one. A turn, a compaction, an undo or a queued prompt goes first: the
    /// notices then ride in the next turn.
    pub(super) async fn wake_if_idle(&mut self) {
        if self.busy() || !self.queue.is_empty() || self.notices.waiting.is_empty() {
            return;
        }
        let mut held: Vec<(JobId, Seq, HeldReason)> = Vec::new();
        let mut wakeable: Vec<JobStart> = Vec::new();
        for notice in &self.notices.waiting {
            // NOTE: the model that stopped the job read its output in the tool result;
            // the notice rides in the next turn with no held record, which would name
            // the wrong reason.
            if self.notices.held.contains(&notice.source_seq) || notice.stopped_by_model() {
                continue;
            }
            match self.notices.alone(notice) {
                Some(reason) => held.push((notice.job.id.clone(), notice.source_seq, reason)),
                None => {
                    if let Some(start) = self.notices.starts.get(&notice.job.id) {
                        wakeable.push(start.clone());
                    }
                }
            }
        }
        let started = match self.wake_reason(&wakeable) {
            Ok(Some(spec)) => {
                self.notices.window.record(self.shared.deps.clock.now());
                Some(spec)
            }
            Ok(None) => None,
            Err(reason) => {
                for notice in &self.notices.waiting {
                    let seq = notice.source_seq;
                    if !self.notices.held.contains(&seq) && self.notices.alone(notice).is_none() {
                        held.push((notice.job.id.clone(), seq, reason));
                    }
                }
                None
            }
        };
        self.record_held(held).await;
        if let Some(mut spec) = started {
            spec.notices = wake::take_notices(&mut self.notices.waiting);
            spec.more_notices = self.notices.waiting.len();
            self.notices.forget_held(&spec.notices);
            self.queue.push_front(spec);
            self.start_next();
        }
    }

    /// The wake turn for notices of the jobs of `wakeable`, `None` when there is none,
    /// or why the limits hold them.
    fn wake_reason(&mut self, wakeable: &[JobStart]) -> Result<Option<TurnSpec>, HeldReason> {
        let Some(first) = wakeable.first() else {
            return Ok(None);
        };
        let config = self.shared.config.current();
        if config.max_wakes_per_hour == 0 {
            return Err(HeldReason::WakesOff);
        }
        let start = wakeable.iter().map(|start| start.mode).min().unwrap_or(first.mode);
        let mode = wake::wake_mode(start, self.notices.newest_prompt);
        // NOTE: the turn resolves these settings when it starts, as a prompt's turn
        // does; a value that cannot work then fails the turn, which the user sees.
        let settings = TurnSettings { mode: Some(mode), ..first.settings.clone() };
        let now = self.shared.deps.clock.now();
        let conversation_id = self.conversation_id();
        if !self.notices.window.allows(now, config.max_wakes_per_hour)
            || !self.shared.deps.toolbox.wake_allowed(conversation_id)
        {
            return Err(HeldReason::WakeLimit);
        }
        Ok(Some(TurnSpec {
            turn_id: self.new_turn_id(),
            text: String::new(),
            origin: first.origin,
            context: None,
            last_command: None,
            settings,
            kind: TurnKind::Wake { cwd: first.cwd.clone() },
            notices: Vec::new(),
            more_notices: 0,
            carried: self.notices.carried.clone(),
        }))
    }

    /// Records `job_notice_held` for each of `held`, and marks them held.
    async fn record_held(&mut self, held: Vec<(JobId, Seq, HeldReason)>) {
        if held.is_empty() {
            return;
        }
        let conversation_id = self.conversation_id();
        let mut batch = Batch::new();
        for (job_id, source_seq, reason) in held {
            self.notices.held.insert(source_seq);
            batch =
                batch.event(conversation_id, Event::JobNoticeHeld { job_id, source_seq, reason });
        }
        if let Err(error) = self.shared.deps.writer.append(batch).await {
            tracing::warn!(error = %error, "a held job notice could not be recorded");
        }
    }
}
