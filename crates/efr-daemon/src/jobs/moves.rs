//! The moves of running `shell` calls into background jobs: Ctrl+B in the turn view of
//! `efr` sends `call.background` (v0.0.4 shell items, DS15-DS33 and DS53-DS56, with the
//! user's change of OQS2).
//!
//! - The tool path registers each foreground `shell` call while it runs
//!   ([`Moves::register`]), with what the turn judged of it (its requirements and
//!   facts), how it runs and when it started. The registration goes when the call
//!   answers.
//! - The handler of `call.background` checks the call ([`check`]): jobs are on, the line
//!   has no hard `interactive` flag and no nested shell, a contained call holds no cache
//!   layer lock, and the engine decides the line again as a job, with the engine and the
//!   settings of the key press. Any `Deny` refuses. efr-shell checks the rest when it
//!   hands the run over: the command started, it is no sentinel run, no shell or REPL,
//!   it waits for no input, and no full-screen program holds the screen.
//! - A call in the sandbox moves at once: it keeps its sandbox, so the move opens
//!   nothing new, and the key press is the user's answer. A call outside the sandbox
//!   (the exit child, a line typed into the hidden shell, full mode) first gets a
//!   preview with a token ([`Moves::preview`]); only a confirm with that token within
//!   [`PREVIEW_LIFE`] and the same job decision moves it ([`Moves::answered`]).
//! - The move ([`Moves::confirm`]): the handler reserves the job's place, puts it with
//!   the receiver of the adopted run and its reply in the call's entry, and asks
//!   efr-shell to hand the run over. The run then answers its call with
//!   `Completion::Moved`; the tool path, which still holds the call's proxy, bus and
//!   snapshot, takes the entry ([`Registration::finish`]), adopts the run into the job
//!   table and answers the handler with the job.
//! - A command that a call left running in the hidden shell moves too (OQS16, DS30):
//!   when such a call answers, its registration stays as a left call
//!   ([`Moves::left_call`]) until its conversation leaves another one. The handler
//!   checks it in the same way, previews it in the same way outside the sandbox, and
//!   then makes the job itself ([`into_job`]): no tool path waits for it any more. Its
//!   proxy socket and bus filters come from the lists of calls past their timeout, and
//!   its reach ends.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::CallContext;
use efr_permissions::{Cause, ConversationPolicy, DecisionInput, Effect, Engine, Requirements};
use efr_protocol::{
    CallId, ConversationId, ExitInfo, ExitKind, JobId, JobInfo, JobState, Launch, MovedCall,
};
use efr_shell::AdoptedRun;
use jiff::Timestamp;
use tokio::sync::oneshot;

use crate::jobs::{AdoptedJob, JobTable, Slot, output, stop_adopted};
use crate::sandbox::bus::BusCall;
use crate::sandbox::proxy::ProxyCall;

/// How long the token of a preview stays good.
pub(crate) const PREVIEW_LIFE: Duration = Duration::from_secs(60);

/// How long the handler waits for the tool path to record the job.
pub(crate) const MOVE_WAIT: Duration = Duration::from_secs(10);

/// The call does not run as a foreground `shell` call of the conversation.
pub(crate) const NOT_RUNNING: &str = "the call does not run";
/// The call ended before it moved.
pub(crate) const ENDED: &str = "the command ended before it moved";
/// This efrd runs no background jobs.
pub(crate) const NO_JOBS: &str = "this efrd runs no background jobs";
/// `jobs.enabled` is false.
pub(crate) const JOBS_OFF: &str = "jobs are off (jobs.enabled = false)";
/// The table has no free place.
pub(crate) const JOBS_FULL: &str = "the most jobs that efr allows run now; stop one first";
/// The line has the hard `interactive` flag.
pub(crate) const INTERACTIVE: &str = "the command can ask at the terminal; a job has no terminal";
/// The line types into a nested shell.
pub(crate) const NESTED: &str = "the call types into a nested shell; it cannot move";
/// A contained call holds the cache layer lock (DS19.7, OQS7).
pub(crate) const CACHE: &str = "the call holds the sandbox's cache; it cannot move";
/// The job decision refuses a part of the line that no job may hold.
pub(crate) const NOT_AS_JOB: &str = "this cannot run as a job";
/// The job decision denies the line for another reason.
pub(crate) const DENIED: &str = "efr denies this command as a job";
/// The turn's record of the call is gone.
pub(crate) const NOT_JUDGED: &str = "efr no longer knows how it judged the call";
/// The confirm's token is not the preview's, or its time passed.
pub(crate) const EXPIRED: &str = "the question expired; press ctrl+b again";
/// The job decision is not the one that the preview showed.
pub(crate) const CHANGED: &str = "the decision changed; press ctrl+b again";
/// The handler did not hear from the tool path in time.
pub(crate) const UNFINISHED: &str = "the move did not finish; see efr jobs";
/// The job could not be recorded, so efr stopped the command.
pub(crate) const NOT_RECORDED: &str = "efr could not record the job, so it stopped the command";
/// A command left running waits for input (DS27).
pub(crate) const WAITS: &str = "the command waits for input; answer it first";

/// A foreground `shell` call while it runs, as the tool path registered it.
#[derive(Debug, Clone)]
pub(crate) struct RunningCall {
    /// The call's context, with how it runs (its launch, mode and origin).
    pub(crate) context: CallContext,
    /// What the turn judged of the call: its requirements with their facts. `None` when
    /// the toolbox kept no record of them.
    pub(crate) requirements: Option<Requirements>,
    /// The line, as the model wrote it.
    pub(crate) command: String,
    /// When the call began to run, on efrd's clock.
    pub(crate) started: Timestamp,
    /// True for a call in the sandbox.
    pub(crate) contained: bool,
    /// The call's dir `$R/sbx/<conversation>/<call>`, for a call through the launcher.
    pub(crate) call_dir: Option<PathBuf>,
    /// True for a contained call whose plan holds the cache layer lock.
    pub(crate) holds_cache: bool,
}

/// What a call whose command was left running keeps for a later move (OQS16): the roots
/// of its job.
#[derive(Debug, Clone, Default)]
pub(crate) struct LeftParts {
    /// The roots that the surface guard of the job scans.
    pub(crate) guard_roots: Vec<PathBuf>,
    /// The roots that the job may write.
    pub(crate) write_roots: Vec<PathBuf>,
}

/// What the job decision of a move found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobCheck {
    /// The exits of the job decision, for the question and the record (DS28). Empty
    /// when the decision asks for nothing.
    pub(crate) exits: Vec<ExitInfo>,
    /// The job's time limit in minutes, from the move.
    pub(crate) minutes: u32,
    /// How the job runs: a call typed into the hidden shell counts as the exit child.
    pub(crate) launch: Launch,
}

/// Checks whether `call` can move into a background job, with the latest `engine` and
/// `settings`: the checks of DS19 that efrd makes (efr-shell makes the others), and the
/// job decision of the line. `Err` is the reason for the person who pressed the key.
pub(crate) fn check(
    engine: &Engine,
    call: &RunningCall,
    settings: &Settings,
) -> Result<JobCheck, &'static str> {
    if !settings.jobs.enabled {
        return Err(JOBS_OFF);
    }
    let Some(requirements) = &call.requirements else { return Err(NOT_JUDGED) };
    if requirements.interactive {
        return Err(INTERACTIVE);
    }
    if requirements.nested {
        return Err(NESTED);
    }
    if call.holds_cache {
        return Err(CACHE);
    }
    // NOTE: the default limit of a job, from the move, cut to the most (DS24).
    let minutes = settings.jobs.default_minutes.max(1).min(settings.jobs.max_minutes.max(1));
    let mut requirements = requirements.clone();
    requirements.background = true;
    requirements.max_minutes = Some(minutes);
    let context = &call.context;
    let input = DecisionInput {
        requirements,
        scope: context.scope.clone(),
        origin: context.origin,
        mode: context.mode,
        conversation_policy: ConversationPolicy::new(&context.scratch),
    };
    let decision = engine.decide(&input);
    if decision.effect() == Effect::Deny {
        let not_as_job = decision.deciding().any(|reason| reason.cause == Cause::NotAsJob);
        return Err(if not_as_job { NOT_AS_JOB } else { DENIED });
    }
    // NOTE: a job outside `auto` counts as one in the exit child, as `start_job` maps it.
    let launch = match &context.launch {
        Launch::Direct => Launch::Unsandboxed,
        launch => launch.clone(),
    };
    let mut kinds: Vec<ExitKind> = Vec::new();
    for need in decision.exits() {
        if !kinds.contains(&need.kind) {
            kinds.push(need.kind);
        }
    }
    let facts: Vec<String> = match decision.effect() {
        Effect::Ask => decision.deciding().map(|reason| reason.for_model().to_string()).collect(),
        _ => Vec::new(),
    };
    let exits = if kinds.is_empty() && facts.is_empty() {
        Vec::new()
    } else {
        vec![ExitInfo {
            kinds,
            grants: launch.grants().to_vec(),
            launch: launch.clone(),
            facts,
            model_reason: None,
            judged: None,
            user_only: true,
            always: None,
        }]
    };
    Ok(JobCheck { exits, minutes, launch })
}

/// What a moved command hands its job besides the run.
#[derive(Debug, Default)]
pub(crate) struct MovedParts {
    /// The call's proxy socket, served until the job ends (DS25).
    pub(crate) proxy: Option<ProxyCall>,
    /// The call's filtered bus proxies, until the job ends.
    pub(crate) bus: Option<BusCall>,
    pub(crate) guard_roots: Vec<PathBuf>,
    pub(crate) write_roots: Vec<PathBuf>,
}

/// Makes `run`, the adopted run of the call `running` that efr-shell handed over, a
/// background job in the place `slot` with the decision `check` and `parts` at `now`
/// (DS22, DS28): first `$CALL/moved` for a call through the launcher (DS23), then the
/// job's number, its record and its task. Returns the job and how long the call ran
/// before the move. `Err` is the reason for the person; the command stops then.
pub(crate) async fn into_job(
    jobs: &JobTable,
    now: Timestamp,
    running: &RunningCall,
    slot: Slot,
    check: &JobCheck,
    run: AdoptedRun,
    parts: MovedParts,
) -> Result<(JobInfo, Duration), &'static str> {
    // NOTE: only now that efr-shell handed the run over: a launcher that ended before
    // the hand-over wrote the shell state of a call that did not move. One that ends
    // after it but before the mark writes its state before any later call of the
    // conversation can run, since the shell was busy with this command (DS23).
    if let Some(dir) = &running.call_dir {
        make_moved_file(dir.join(efr_sandbox::MOVED_FILE)).await;
    }
    let context = &running.context;
    let conversation = context.conversation_id;
    let number = match jobs.number(conversation).await {
        Ok(number) => number,
        Err(error) => {
            tracing::warn!(%conversation, error = %error, "a moved call got no job number; efr stops it");
            stop_adopted(&run).await;
            return Err(NOT_RECORDED);
        }
    };
    let id = JobId::nth(number);
    let after = Duration::try_from(now.duration_since(running.started)).unwrap_or_default();
    let output_path = context.scratch.join(efr_sandbox::JOB_DIR).join(output::file_name(&id));
    let info = JobInfo {
        id,
        conversation_id: conversation,
        turn_id: context.turn_id,
        call_id: context.call_id,
        command: running.command.clone(),
        launch: check.launch.clone(),
        state: JobState::Running,
        started_at: now,
        ends_by: now,
        ended_at: None,
        exit_code: None,
        stopped_by: None,
        output: output_path,
        output_bytes: 0,
        watch: None,
        wakes: 0,
        forwards: Vec::new(),
        moved_from: Some(MovedCall {
            call_id: context.call_id,
            turn_id: context.turn_id,
            after_secs: after.as_secs(),
            exits: check.exits.clone(),
            launcher: running.call_dir.is_some(),
        }),
        waits_input: false,
        cgroup: None,
    };
    let MovedParts { proxy, bus, guard_roots, write_roots } = parts;
    let job = AdoptedJob {
        info,
        limit: Duration::from_secs(u64::from(check.minutes) * 60),
        scratch: context.scratch.clone(),
        call_dir: running.call_dir.clone(),
        guard_roots,
        write_roots,
        contained: running.contained,
        proxy,
        bus,
        full: context.is_full(),
    };
    match jobs.adopt(run, slot, job).await {
        Ok(info) => Ok((info, after)),
        Err(error) => {
            tracing::warn!(%conversation, error = %error, "a moved call could not become a job; efr stopped it");
            Err(NOT_RECORDED)
        }
    }
}

/// Makes `$CALL/moved` as an empty file in one call (`O_CREAT | O_EXCL`, 0600), so the
/// launcher of a moved call writes no shell state at its end (DS23).
async fn make_moved_file(path: PathBuf) {
    let made = tokio::task::spawn_blocking(move || {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)
    })
    .await;
    if !matches!(made, Ok(Ok(_))) {
        tracing::warn!(
            "the moved mark of a call could not be made; its launcher may write the shell state of its start"
        );
    }
}

/// The job that a move made, as the tool path tells the handler.
#[derive(Debug, Clone)]
pub(crate) struct Moved {
    pub(crate) job: JobId,
}

/// What the handler hands the tool path for a move that efr-shell takes: the job's
/// place, the run, the decision and where the answer goes.
#[derive(Debug)]
pub(crate) struct Confirmed {
    pub(crate) slot: Slot,
    pub(crate) adopted: oneshot::Receiver<AdoptedRun>,
    pub(crate) reply: oneshot::Sender<Result<Moved, &'static str>>,
    pub(crate) check: JobCheck,
}

/// A preview that waits for the user's answer.
#[derive(Debug)]
struct Preview {
    token: u64,
    exits: Vec<ExitInfo>,
    at: Timestamp,
}

#[derive(Debug)]
struct Entry {
    call: RunningCall,
    preview: Option<Preview>,
    confirmed: Option<Confirmed>,
}

/// A call whose command was left running in the hidden shell, for a move of that
/// command (OQS16).
#[derive(Debug)]
struct LeftEntry {
    call: RunningCall,
    parts: LeftParts,
    preview: Option<Preview>,
}

/// The foreground `shell` calls that run now, by call, with their pending moves (DS54).
/// Cheap to clone; clones share everything.
#[derive(Debug, Clone)]
pub(crate) struct Moves {
    calls: Arc<Mutex<HashMap<CallId, Entry>>>,
    /// The calls whose command was left running, at most one for each conversation: the
    /// hidden shell takes no other marked line while such a command runs.
    left: Arc<Mutex<HashMap<CallId, LeftEntry>>>,
    tokens: Arc<AtomicU64>,
}

impl Default for Moves {
    fn default() -> Self {
        Moves { calls: Arc::default(), left: Arc::default(), tokens: Arc::new(AtomicU64::new(1)) }
    }
}

impl Moves {
    fn lock(&self) -> MutexGuard<'_, HashMap<CallId, Entry>> {
        // Each change is whole, so a poisoned lock still holds a usable map.
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers `call` while it runs; the registration goes when the returned value
    /// drops or [finishes](Registration::finish).
    pub(crate) fn register(&self, call: RunningCall) -> Registration {
        let id = call.context.call_id;
        self.lock().insert(id, Entry { call, preview: None, confirmed: None });
        Registration { moves: self.clone(), call: id }
    }

    /// The call `call` of `conversation` while it runs and has no move under way.
    pub(crate) fn running(
        &self,
        conversation: ConversationId,
        call: CallId,
    ) -> Option<RunningCall> {
        self.lock()
            .get(&call)
            .filter(|entry| entry.call.context.conversation_id == conversation)
            .filter(|entry| entry.confirmed.is_none())
            .map(|entry| entry.call.clone())
    }

    /// Keeps a preview of the move of `call` with `exits` at `now`, and returns its
    /// token; `None` when the call no longer runs.
    pub(crate) fn preview(
        &self,
        call: CallId,
        exits: Vec<ExitInfo>,
        now: Timestamp,
    ) -> Option<u64> {
        let token = self.tokens.fetch_add(1, Ordering::Relaxed);
        let mut calls = self.lock();
        let entry = calls.get_mut(&call)?;
        entry.preview = Some(Preview { token, exits, at: now });
        Some(token)
    }

    /// Takes the preview of `call` that the user answered with `token`, at `now`, when
    /// the job decision still finds `exits`.
    pub(crate) fn answered(
        &self,
        call: CallId,
        token: u64,
        exits: &[ExitInfo],
        now: Timestamp,
    ) -> Result<(), &'static str> {
        let mut calls = self.lock();
        let entry = calls.get_mut(&call).ok_or(NOT_RUNNING)?;
        let preview = entry.preview.take().ok_or(EXPIRED)?;
        let age = Duration::try_from(now.duration_since(preview.at)).unwrap_or(Duration::MAX);
        if preview.token != token || age > PREVIEW_LIFE {
            return Err(EXPIRED);
        }
        if preview.exits != exits {
            return Err(CHANGED);
        }
        Ok(())
    }

    /// Puts the move `confirmed` in the entry of `call`; gives it back when the call no
    /// longer runs or a move of it is under way.
    pub(crate) fn confirm(&self, call: CallId, confirmed: Confirmed) -> Result<(), Confirmed> {
        let mut calls = self.lock();
        match calls.get_mut(&call) {
            Some(entry) if entry.confirmed.is_none() => {
                entry.confirmed = Some(confirmed);
                Ok(())
            }
            _ => Err(confirmed),
        }
    }

    /// Drops the move of `call` that efr-shell did not take: its place is free again.
    pub(crate) fn cancel(&self, call: CallId) {
        let confirmed = self.lock().get_mut(&call).and_then(|entry| entry.confirmed.take());
        drop(confirmed);
    }

    fn left_lock(&self) -> MutexGuard<'_, HashMap<CallId, LeftEntry>> {
        // Each change is whole, so a poisoned lock still holds a usable map.
        self.left.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `call`, whose command was left running, with `parts` for a later move. An
    /// older left call of the same conversation goes: its command ended, or its shell
    /// did, since the hidden shell takes no other call while one runs.
    fn keep_left(&self, call: RunningCall, parts: LeftParts) {
        let conversation = call.context.conversation_id;
        let mut left = self.left_lock();
        left.retain(|_, entry| entry.call.context.conversation_id != conversation);
        left.insert(call.context.call_id, LeftEntry { call, parts, preview: None });
    }

    /// The call `call` of `conversation` whose command was left running, as its call
    /// registered it.
    pub(crate) fn left_call(
        &self,
        conversation: ConversationId,
        call: CallId,
    ) -> Option<RunningCall> {
        self.left_lock()
            .get(&call)
            .filter(|entry| entry.call.context.conversation_id == conversation)
            .map(|entry| entry.call.clone())
    }

    /// Forgets the left call `call`: its command no longer runs in the hidden shell.
    pub(crate) fn forget_left(&self, call: CallId) {
        self.left_lock().remove(&call);
    }

    /// [`preview`](Self::preview) for the command that `call` left running.
    pub(crate) fn preview_left(
        &self,
        call: CallId,
        exits: Vec<ExitInfo>,
        now: Timestamp,
    ) -> Option<u64> {
        let token = self.tokens.fetch_add(1, Ordering::Relaxed);
        let mut left = self.left_lock();
        let entry = left.get_mut(&call)?;
        entry.preview = Some(Preview { token, exits, at: now });
        Some(token)
    }

    /// [`answered`](Self::answered) for the command that `call` left running.
    pub(crate) fn answered_left(
        &self,
        call: CallId,
        token: u64,
        exits: &[ExitInfo],
        now: Timestamp,
    ) -> Result<(), &'static str> {
        let mut left = self.left_lock();
        let entry = left.get_mut(&call).ok_or(NOT_RUNNING)?;
        let preview = entry.preview.take().ok_or(EXPIRED)?;
        let age = Duration::try_from(now.duration_since(preview.at)).unwrap_or(Duration::MAX);
        if preview.token != token || age > PREVIEW_LIFE {
            return Err(EXPIRED);
        }
        if preview.exits != exits {
            return Err(CHANGED);
        }
        Ok(())
    }

    /// Takes the left call `call` for its move, so a second move of it finds none.
    pub(crate) fn take_left(&self, call: CallId) -> Option<(RunningCall, LeftParts)> {
        self.left_lock().remove(&call).map(|entry| (entry.call, entry.parts))
    }

    /// Puts back the left call `running` that efr-shell did not hand over, for a later
    /// try.
    pub(crate) fn restore_left(&self, running: RunningCall, parts: LeftParts) {
        let call = running.context.call_id;
        self.left_lock().entry(call).or_insert(LeftEntry { call: running, parts, preview: None });
    }
}

/// A call's registration for a move, until it answers.
#[derive(Debug)]
pub(crate) struct Registration {
    moves: Moves,
    call: CallId,
}

impl Registration {
    /// Ends the registration once the call answered, with the move that the handler
    /// put in it, if any. A call that did not move and whose command was left running
    /// (`left`) stays for a move of that command (OQS16).
    pub(crate) fn finish(self, left: Option<LeftParts>) -> Option<(RunningCall, Confirmed)> {
        let entry = self.moves.lock().remove(&self.call)?;
        match entry.confirmed {
            Some(confirmed) => Some((entry.call, confirmed)),
            None => {
                if let Some(parts) = left {
                    self.moves.keep_left(entry.call, parts);
                }
                None
            }
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // NOTE: a call that an interrupt dropped takes its move with it: the reply goes
        // unanswered, and a run that efr-shell handed over finds no taker, so efr-shell
        // closes its shell.
        let entry = self.moves.lock().remove(&self.call);
        drop(entry);
    }
}

#[cfg(test)]
mod tests;
