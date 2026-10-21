//! The job table (efr's auto spec, section 3.18): the background jobs that `shell` calls
//! with `background: true` started, while they run and when they end.
//!
//! The toolbox prepares a job (`tools/job_tools.rs`): its call dir below
//! `$R/sbx/<conversation>/jobs/`, the spec with its job plan, the snapshot before it
//! under the plan lock. The table spawns the launcher (`jobs/launch.rs`), puts its pid
//! in the list of model-side peers, and only then writes the start byte. It reads the
//! output pipe into `$SCRATCH/jobs/<id>.out` (`jobs/output.rs`), records `job_started`
//! before the tool answers, and runs one task per job under its own cancel token, never
//! in efrd's `background` tasks, so the drain at efrd's stop still sees each job end.
//!
//! A job ends when its command ends, at its time limit (on efrd's clock), when the
//! model (`job_stop`), the user (`jobs.stop`) or efrd's stop ends it, and a job that a
//! full call started when a reload turns full mode off ([`JobTable::stop_full`]). Every
//! stop by efrd sends SIGCONT and SIGTERM to the launcher through its pidfd, SIGKILL after
//! [`STOP_GRACE`], and then the sweep: SIGKILL to every process of the job's session
//! (`jobs/sweep.rs`). The table records `job_ended` with the tail, the processes that
//! may still run and whether the launcher was lost, hands the actor the notice (not
//! while efrd stops), and removes the job's dirs.
//!
//! A running `shell` call that the user moved into a job with Ctrl+B
//! ([`JobTable::adopt`], `jobs/moves.rs`) has no launcher of its own: the job owns the
//! call's old hidden shell and its PTY ([`JobProcess::Adopted`]). Its stop sends
//! `SIGINT` to the shell's foreground job and closes the shell after [`STOP_GRACE`]. A
//! wait for a secret that efr-shell interrupted gets the same grace before the close.
//! Each close first kills every process below the shell (DS55): a process that left
//! the shell's session is below the shell only while the shell lives. The end then
//! sweeps the shell's session. A call through the launcher keeps it: its `result.json` in the call's dir is
//! the result, as for a job. A call typed into the shell has none; its exit status is.
//!
//! At start, a job that the last efrd left running gets `job_ended` with
//! `daemon_restart`: efrd kills its launcher only when the pid's start time still
//! matches the `job_started` event, sweeps its session, and reads a `result.json` that
//! the launcher left. Its notice line goes to the terminal of its conversation right
//! away ([`restart_notices`]): the notices task starts later and never sees that end.
//! Then every actor gets the notices of the last day that no turn took, which never
//! start a turn of their own.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_config::JobsSettings;
use efr_conversation::{JobNoticeInput, NoticeBody, notice_text, notice_text_refused};
use efr_protocol::{
    CallId, ConversationId, Event, InputWait, JobId, JobInfo, JobState, JobStopBy, Launch,
    NoticeKind, Seq, SurfaceChange, TurnId,
};
use efr_sandbox::{RESULT_FILE, SandboxResult};
use efr_stdx::paths::Dirs;
use efr_stdx::time::Clock;
use efr_store::{Batch, Readers, WriterHandle};
use efr_tools::{blocked_note, bus_note};
use jiff::Timestamp;
use rustix::process::Signal;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::full::FullTurns;
use crate::sandbox::bus::{BusCall, BusReport};
use crate::sandbox::plan::{SANDBOX_DIR, SHELL_DIR};
use crate::sandbox::proxy::ProxyCall;
use crate::sandbox::seams::HostPorts;

pub(crate) mod forward;
pub(crate) mod guard;
pub(crate) mod launch;
pub(crate) mod moves;
pub(crate) mod output;
pub(crate) mod retention;
pub(crate) mod since;
pub(crate) mod sweep;
pub(crate) mod undo;
pub(crate) mod wakes;

pub(crate) use launch::SpawnEnv;
pub(crate) use output::Matcher;

/// How long a stop waits for the launcher to end after SIGTERM, before SIGKILL: the
/// launcher's own 4 s, and 2 s more.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(6);

/// How long the end of a job waits for the rest of its output after the launcher ended.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// How often the table looks whether a swept session still has processes.
const SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// The most jobs that one answer of `jobs.list` holds.
const MAX_LIST: u32 = efr_protocol::JobsListResult::MAX_JOBS as u32;

/// How far back efrd looks at start for notices that no turn took.
const UNDELIVERED_WINDOW: Duration = Duration::from_secs(24 * 3600);

/// A job, named by its conversation and its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct JobKey {
    pub(crate) conversation: ConversationId,
    pub(crate) number: u32,
}

impl JobKey {
    /// The key of the job `id` of `conversation`; `None` for an id with no number.
    pub(crate) fn of(conversation: ConversationId, id: &JobId) -> Option<JobKey> {
        id.number().map(|number| JobKey { conversation, number })
    }
}

/// A running job, as the guard schedule (`jobs/guard.rs`), the undo of a turn
/// (`jobs/undo.rs`) and the snapshot collector read it.
#[derive(Debug, Clone)]
pub(crate) struct RunningJob {
    pub(crate) key: JobKey,
    #[expect(dead_code, reason = "no reader names a running job by its call yet")]
    pub(crate) call: CallId,
    pub(crate) turn: TurnId,
    /// The roots that the surface guard of the job scans.
    pub(crate) guard_roots: Vec<PathBuf>,
    /// The roots that the job may write.
    pub(crate) write_roots: Vec<PathBuf>,
    pub(crate) started_at: Timestamp,
    /// True for a job in the sandbox; false for an exit child job.
    pub(crate) contained: bool,
    /// The job's call dir, `$R/sbx/<conversation>/jobs/<call>`.
    pub(crate) call_dir: PathBuf,
    /// The job's state dir, `$SBX/jobs/<call>`.
    pub(crate) state_dir: PathBuf,
    /// True for a call that the user moved into the job: it has no state dir, and its
    /// guard runs only at its end, in its launcher (DS25, OQS8).
    pub(crate) moved: bool,
}

/// A notice for a conversation's actor.
#[derive(Debug)]
pub(crate) struct Notice {
    pub(crate) conversation: ConversationId,
    pub(crate) input: JobNoticeInput,
}

/// What the table is built from.
pub(crate) struct JobParts {
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) writer: WriterHandle,
    pub(crate) readers: Readers,
    pub(crate) dirs: Dirs,
    /// The launcher's copy, `$R/bin/efr-sbx`.
    pub(crate) launcher: PathBuf,
    pub(crate) spawn_env: SpawnEnv,
    /// The full work that runs, which counts each job that a full call started.
    pub(crate) full: FullTurns,
    /// The host ports of the job port forwards: the system's, or a test's.
    pub(crate) host_ports: Arc<dyn HostPorts>,
}

/// A job that the toolbox prepared: the launcher may start it.
#[derive(Debug)]
pub(crate) struct NewJob {
    /// The job as it starts; `started_at` and `ends_by` are set at the start.
    pub(crate) info: JobInfo,
    /// The time limit.
    pub(crate) limit: Duration,
    pub(crate) call_dir: PathBuf,
    pub(crate) state_dir: PathBuf,
    /// The job's start dir.
    pub(crate) cwd: PathBuf,
    /// The conversation's `$SCRATCH`.
    pub(crate) scratch: PathBuf,
    pub(crate) matcher: Option<Matcher>,
    /// How many watch matches may still wake the model.
    pub(crate) wakes: u8,
    pub(crate) guard_roots: Vec<PathBuf>,
    pub(crate) write_roots: Vec<PathBuf>,
    pub(crate) contained: bool,
    /// The job's proxy socket, served for its whole life (DP14); `None` without a proxy
    /// plan.
    pub(crate) proxy: Option<ProxyCall>,
    /// The job's filtered bus proxies, for its whole life with the rules of its start
    /// (DE1, DE12); `None` without a bus view.
    pub(crate) bus: Option<BusCall>,
    /// The job's forward socket, for its whole life (DL6); `None` for a job with no
    /// port forwards.
    pub(crate) forward: Option<forward::ForwardSocket>,
    /// True when a call of full mode started the job: it counts as full work until it
    /// ends, so a reload holds a loosening change of the config meanwhile.
    pub(crate) full: bool,
}

/// A running call that the user moved into a job, as the tool path hands it to
/// [`JobTable::adopt`].
#[derive(Debug)]
pub(crate) struct AdoptedJob {
    /// The job; `started_at` and `ends_by` are set at the adopt.
    pub(crate) info: JobInfo,
    /// The time limit, from the move (DS24).
    pub(crate) limit: Duration,
    /// The conversation's `$SCRATCH`.
    pub(crate) scratch: PathBuf,
    /// The call's dir, for a call through the launcher; `None` for a line typed into
    /// the hidden shell.
    pub(crate) call_dir: Option<PathBuf>,
    pub(crate) guard_roots: Vec<PathBuf>,
    pub(crate) write_roots: Vec<PathBuf>,
    pub(crate) contained: bool,
    /// The call's proxy socket, served until the job ends (DS25).
    pub(crate) proxy: Option<ProxyCall>,
    /// The call's filtered bus proxies, until the job ends.
    pub(crate) bus: Option<BusCall>,
    /// True when a call of full mode moved.
    pub(crate) full: bool,
}

/// A launcher that efrd spawned and that waits for its start byte, with the reader of
/// its output.
#[derive(Debug)]
pub(crate) struct Spawned {
    process: launch::Spawned,
    output: output::Output,
    matches: mpsc::UnboundedReceiver<String>,
}

impl Spawned {
    /// The launcher's pid.
    pub(crate) fn pid(&self) -> u32 {
        self.process.pid
    }
}

/// What the snapshot of a job's write roots right after its start holds, for its end.
pub(crate) type KeptSnapshot = (efr_snapshot::CallSnapshot, efr_snapshot::Limits);

/// The job table. Cheap to clone; clones share everything.
#[derive(Clone)]
pub(crate) struct JobTable {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for JobTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobTable").field("running", &self.lock().running.len()).finish()
    }
}

struct Inner {
    clock: Arc<dyn Clock>,
    writer: WriterHandle,
    readers: Readers,
    dirs: Dirs,
    launcher: PathBuf,
    spawn_env: SpawnEnv,
    full: FullTurns,
    /// The port forwards of every job.
    forwards: forward::Forwards,
    /// The foreground `shell` calls that may move into a job.
    moves: moves::Moves,
    table: Mutex<Table>,
    notices: mpsc::UnboundedSender<Notice>,
    /// Cancelled once efrd recorded the end of every job at its stop.
    cancel: CancellationToken,
}

#[derive(Default)]
struct Table {
    running: HashMap<JobKey, Entry>,
    /// The next number of each conversation that the table saw.
    next: HashMap<ConversationId, u32>,
    /// The launchers that efrd spawned, from the spawn to the end of the sweep.
    launchers: HashSet<u32>,
    /// The sessions of jobs that ended while a process still had them.
    sessions: HashSet<u32>,
    /// True while efrd stops: no notice goes to an actor.
    stopping: bool,
    /// The jobs of each conversation that start now, from the limit check until their
    /// start is done (`Slot`).
    starting: HashMap<ConversationId, usize>,
}

/// The place of a job that starts in the limits of the table; it frees the place when
/// it drops, after the job runs or did not start.
pub(crate) struct Slot {
    table: JobTable,
    conversation: ConversationId,
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slot").field("conversation", &self.conversation).finish()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut table = self.table.lock();
        if let Some(count) = table.starting.get_mut(&self.conversation) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                table.starting.remove(&self.conversation);
            }
        }
    }
}

/// A running job.
struct Entry {
    info: JobInfo,
    meta: RunningJob,
    output: output::Output,
    stop: watch::Sender<Option<JobStopBy>>,
    ended: watch::Receiver<Option<JobInfo>>,
    /// The lifeline's write end; it closes when the job ends or efrd goes.
    lifeline: Option<std::io::PipeWriter>,
    /// The snapshot of the job's write roots right after its start. A moved call's
    /// job gets it after the move ([`JobTable::keep_snapshot`]).
    kept: Option<KeptSnapshot>,
    /// True once the job's end took [`kept`](Self::kept): a snapshot that comes later
    /// joins the turn on its own.
    kept_taken: bool,
    /// True when a call of full mode started the job.
    full: bool,
    /// The changes that a guard check found while the job ran (`jobs/guard.rs`).
    guard: Vec<SurfaceChange>,
    /// The job's proxy socket until the job ends; dropping it closes the socket.
    proxy: Option<ProxyCall>,
    /// The job's filtered bus proxies until the job ends; dropping them kills them.
    bus: Option<BusCall>,
    /// The job's port forwards until the job ends; dropping them closes the socket,
    /// the listeners and the connections.
    forward: Option<forward::JobForward>,
}

impl JobTable {
    /// The table, and the queue of notices that `run.rs` hands to the actors.
    pub(crate) fn new(parts: JobParts) -> (JobTable, mpsc::UnboundedReceiver<Notice>) {
        let JobParts { clock, writer, readers, dirs, launcher, spawn_env, full, host_ports } =
            parts;
        let (notices, queue) = mpsc::unbounded_channel();
        let inner = Inner {
            clock,
            writer,
            readers,
            dirs,
            launcher,
            spawn_env,
            full,
            forwards: forward::Forwards::new(host_ports),
            moves: moves::Moves::default(),
            table: Mutex::new(Table::default()),
            notices,
            cancel: CancellationToken::new(),
        };
        (JobTable { inner: Arc::new(inner) }, queue)
    }

    fn lock(&self) -> MutexGuard<'_, Table> {
        // Each change is whole, so a poisoned lock still holds a usable table.
        self.inner.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The time now on efrd's clock.
    pub(crate) fn now(&self) -> Timestamp {
        self.inner.clock.now()
    }

    /// The port forwards of every job.
    pub(crate) fn forwards(&self) -> &forward::Forwards {
        &self.inner.forwards
    }

    /// The foreground `shell` calls that may move into a job now.
    pub(crate) fn moves(&self) -> &moves::Moves {
        &self.inner.moves
    }

    /// Stops the port forwards of every running job, with a `stopped` event for each
    /// port, after a reload turned `jobs.forward_ports` off (DL11). A job that read the
    /// settings before and starts later forwards nothing either.
    pub(crate) fn forwards_off(&self) {
        self.inner.forwards.turn_off();
        let table = self.lock();
        for entry in table.running.values() {
            if let Some(forward) = &entry.forward {
                forward.turn_off();
            }
        }
    }

    /// The call dir of the job that `call` of `conversation` starts.
    pub(crate) fn call_dir(&self, conversation: ConversationId, call: CallId) -> PathBuf {
        job_call_dir(&self.inner.dirs, conversation, call)
    }

    /// The `env` file `env` of a job, with efrd's own roots when it runs in the exit
    /// child (`unsandboxed`); a contained job's file stays as it is.
    pub(crate) fn job_env(&self, env: Vec<u8>, unsandboxed: bool) -> Vec<u8> {
        if unsandboxed { launch::with_roots(&env, &self.inner.spawn_env.roots) } else { env }
    }

    /// The token under which every job task runs.
    pub(crate) fn token(&self) -> CancellationToken {
        self.inner.cancel.clone()
    }

    /// The next job number of `conversation`: one more than the highest that the store
    /// or this table knows, so two starts never get one id and an id never repeats
    /// after a restart.
    pub(crate) async fn number(&self, conversation: ConversationId) -> Result<u32, DaemonError> {
        let known = self.lock().next.get(&conversation).copied();
        let seed = match known {
            Some(next) => next,
            None => {
                self.inner
                    .readers
                    .with(move |conn| efr_store::jobs::next_number(conn, conversation))
                    .await?
            }
        };
        let mut table = self.lock();
        let next = table.next.entry(conversation).or_insert(seed);
        *next = (*next).max(seed);
        let number = *next;
        *next = next.saturating_add(1);
        Ok(number)
    }

    /// Holds a place for one more job of `conversation` until the [`Slot`] drops, so
    /// two starts at once cannot both pass `jobs.max_running` or
    /// `jobs.max_running_daemon`. `Err` is what the model reads when one more would pass
    /// a limit: the running jobs, with their time left.
    pub(crate) fn reserve(
        &self,
        conversation: ConversationId,
        settings: &JobsSettings,
    ) -> Result<Slot, String> {
        let now = self.inner.clock.now();
        let mut table = self.lock();
        let own: Vec<&Entry> = table
            .running
            .values()
            .filter(|entry| entry.meta.key.conversation == conversation)
            .collect();
        // NOTE: a job that starts counts as one that runs: it holds its place from the
        // check on.
        let own_count = own.len() + table.starting.get(&conversation).copied().unwrap_or(0);
        let all_count = table.running.len() + table.starting.values().sum::<usize>();
        let limit = if own_count >= settings.max_running as usize {
            format!("{} in this conversation", jobs_run(own_count))
        } else if all_count >= settings.max_running_daemon as usize {
            format!("{} in efr", jobs_run(all_count))
        } else {
            *table.starting.entry(conversation).or_insert(0) += 1;
            return Ok(Slot { table: self.clone(), conversation });
        };
        let mut text = format!(
            "this job did not start: {limit}, the most that efr allows. Stop one with job_stop, \
             or wait for one to end."
        );
        let mut listed = own;
        listed.sort_by_key(|entry| entry.meta.key.number);
        for entry in listed {
            text.push('\n');
            text.push_str(&job_line(&entry.info, now));
        }
        Err(text)
    }

    /// Spawns the launcher of `job`, puts its pid in the list of model-side peers, and
    /// starts the reader of its output. The launcher waits for its start byte.
    pub(crate) fn spawn(&self, job: &NewJob) -> std::io::Result<Spawned> {
        let process =
            launch::spawn(&self.inner.launcher, &job.call_dir, &job.cwd, &self.inner.spawn_env)?;
        self.lock().launchers.insert(process.pid);
        let file = match output::OutputFile::create(
            &job.scratch,
            &output::file_name(&job.info.id),
            output::Limits::DEFAULT,
        ) {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::warn!(%error, job = %job.info.id, "a job's output file could not be made; its output is not kept");
                None
            }
        };
        let (send, matches) = mpsc::unbounded_channel();
        let reader = process.output.try_clone()?;
        let output =
            output::spawn_reader(job.info.id.as_str(), reader, file, job.matcher.clone(), send)?;
        Ok(Spawned { process, output, matches })
    }

    /// Writes the start byte of `spawned` and waits until its launcher wrote `started`
    /// or ended. `Err` is what the model reads for a job that did not start; its
    /// launcher is gone then.
    pub(crate) async fn start(&self, spawned: &mut Spawned, job: &NewJob) -> Result<(), String> {
        let written = launch::start(&mut spawned.process.lifeline);
        let started = job.call_dir.join(efr_sandbox::STARTED_FILE);
        let pid = spawned.process.pid;
        let waited = match written {
            Ok(()) => tokio::task::spawn_blocking(move || launch::wait_started(&started, pid))
                .await
                .unwrap_or(launch::StartWait::TimedOut),
            Err(error) => {
                tracing::warn!(%error, "the start byte of a job could not be written");
                launch::StartWait::Ended
            }
        };
        if waited == launch::StartWait::Started {
            return Ok(());
        }
        if waited == launch::StartWait::TimedOut {
            launch::signal(spawned.process.pidfd.as_ref(), pid, Signal::KILL);
        }
        let _ = spawned.process.child.wait().await;
        let swept = tokio::task::spawn_blocking(move || sweep::kill_session(pid)).await;
        drop(swept);
        self.lock().launchers.remove(&pid);
        let result = read_result(&job.call_dir).await;
        let reason = match result {
            Some(result) => result
                .setup_error
                .or(result.launch_error)
                .unwrap_or_else(|| "the launcher ended before the job started".to_owned()),
            None => "the launcher ended before the job started".to_owned(),
        };
        remove_dirs(vec![job.call_dir.clone(), job.state_dir.clone()]).await;
        Err(format!("[the job could not start: {reason}. The command did not run.]"))
    }

    /// Records `job_started` for `job`, whose launcher wrote `started`, and starts the
    /// task that follows it to its end. Returns the job as it started.
    pub(crate) async fn begin(
        &self,
        spawned: Spawned,
        mut job: NewJob,
        kept: Option<KeptSnapshot>,
    ) -> Result<JobInfo, DaemonError> {
        let Spawned { process, output, matches } = spawned;
        let proxy = job.proxy.take();
        let bus = job.bus.take();
        let socket = job.forward.take();
        let mut info = job.info.clone();
        let now = self.inner.clock.now();
        info.started_at = now;
        info.ends_by = now.checked_add(job.limit).unwrap_or(now);
        info.state = JobState::Running;
        let event = Event::JobStarted {
            turn_id: info.turn_id,
            call_id: info.call_id,
            job: info.clone(),
            launcher_pid: process.pid,
            launcher_start: process.start,
        };
        let key =
            JobKey { conversation: info.conversation_id, number: info.id.number().unwrap_or(0) };
        let recorded = self.inner.writer.append(Batch::new().event(key.conversation, event)).await;
        let (stop, stop_rx) = watch::channel(None);
        let (ended_tx, ended) = watch::channel(None);
        let meta = RunningJob {
            key,
            call: info.call_id,
            turn: info.turn_id,
            guard_roots: job.guard_roots.clone(),
            write_roots: job.write_roots.clone(),
            started_at: now,
            contained: job.contained,
            call_dir: job.call_dir.clone(),
            state_dir: job.state_dir.clone(),
            moved: false,
        };
        let launch::Spawned { child, pid, pidfd, lifeline, .. } = process;
        let entry = Entry {
            info: info.clone(),
            meta,
            output: output.clone(),
            stop,
            ended,
            lifeline: Some(lifeline),
            kept,
            kept_taken: false,
            full: job.full,
            guard: Vec::new(),
            proxy,
            bus,
            forward: None,
        };
        // NOTE: counted before the job is in the table, so no reload between the two
        // finds the job running and not counted.
        if job.full {
            self.inner.full.begin_job(key);
        }
        self.lock().running.insert(key, entry);
        // NOTE: the forwards start once the job is in the table, so each forward of the
        // job reaches its entry; a job that ended meanwhile drops them at once.
        if let Some(socket) = socket {
            let forward = forward::serve(self.clone(), key, socket);
            if let Some(entry) = self.lock().running.get_mut(&key) {
                entry.forward = Some(forward);
            }
        }
        // NOTE: a reload that turned full mode off before the job was in the table did
        // not find it there; it marked full mode off first, so the job stops here.
        if job.full && self.inner.full.is_off() {
            self.request_stop(key, JobStopBy::FullOff);
        }
        let watching = (job.matcher.is_some() && job.wakes > 0)
            .then(|| tokio::spawn(self.clone().follow_matches(info.clone(), matches, job.wakes)));
        let task = JobTask {
            table: self.clone(),
            key,
            process: JobProcess::Launcher { child, pidfd },
            pid,
            output,
            stop: stop_rx,
            ended: ended_tx,
            ends_by: info.ends_by,
            result_dir: Some(job.call_dir.clone()),
            dirs: vec![job.call_dir, job.state_dir],
            output_path: info.output.clone(),
            watching,
            left: Vec::new(),
        };
        tokio::spawn(task.run());
        match recorded {
            Ok(_) => Ok(info),
            Err(source) => {
                // NOTE: a job that the store did not take must not run unseen.
                let _ = self.stop(key, JobStopBy::DaemonStop).await;
                Err(DaemonError::from(source))
            }
        }
    }

    /// Makes the running call `run`, which the user moved with Ctrl+B, the job `job` in
    /// the place `slot` (DS22): records `job_started` with the shell's pid and start
    /// time as the launcher's, writes the output so far and then the rest of the
    /// command's output to the job's output file, and starts the task that follows the
    /// job to its end. On any failure the command stops, and its shell closes.
    pub(crate) async fn adopt(
        &self,
        mut run: efr_shell::AdoptedRun,
        slot: Slot,
        mut job: AdoptedJob,
    ) -> Result<JobInfo, DaemonError> {
        let pid = run.shell_pid();
        let start = tokio::task::spawn_blocking(move || sweep::start_time(pid))
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        let now = self.inner.clock.now();
        job.info.started_at = now;
        job.info.ends_by = now.checked_add(job.limit).unwrap_or(now);
        job.info.state = JobState::Running;
        let info = job.info.clone();
        // NOTE: a line typed into the hidden shell had no prepare that made the dir.
        let scratch = job.scratch.clone();
        let made = tokio::task::spawn_blocking(move || output::make_dir(&scratch)).await;
        if !matches!(made, Ok(Ok(()))) {
            tracing::warn!(job = %info.id, "the jobs dir of a moved call could not be made");
        }
        let file = match output::OutputFile::create(
            &job.scratch,
            &output::file_name(&info.id),
            output::Limits::DEFAULT,
        ) {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::warn!(%error, job = %info.id, "a job's output file could not be made; its output is not kept");
                None
            }
        };
        // NOTE: the output so far first, as the call's result has it, then the rest as
        // plain text from the shell's tee (DS21).
        let mut head = run.head().as_bytes().to_vec();
        if head.last().is_some_and(|byte| *byte != b'\n') {
            head.push(b'\n');
        }
        let head = std::io::Cursor::new(head);
        let reader: Box<dyn std::io::Read + Send> = match run.take_output() {
            Some(rest) => Box::new(std::io::Read::chain(head, rest)),
            None => Box::new(head),
        };
        let (send, _matches) = mpsc::unbounded_channel();
        let output = match output::spawn_reader(info.id.as_str(), reader, file, None, send) {
            Ok(output) => output,
            Err(source) => {
                stop_adopted(&run).await;
                return Err(DaemonError::Io { path: info.output.clone(), source });
            }
        };
        let key =
            JobKey { conversation: info.conversation_id, number: info.id.number().unwrap_or(0) };
        self.lock().launchers.insert(pid);
        let event = Event::JobStarted {
            turn_id: info.turn_id,
            call_id: info.call_id,
            job: info.clone(),
            launcher_pid: pid,
            launcher_start: start,
        };
        let recorded = self.inner.writer.append(Batch::new().event(key.conversation, event)).await;
        let (stop, stop_rx) = watch::channel(None);
        let (ended_tx, ended) = watch::channel(None);
        let meta = RunningJob {
            key,
            call: info.call_id,
            turn: info.turn_id,
            guard_roots: job.guard_roots,
            write_roots: job.write_roots,
            started_at: now,
            contained: job.contained,
            call_dir: job.call_dir.clone().unwrap_or_default(),
            state_dir: PathBuf::new(),
            moved: true,
        };
        let entry = Entry {
            info: info.clone(),
            meta,
            output: output.clone(),
            stop,
            ended,
            lifeline: None,
            kept: None,
            kept_taken: false,
            full: job.full,
            guard: Vec::new(),
            proxy: job.proxy,
            bus: job.bus,
            forward: None,
        };
        if job.full {
            self.inner.full.begin_job(key);
        }
        self.lock().running.insert(key, entry);
        drop(slot);
        if job.full && self.inner.full.is_off() {
            self.request_stop(key, JobStopBy::FullOff);
        }
        let task = JobTask {
            table: self.clone(),
            key,
            process: JobProcess::Adopted { run: Box::new(run), end: None },
            pid,
            output,
            stop: stop_rx,
            ended: ended_tx,
            ends_by: info.ends_by,
            result_dir: job.call_dir.clone(),
            dirs: job.call_dir.into_iter().collect(),
            output_path: info.output.clone(),
            watching: None,
            left: Vec::new(),
        };
        tokio::spawn(task.run());
        tracing::info!(conversation = %key.conversation, job = %info.id, "a running call moved into a background job");
        match recorded {
            Ok(_) => Ok(info),
            Err(source) => {
                // NOTE: a job that the store did not take must not run unseen.
                let _ = self.stop(key, JobStopBy::DaemonStop).await;
                Err(DaemonError::from(source))
            }
        }
    }

    /// Hands the job `job` of a moved call the snapshot of its roots right after the
    /// move (DS28): at the job's end, what its command wrote since joins the turn that
    /// started the call. When the job's end came first, the snapshot joins the turn
    /// now.
    pub(crate) async fn keep_snapshot(&self, job: &JobInfo, kept: KeptSnapshot) {
        let key =
            JobKey { conversation: job.conversation_id, number: job.id.number().unwrap_or(0) };
        let late = {
            let mut guard = self.lock();
            match guard.running.get_mut(&key) {
                Some(entry) if !entry.kept_taken => {
                    entry.kept = Some(kept);
                    None
                }
                _ => Some(kept),
            }
        };
        if let Some((snapshot, limits)) = late {
            join_start_turn(snapshot, limits, job.started_at).await;
        }
    }

    /// Records each watch match of the job `started` while it has `wakes` left, and
    /// hands its notice to the actor. It ends when the reader of the output ended, so
    /// the job's end comes after every match.
    async fn follow_matches(
        self,
        started: JobInfo,
        mut matches: mpsc::UnboundedReceiver<String>,
        mut wakes: u8,
    ) {
        let key = JobKey {
            conversation: started.conversation_id,
            number: started.id.number().unwrap_or(0),
        };
        let mut info = started;
        let cancel = self.inner.cancel.clone();
        while wakes > 0 {
            let line = tokio::select! {
                () = cancel.cancelled() => return,
                line = matches.recv() => match line {
                    Some(line) => line,
                    None => return,
                },
            };
            wakes -= 1;
            info.wakes = info.wakes.saturating_add(1);
            self.update(key, |live| live.wakes = live.wakes.saturating_add(1));
            let info = info.clone();
            let event = Event::JobWatchMatched {
                job_id: info.id.clone(),
                line: line.clone(),
                wakes_left: wakes,
            };
            let committed =
                self.inner.writer.append(Batch::new().event(key.conversation, event)).await;
            let seq = match committed {
                Ok(committed) => last_seq(&committed),
                Err(error) => {
                    tracing::warn!(%error, job = %info.id, "a watch match could not be recorded");
                    continue;
                }
            };
            let body = NoticeBody::Watch { line: &line, wakes_left: wakes };
            let text = notice_text(&info, &info.output.display().to_string(), body);
            self.notify(
                key.conversation,
                JobNoticeInput::new(info, NoticeKind::Watch, seq, text, true),
            );
        }
    }

    /// Changes the running job `key` with `change` and returns it; `None` when it
    /// does not run.
    fn update(&self, key: JobKey, change: impl FnOnce(&mut JobInfo)) -> Option<JobInfo> {
        let mut table = self.lock();
        let entry = table.running.get_mut(&key)?;
        change(&mut entry.info);
        Some(entry.info.clone())
    }

    fn notify(&self, conversation: ConversationId, input: JobNoticeInput) {
        if self.lock().stopping {
            return;
        }
        let _ = self.inner.notices.send(Notice { conversation, input });
    }

    /// The running jobs.
    pub(crate) fn running(&self) -> Vec<RunningJob> {
        self.lock().running.values().map(|entry| entry.meta.clone()).collect()
    }

    /// The running job `key` as it stands now, with the bytes of its output so far.
    pub(crate) fn live(&self, key: JobKey) -> Option<JobInfo> {
        let table = self.lock();
        let entry = table.running.get(&key)?;
        let mut info = entry.info.clone();
        info.output_bytes = entry.output.bytes();
        Some(info)
    }

    /// The running jobs of `conversation`, or of every conversation.
    fn live_of(&self, conversation: Option<ConversationId>) -> Vec<JobInfo> {
        let table = self.lock();
        table
            .running
            .values()
            .filter(|entry| conversation.is_none_or(|id| entry.meta.key.conversation == id))
            .map(|entry| {
                let mut info = entry.info.clone();
                info.output_bytes = entry.output.bytes();
                info
            })
            .collect()
    }

    /// The pids that keep a socket peer model-side: each launcher that runs, and each
    /// session of a job that ended while a process still had it.
    pub(crate) fn model_side(&self) -> Vec<u32> {
        let table = self.lock();
        table.launchers.iter().chain(table.sessions.iter()).copied().collect()
    }

    /// The jobs of `conversation`, or of every conversation, newest first, at most
    /// 200: the store's record, with the running ones as they stand now.
    pub(crate) async fn list(
        &self,
        conversation: Option<ConversationId>,
    ) -> Result<Vec<JobInfo>, DaemonError> {
        let rows = self
            .inner
            .readers
            .with(move |conn| match conversation {
                Some(id) => efr_store::jobs::of_conversation(conn, id, MAX_LIST),
                None => efr_store::jobs::all(conn, MAX_LIST),
            })
            .await?;
        let live = self.live_of(conversation);
        Ok(rows
            .into_iter()
            .map(|row| {
                live.iter()
                    .find(|job| {
                        job.conversation_id == row.info.conversation_id && job.id == row.info.id
                    })
                    .cloned()
                    .unwrap_or(row.info)
            })
            .collect())
    }

    /// The job `key`: as it runs now, else as the store recorded it.
    pub(crate) async fn find(&self, key: JobKey) -> Result<Option<JobInfo>, DaemonError> {
        if let Some(info) = self.live(key) {
            return Ok(Some(info));
        }
        let jobs = self.list(Some(key.conversation)).await?;
        Ok(jobs.into_iter().find(|job| job.id.number() == Some(key.number)))
    }

    /// Asks the running job `key` to stop with the cause `by`, unless a stop was asked
    /// before; returns the watch of its end, or `None` when it does not run.
    fn request_stop(&self, key: JobKey, by: JobStopBy) -> Option<watch::Receiver<Option<JobInfo>>> {
        let table = self.lock();
        table.running.get(&key).map(|entry| {
            ask_stop(entry, by);
            entry.ended.clone()
        })
    }

    /// Asks every running job that a call of full mode started to stop with
    /// `full_off`, and returns their keys; it does not wait for their ends. A reload
    /// calls it after it installed settings that turn full mode off, and after it
    /// marked full mode off for [`begin`](Self::begin) (`reload.rs`, DV37). A job of
    /// another mode runs on.
    pub(crate) fn stop_full(&self) -> Vec<JobKey> {
        let table = self.lock();
        let mut stopped: Vec<JobKey> = table
            .running
            .iter()
            .filter(|(_, entry)| entry.full)
            .map(|(key, entry)| {
                ask_stop(entry, JobStopBy::FullOff);
                *key
            })
            .collect();
        stopped.sort_by_key(|key| (key.conversation, key.number));
        stopped
    }

    /// Stops the job `key` with the cause `by` and waits for its end. A job that ended
    /// already keeps its final state; an unknown job is `JobNotFound`.
    pub(crate) async fn stop(&self, key: JobKey, by: JobStopBy) -> Result<JobInfo, DaemonError> {
        let ended = self.request_stop(key, by);
        if let Some(mut ended) = ended
            && let Ok(done) = ended.wait_for(Option::is_some).await
            && let Some(info) = done.clone()
        {
            return Ok(info);
        }
        match self.find(key).await? {
            Some(info) => Ok(info),
            None => Err(DaemonError::JobNotFound {
                conversation_id: key.conversation,
                job_id: JobId::nth(key.number),
            }),
        }
    }

    /// At efrd's stop: stops every job with `daemon_stop`, records each end with no
    /// notice, then cancels the job tasks. It waits at most [`STOP_GRACE`] in all for
    /// the launchers, and the sweep after.
    pub(crate) async fn stop_all(&self) {
        let keys: Vec<JobKey> = {
            let mut table = self.lock();
            table.stopping = true;
            table.running.keys().copied().collect()
        };
        let stops = keys.into_iter().map(|key| self.stop(key, JobStopBy::DaemonStop));
        let all = futures::future::join_all(stops);
        // NOTE: each stop escalates on its own after STOP_GRACE; the margin covers the
        // sweep and the record.
        if self
            .inner
            .clock
            .timeout(STOP_GRACE + OUTPUT_GRACE + Duration::from_secs(2), all)
            .await
            .is_err()
        {
            tracing::warn!("a job did not end in time at efrd's stop");
        }
        self.inner.cancel.cancel();
    }

    /// Looks every [`SCAN_INTERVAL`] whether the sessions of ended jobs still have
    /// processes, and forgets those that have none.
    pub(crate) async fn scan(self) {
        let cancel = self.inner.cancel.clone();
        loop {
            tokio::select! {
                () = cancel.cancelled() => return,
                () = self.inner.clock.sleep(SCAN_INTERVAL) => {}
            }
            self.forget_empty_sessions().await;
        }
    }

    async fn forget_empty_sessions(&self) {
        let sessions: Vec<u32> = self.lock().sessions.iter().copied().collect();
        if sessions.is_empty() {
            return;
        }
        let empty = tokio::task::spawn_blocking(move || {
            sessions.into_iter().filter(|session| !sweep::has_members(*session)).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let mut table = self.lock();
        for session in empty {
            table.sessions.remove(&session);
        }
    }

    /// At start, before any job runs: ends each job that the last efrd left running
    /// with `daemon_restart`, and removes every job dir. Returns each `job_ended` that
    /// it recorded, with its conversation, for [`restart_notices`].
    pub(crate) async fn recover(&self) -> Result<Vec<(ConversationId, Event)>, DaemonError> {
        self.recover_with(sweep::sweep_left).await
    }

    /// [`recover`](Self::recover) with `sweeper` in place of [`sweep::sweep_left`].
    async fn recover_with(
        &self,
        sweeper: fn(u32, u64, i64) -> Vec<String>,
    ) -> Result<Vec<(ConversationId, Event)>, DaemonError> {
        let rows = self.inner.readers.with(efr_store::jobs::running).await?;
        let dirs = self.inner.dirs.clone();
        let mut ended = Vec::new();
        for row in rows {
            let info = row.info;
            let pid = row.launcher_pid;
            let start = row.launcher_start;
            let started_at = info.started_at.as_second();
            // NOTE: the pid and its start time come from the event, never from a file
            // that a job could write.
            let swept = tokio::task::spawn_blocking(move || sweeper(pid, start, started_at))
                .await
                .unwrap_or_default();
            // NOTE: a process that outlived the sweep has the job's session: it stays
            // model-side until the scan finds the session empty, as after `finish`.
            if pid > 0 && !swept.is_empty() {
                self.lock().sessions.insert(pid);
            }
            // NOTE: a call that the user moved into a job keeps the dir of its call
            // (DS32); its end guard ran in its launcher, or it is lost with it.
            let moved = info.moved_from.is_some();
            let call_dir = if moved {
                moved_call_dir(&dirs, info.conversation_id, info.call_id)
            } else {
                job_call_dir(&dirs, info.conversation_id, info.call_id)
            };
            let result = read_result(&call_dir).await;
            let contained = matches!(info.launch, Launch::Contained { .. });
            // NOTE: a moved line that ran outside the sandbox may have been typed into
            // the hidden shell, with no launcher to lose.
            let launcher_lost = result.is_none() && (!moved || contained);
            // NOTE: a contained job that left no result had no end guard: its guard
            // check runs now, before its state dir goes.
            let lost_guard = if launcher_lost && contained && !moved {
                let state_dir = job_state_dir(&dirs, info.conversation_id, info.call_id);
                self.check_lost(call_dir.clone(), state_dir).await.changes
            } else {
                Vec::new()
            };
            let mut summary = result.map(|result| result.summary).unwrap_or_default();
            summary.surface_changes = with_guarded(summary.surface_changes, lost_guard);
            let mut survivors = summary.survivors;
            survivors.extend(swept);
            survivors.sort();
            survivors.dedup();
            let path = info.output.clone();
            let (bytes, tail) = tokio::task::spawn_blocking(move || read_tail(&path))
                .await
                .unwrap_or((0, String::new()));
            let now = self.inner.clock.now();
            let event = Event::JobEnded {
                job_id: info.id.clone(),
                state: JobState::Stopped,
                exit_code: None,
                stopped_by: Some(JobStopBy::DaemonRestart),
                output_bytes: bytes.max(info.output_bytes),
                ran_secs: ran_secs(info.started_at, now),
                tail,
                quarantined: summary.surface_changes.iter().any(|change| change.quarantined),
                surface_changes: summary.surface_changes,
                survivors,
                launcher_lost,
                output_truncated: true,
                blocked: Vec::new(),
                blocked_more: 0,
                bus_refused: Vec::new(),
                bus_refused_more: 0,
            };
            let batch = Batch::new().event(info.conversation_id, event.clone());
            self.inner.writer.append(batch).await?;
            tracing::info!(conversation = %info.conversation_id, job = %info.id, "a job of the last efrd was recorded as stopped");
            ended.push((info.conversation_id, event));
        }
        let removed = tokio::task::spawn_blocking(move || remove_all_job_dirs(&dirs)).await;
        if removed.is_err() {
            tracing::warn!("removing the job dirs of the last efrd panicked");
        }
        Ok(ended)
    }

    /// The notices of the last day that no turn took, each for its conversation and
    /// with `wakes: false`.
    pub(crate) async fn undelivered(&self) -> Result<Vec<Notice>, DaemonError> {
        let now = self.inner.clock.now();
        let since = now.checked_sub(UNDELIVERED_WINDOW).unwrap_or(now);
        let found = self
            .inner
            .readers
            .with(move |conn| efr_store::jobs::undelivered_notices(conn, since))
            .await?;
        Ok(found
            .into_iter()
            .filter_map(|notice| {
                let job = notice.job;
                let shown = job.output.display().to_string();
                let (kind, text) = match &notice.event {
                    Event::JobEnded {
                        state,
                        exit_code,
                        stopped_by,
                        tail,
                        blocked,
                        blocked_more,
                        bus_refused,
                        bus_refused_more,
                        ..
                    } => {
                        let body = NoticeBody::Ended {
                            state: *state,
                            exit_code: *exit_code,
                            stopped_by: *stopped_by,
                            tail,
                        };
                        // NOTE: the limit of a job of an earlier efrd is not known here;
                        // the note names the key without the number.
                        let refused = joined_notes(
                            blocked_note(blocked, *blocked_more, 0),
                            bus_note(bus_refused, *bus_refused_more, false),
                        );
                        let text = notice_text_refused(&job, &shown, body, refused.as_deref());
                        (NoticeKind::Exited, text)
                    }
                    Event::JobWatchMatched { line, wakes_left, .. } => {
                        let body = NoticeBody::Watch { line, wakes_left: *wakes_left };
                        (NoticeKind::Watch, notice_text(&job, &shown, body))
                    }
                    _ => return None,
                };
                let input = JobNoticeInput::new(job, kind, notice.source_seq, text, false);
                Some(Notice { conversation: notice.conversation_id, input })
            })
            .collect())
    }

    /// Drops the lifeline's write end of `key`, as efrd's death would: its launcher
    /// stops the job. For a test of the lifeline.
    #[cfg(test)]
    pub(crate) fn drop_lifeline(&self, key: JobKey) {
        if let Some(entry) = self.lock().running.get_mut(&key) {
            entry.lifeline = None;
        }
    }

    /// Forgets the session `session` once no process has it; for a test of the scan.
    #[cfg(test)]
    pub(crate) async fn scan_once(&self) {
        self.forget_empty_sessions().await;
    }

    /// [`recover`](Self::recover) with `sweeper` in place of the real sweep; for a test.
    #[cfg(test)]
    pub(crate) async fn recover_by(
        &self,
        sweeper: fn(u32, u64, i64) -> Vec<String>,
    ) -> Result<Vec<(ConversationId, Event)>, DaemonError> {
        self.recover_with(sweeper).await
    }

    /// Keeps `session` model-side as a session of a job that ended; for a test.
    #[cfg(test)]
    pub(crate) fn keep_session(&self, session: u32) {
        self.lock().sessions.insert(session);
    }
}

/// The task that follows one job to its end.
struct JobTask {
    table: JobTable,
    key: JobKey,
    process: JobProcess,
    /// The launcher's pid, or the adopted shell's: the session that the end sweeps.
    pid: u32,
    output: output::Output,
    stop: watch::Receiver<Option<JobStopBy>>,
    ended: watch::Sender<Option<JobInfo>>,
    ends_by: Timestamp,
    /// The dir of the launcher's `result.json`; `None` for a line that a moved call
    /// typed into the hidden shell, which has no launcher.
    result_dir: Option<PathBuf>,
    /// The dirs that go at the end: the call dir and the state dir of a job, the call
    /// dir only of a moved call (its shell dir with `state.json` stays).
    dirs: Vec<PathBuf>,
    output_path: PathBuf,
    /// The task that records the job's watch matches.
    watching: Option<tokio::task::JoinHandle<()>>,
    /// The processes below a moved command's shell that a close before the end could
    /// not kill.
    left: Vec<String>,
}

/// What runs a job, and the rules of its end (DS22).
enum JobProcess {
    /// The launcher that efrd spawned for the job.
    Launcher { child: tokio::process::Child, pidfd: Option<std::os::fd::OwnedFd> },
    /// A running call that the user moved into the job, in its old hidden shell.
    Adopted {
        run: Box<efr_shell::AdoptedRun>,
        /// How the command ended, once it did.
        end: Option<Box<efr_shell::AdoptedEnd>>,
    },
}

impl JobProcess {
    /// Waits until the job's command ends: its launcher, or the moved command.
    async fn wait(&mut self, key: JobKey) {
        match self {
            JobProcess::Launcher { child, .. } => {
                if let Err(error) = child.wait().await {
                    tracing::warn!(%error, job = key.number, "waiting for a job's launcher failed");
                }
            }
            JobProcess::Adopted { run, end } => {
                if let Ok(ended) = run.ended().await {
                    *end = Some(Box::new(ended));
                }
            }
        }
    }
}

impl JobTask {
    async fn run(mut self) {
        let clock = Arc::clone(&self.table.inner.clock);
        let mut stop_rx = self.stop.clone();
        // NOTE: from the time limit, not from now: a clock that moved before this task
        // first ran still ends the job at its limit.
        let left = self.ends_by.duration_since(clock.now());
        let left = Duration::try_from(left).unwrap_or(Duration::ZERO);
        let key = self.key;
        let looks = match &self.process {
            JobProcess::Adopted { run, .. } => Some(run.looks()),
            JobProcess::Launcher { .. } => None,
        };
        let requested = tokio::select! {
            () = self.process.wait(key) => Wakeup::Ended,
            stop = stop_rx.wait_for(Option::is_some) => {
                stop.ok().and_then(|stop| *stop).map_or(Wakeup::Ended, Wakeup::Stop)
            }
            () = clock.sleep(left) => Wakeup::Stop(JobStopBy::TimeLimit),
            () = secret_wait(looks) => Wakeup::Secret,
        };
        let stopped_by = match requested {
            Wakeup::Stop(by) => {
                // NOTE: the first stop wins: a stop and the limit at once keep the stop.
                if let Some(entry) = self.table.lock().running.get(&self.key) {
                    ask_stop(entry, by);
                }
                let by = (*self.stop.borrow()).unwrap_or(by);
                self.escalate(&clock, true).await;
                Some(by)
            }
            // NOTE: efr-shell interrupted the command, which waits for a secret that
            // nobody can type in a job; the job closes the shell when the command did
            // not end, after it killed what is below the shell (DS55).
            Wakeup::Secret => {
                self.escalate(&clock, false).await;
                None
            }
            Wakeup::Ended => None,
        };
        self.finish(stopped_by).await;
    }

    /// A launcher: SIGCONT and SIGTERM, SIGKILL after [`STOP_GRACE`]. A moved command:
    /// SIGINT to the job in the shell's foreground when `interrupt` (a launcher then
    /// writes its result), and the shell closes when the command did not end within
    /// [`STOP_GRACE`], after every process below it was killed (DS55).
    async fn escalate(&mut self, clock: &Arc<dyn Clock>, interrupt: bool) {
        let (key, pid) = (self.key, self.pid);
        match &mut self.process {
            JobProcess::Launcher { child, pidfd } => {
                launch::signal(pidfd.as_ref(), pid, Signal::CONT);
                launch::signal(pidfd.as_ref(), pid, Signal::TERM);
                if clock.timeout(STOP_GRACE, child.wait()).await.is_err() {
                    tracing::warn!(
                        job = key.number,
                        "a job's launcher did not end after SIGTERM; killing it"
                    );
                    launch::signal(pidfd.as_ref(), pid, Signal::KILL);
                    let _ = child.wait().await;
                }
            }
            JobProcess::Adopted { run, end } => {
                if interrupt && let Err(error) = run.interrupt().await {
                    tracing::warn!(%error, job = key.number, "could not interrupt a moved command");
                }
                match clock.timeout(STOP_GRACE, run.ended()).await {
                    Ok(Ok(ended)) => *end = Some(Box::new(ended)),
                    Ok(Err(_)) => {}
                    Err(_) => {
                        tracing::warn!(
                            job = key.number,
                            "a moved command did not end after SIGINT; closing its shell"
                        );
                        self.left = close_adopted(run).await;
                    }
                }
            }
        }
    }

    /// Ends what is left of the job's processes and returns the names of those that
    /// are still there: the launcher's session; for a moved command every process below
    /// its shell (DS55), then the shell itself and its session.
    async fn sweep(&self) -> Vec<String> {
        let session = self.pid;
        let JobProcess::Adopted { run, .. } = &self.process else {
            return tokio::task::spawn_blocking(move || sweep::kill_session(session))
                .await
                .unwrap_or_default();
        };
        let below = close_adopted(run).await;
        let mut left = tokio::task::spawn_blocking(move || sweep::kill_session(session))
            .await
            .unwrap_or_default();
        left.extend(below);
        left.extend(self.left.iter().cloned());
        left.sort();
        left.dedup();
        left
    }

    /// Everything after the launcher ended: the sweep, the rest of the output, the
    /// result, the record, the notice and the dirs.
    async fn finish(mut self, stopped_by: Option<JobStopBy>) {
        let table = &self.table;
        let clock = Arc::clone(&table.inner.clock);
        let (lifeline, guarded, info, proxy, bus, forward) = {
            let mut guard = table.lock();
            match guard.running.get_mut(&self.key) {
                Some(entry) => (
                    entry.lifeline.take(),
                    std::mem::take(&mut entry.guard),
                    Some(entry.info.clone()),
                    entry.proxy.take(),
                    entry.bus.take(),
                    entry.forward.take(),
                ),
                None => (None, Vec::new(), None, None, None, None),
            }
        };
        drop(lifeline);
        // NOTE: the forwards end with the job (DL11): the socket closes before the
        // job's dirs go.
        drop(forward);
        let Some(info) = info else { return };
        let swept = self.sweep().await;
        let output_ended = clock.timeout(OUTPUT_GRACE, self.output.ended()).await.is_ok();
        if !output_ended {
            tracing::info!(job = %info.id, "a process of the job still holds its output; the end takes the output so far");
        }
        if let Some(watching) = self.watching.take() {
            // NOTE: the matches of the output that the reader read come before the end.
            if output_ended {
                let _ = watching.await;
            } else {
                watching.abort();
            }
        }
        let info = table.lock().running.get(&self.key).map_or(info, |entry| entry.info.clone());
        // NOTE: the launcher ended, and the bridge with it; the socket closes before the
        // job's dirs go (DP14), and its refusals reach the record and the notice (DP12).
        let (blocked, blocked_more, refused) = match proxy {
            Some(proxy) => {
                let limit = proxy.max_upload_bytes();
                let report = proxy.close(crate::sandbox::proxy::CLOSE_GRACE).await;
                let note = blocked_note(&report.blocked, report.blocked_more, limit);
                (report.blocked, report.blocked_more, note)
            }
            None => (Vec::new(), 0, None),
        };
        // NOTE: the bus filters end with the job, before its dirs go (DE1); their
        // refusals reach the record and the notice (DE11), and a filter that died
        // reaches the notice (DE8).
        let bus = match bus {
            Some(bus) => bus.finish().await,
            None => BusReport::default(),
        };
        let refused = joined_notes(refused, bus_note(&bus.refused, bus.more, bus.stopped));
        let (refused, result, launcher_lost, ended) = match &self.process {
            JobProcess::Launcher { .. } => {
                let result = match &self.result_dir {
                    Some(dir) => read_result(dir).await,
                    None => None,
                };
                let ended = end_of(result.as_ref(), stopped_by);
                (refused, result.clone(), result.is_none(), ended)
            }
            JobProcess::Adopted { run, end } => {
                // NOTE: nobody follows a job, so efr-shell stopped a wait for a secret at
                // once (DS27); the notice says why the command ended.
                let looked = *run.looks().borrow();
                let secret = is_secret(looked);
                let refused = if secret {
                    joined_notes(Some(SECRET_STOP.to_owned()), refused)
                } else {
                    refused
                };
                match &self.result_dir {
                    Some(dir) => {
                        let result = match read_result(dir).await {
                            Some(result) => Some(result),
                            None => end.as_ref().and_then(|end| end.sandbox.clone()),
                        };
                        let ended = end_of(result.as_ref(), stopped_by);
                        (refused, result.clone(), result.is_none(), ended)
                    }
                    // NOTE: a line typed into the hidden shell has no launcher, so none
                    // was lost; its `D` mark carries its exit status.
                    None => {
                        let ended = match (stopped_by, end.as_ref().and_then(|end| end.exit_code)) {
                            (None, Some(code)) => Ended {
                                state: JobState::Exited,
                                exit_code: Some(code),
                                stopped_by: None,
                            },
                            (stopped_by, _) => end_of(None, stopped_by),
                        };
                        (refused, None, false, ended)
                    }
                }
            }
        };
        let summary = result.map(|result| result.summary).unwrap_or_default();
        let mut survivors = summary.survivors;
        survivors.extend(swept.iter().cloned());
        survivors.sort();
        survivors.dedup();
        let now = clock.now();
        let mut final_info = info.clone();
        final_info.state = ended.state;
        final_info.exit_code = ended.exit_code;
        final_info.stopped_by = ended.stopped_by;
        final_info.output_bytes = self.output.bytes();
        final_info.ended_at = Some(now);
        // NOTE: the forwards ended with the job.
        final_info.forwards.clear();
        let tail = self.output.tail();
        // NOTE: the changes that a guard check moved while the job ran are gone from
        // the surface, so the launcher's end guard no longer sees them.
        let surface_changes = with_guarded(summary.surface_changes, guarded);
        let event = Event::JobEnded {
            job_id: info.id.clone(),
            state: ended.state,
            exit_code: ended.exit_code,
            stopped_by: ended.stopped_by,
            output_bytes: final_info.output_bytes,
            ran_secs: ran_secs(info.started_at, now),
            tail: tail.clone(),
            quarantined: surface_changes.iter().any(|change| change.quarantined),
            surface_changes,
            survivors,
            launcher_lost,
            output_truncated: self.output.truncated(),
            blocked,
            blocked_more,
            bus_refused: bus.refused,
            bus_refused_more: bus.more,
        };
        // NOTE: the launcher ended, so its pid leaves the model-side list before the
        // record: a reader that sees `JobEnded` sees the list without it. A session that
        // a process still had stays listed.
        {
            let mut guard = table.lock();
            guard.launchers.remove(&self.pid);
            if !swept.is_empty() {
                guard.sessions.insert(self.pid);
            }
        }
        let committed =
            table.inner.writer.append(Batch::new().event(self.key.conversation, event)).await;
        // NOTE: before the job leaves the table, so an undo waits until the job's files
        // joined the turn that started it. Taken this late, so the snapshot of a moved
        // call's job, which comes after the move, is here unless the job ended first.
        let kept = table.lock().running.get_mut(&self.key).and_then(|entry| {
            entry.kept_taken = true;
            entry.kept.take()
        });
        if let Some((snapshot, limits)) = kept {
            join_start_turn(snapshot, limits, info.started_at).await;
        }
        table.lock().running.remove(&self.key);
        table.inner.full.end_job(self.key);
        remove_dirs(std::mem::take(&mut self.dirs)).await;
        match committed {
            Ok(committed) => {
                let seq = last_seq(&committed);
                let body = NoticeBody::Ended {
                    state: ended.state,
                    exit_code: ended.exit_code,
                    stopped_by: ended.stopped_by,
                    tail: &tail,
                };
                let shown = self.output_path.display().to_string();
                let text = notice_text_refused(&final_info, &shown, body, refused.as_deref());
                // NOTE: the model that stopped a job read its output in the tool result:
                // the notice rides in the next turn and uses none of the hour's wake
                // turns. So does the notice of a job that efrd stopped because full mode
                // went off: a wake turn would run with the mode that the user just took
                // away. Every other end wakes the model, also a stop by the user.
                let wakes =
                    !matches!(ended.stopped_by, Some(JobStopBy::Model | JobStopBy::FullOff));
                let input =
                    JobNoticeInput::new(final_info.clone(), NoticeKind::Exited, seq, text, wakes);
                table.notify(self.key.conversation, input);
            }
            Err(error) => {
                tracing::warn!(%error, job = %info.id, "the end of a job could not be recorded");
            }
        }
        tracing::info!(conversation = %self.key.conversation, job = %info.id, state = ended.state.as_str(), "a job ended");
        let _ = self.ended.send(Some(final_info));
    }
}

/// The note of a job's end when efr stopped its moved command because it waited for a
/// password that nobody could type (DS27).
const SECRET_STOP: &str =
    "[efr stopped the command: it waited for a password, and nobody can type one in a job]";

/// Writes the notice line of each job end of `ended`, which [`JobTable::recover`]
/// recorded at this start, to the notice file of its conversation's terminal under
/// `dir`, as the reconciliation writes the notice of a prompt that did not run. No
/// client is connected yet, so no terminal shows the end, and the notices task, which
/// starts later, never sees these events.
pub(crate) async fn restart_notices(
    readers: &Readers,
    ended: Vec<(ConversationId, Event)>,
    dir: &Path,
) {
    let mut lines = Vec::new();
    for (conversation_id, event) in ended {
        let summary =
            readers.with(move |conn| efr_store::conversations::get(conn, conversation_id)).await;
        let summary = match summary {
            Ok(Some(summary)) => summary,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, %conversation_id, "the notice of a job that efrd ended could not be prepared");
                continue;
            }
        };
        let (Some(tty), Some(line)) =
            (summary.tty, crate::notices::line(&event, summary.title.as_deref(), None))
        else {
            continue;
        };
        lines.push((tty, line));
    }
    if lines.is_empty() {
        return;
    }
    let dir = dir.to_path_buf();
    let written = tokio::task::spawn_blocking(move || {
        for (tty, line) in lines {
            if let Err(error) = crate::notices::append(&dir, &tty, &line) {
                tracing::warn!(%error, "the notice of a job that efrd ended could not be written");
            }
        }
    })
    .await;
    if written.is_err() {
        tracing::warn!("writing the notices of jobs that efrd ended panicked");
    }
}

/// Asks the job of `entry` to stop with `by`; the first cause that was asked wins.
fn ask_stop(entry: &Entry, by: JobStopBy) {
    entry.stop.send_if_modified(|stop| {
        if stop.is_none() {
            *stop = Some(by);
            true
        } else {
            false
        }
    });
}

/// The notes of a job's end for the model: the network proxy's, then the bus
/// filter's, one per line.
fn joined_notes(network: Option<String>, bus: Option<String>) -> Option<String> {
    match (network, bus) {
        (Some(network), Some(bus)) => Some(format!("{network}\n{bus}")),
        (network, bus) => network.or(bus),
    }
}

/// Adds the files that a job changed since `snapshot`, the snapshot of its roots right
/// after its start at `started`, to the turn that started it, without the paths that
/// other turns changed meanwhile.
async fn join_start_turn(
    snapshot: efr_snapshot::CallSnapshot,
    limits: efr_snapshot::Limits,
    started: Timestamp,
) {
    let Some(store) = snapshot.snapshots() else {
        snapshot.discard();
        return;
    };
    if let Some(joined) = store.after_job(snapshot, started, limits).await {
        tracing::debug!(
            files = joined.files.len(),
            "a job's files joined the turn that started it"
        );
    }
}

/// How a job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ended {
    pub(crate) state: JobState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stopped_by: Option<JobStopBy>,
}

/// How a job ended, from its launcher's `result` and the stop that efrd asked for.
pub(crate) fn end_of(result: Option<&SandboxResult>, stopped_by: Option<JobStopBy>) -> Ended {
    if let Some(by) = stopped_by {
        let state = if by == JobStopBy::TimeLimit { JobState::TimedOut } else { JobState::Stopped };
        return Ended { state, exit_code: None, stopped_by: Some(by) };
    }
    let Some(result) = result else {
        return Ended { state: JobState::Failed, exit_code: None, stopped_by: None };
    };
    if result.setup_error.is_some() || (!result.started && result.exit_code.is_none()) {
        return Ended { state: JobState::Failed, exit_code: None, stopped_by: None };
    }
    match result.stop {
        Some(efr_sandbox::JobStop::Deadline) => {
            return Ended {
                state: JobState::TimedOut,
                exit_code: None,
                stopped_by: Some(JobStopBy::TimeLimit),
            };
        }
        Some(_) => {
            return Ended {
                state: JobState::Stopped,
                exit_code: None,
                stopped_by: Some(JobStopBy::Unknown),
            };
        }
        None => {}
    }
    let exit_code = result.exit_code.or(result.signal.map(|signal| 128 + signal));
    Ended { state: JobState::Exited, exit_code, stopped_by: None }
}

/// The sequence number of the last event of `committed`.
fn last_seq(committed: &efr_store::Committed) -> Seq {
    committed.events().last().map_or(Seq::new(0), |envelope| envelope.seq)
}

/// Whole seconds from `from` to `to`.
fn ran_secs(from: Timestamp, to: Timestamp) -> u64 {
    u64::try_from(to.as_second().saturating_sub(from.as_second())).unwrap_or(0)
}

/// `1 job runs` or `<count> jobs run`.
fn jobs_run(count: usize) -> String {
    if count == 1 { "1 job runs".to_owned() } else { format!("{count} jobs run") }
}

/// One line about `job` for the model: its id, state, command, time left or end, and
/// output size.
pub(crate) fn job_line(job: &JobInfo, now: Timestamp) -> String {
    let command: String = {
        let flat = job.command.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() > 80 {
            let mut cut: String = flat.chars().take(80).collect();
            cut.push_str("...");
            cut
        } else {
            flat
        }
    };
    let when = match job.state {
        JobState::Running => {
            let left = job.ends_by.as_second().saturating_sub(now.as_second()).max(0);
            format!("{} minutes left", (left + 59) / 60)
        }
        _ => {
            let mut text = job.state.as_str().replace('_', " ");
            if let Some(code) = job.exit_code {
                text.push_str(&format!(" with exit {code}"));
            }
            if let Some(by) = job.stopped_by {
                text.push_str(&format!(" by {}", by.as_str().replace('_', " ")));
            }
            text
        }
    };
    let ports = match job.state {
        JobState::Running => forward::ports_text(&job.forwards),
        _ => None,
    };
    let ports = ports.map(|ports| format!(", {ports}")).unwrap_or_default();
    format!(
        "{} {}: {command} ({when}, {} bytes of output{ports})",
        job.id,
        job.state.as_str(),
        job.output_bytes
    )
}

/// The job call dir of `call` of `conversation`, `$R/sbx/<conversation>/jobs/<call>`.
pub(crate) fn job_call_dir(dirs: &Dirs, conversation: ConversationId, call: CallId) -> PathBuf {
    dirs.runtime()
        .join(SHELL_DIR)
        .join(conversation.to_string())
        .join(efr_sandbox::JOB_DIR)
        .join(call.to_string())
}

/// The call dir of the call `call` of `conversation` that the user moved into a job,
/// `$R/sbx/<conversation>/<call>`: the dir of a foreground call of the sandbox.
pub(crate) fn moved_call_dir(dirs: &Dirs, conversation: ConversationId, call: CallId) -> PathBuf {
    dirs.runtime().join(SHELL_DIR).join(conversation.to_string()).join(call.to_string())
}

/// Stops the command of a moved call whose job did not start: `SIGINT` to the job in
/// its shell's foreground, then the shell closes as [`close_adopted`] closes it.
pub(crate) async fn stop_adopted(run: &efr_shell::AdoptedRun) {
    if let Err(error) = run.interrupt().await {
        tracing::warn!(%error, "could not interrupt a moved command");
    }
    close_adopted(run).await;
}

/// Closes the shell of a moved command, and first kills every process below it (DS55):
/// the shell is a child subreaper, so a process that left its session with `setsid` is
/// below it only while the shell lives. Returns the names of the processes below the
/// shell that are still there.
async fn close_adopted(run: &efr_shell::AdoptedRun) -> Vec<String> {
    let shell = run.shell_pid();
    let left = tokio::task::spawn_blocking(move || sweep::kill_descendants(shell))
        .await
        .unwrap_or_default();
    if let Err(error) = run.close().await {
        tracing::warn!(%error, "could not close the shell of a moved command");
    }
    left
}

/// Why a job's task woke before its end.
enum Wakeup {
    /// The command ended.
    Ended,
    /// A stop or the time limit.
    Stop(JobStopBy),
    /// efr-shell interrupted a moved command that waits for a secret.
    Secret,
}

/// Waits until a moved command waits for a secret, as `looks` tells it: a hidden wait or
/// a visible wait that looks secret. It never ends for a launcher (`None`), nor once the
/// watch of the moved command ended.
async fn secret_wait(looks: Option<watch::Receiver<(InputWait, bool)>>) {
    if let Some(mut looks) = looks
        && looks.wait_for(|looked| is_secret(*looked)).await.is_ok()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// True for a wait that efr-shell stops in a job: a hidden wait, or a visible wait that
/// looks secret.
fn is_secret(looked: (InputWait, bool)) -> bool {
    matches!(looked, (InputWait::Hidden, _) | (InputWait::Visible, true))
}

/// The job state dir of `call` of `conversation`, `$S/sandbox/<conversation>/jobs/<call>`.
pub(crate) fn job_state_dir(dirs: &Dirs, conversation: ConversationId, call: CallId) -> PathBuf {
    dirs.state()
        .join(SANDBOX_DIR)
        .join(conversation.to_string())
        .join(efr_sandbox::JOB_DIR)
        .join(call.to_string())
}

/// `changes` with each change of `guarded` that it lacks.
fn with_guarded(
    mut changes: Vec<SurfaceChange>,
    guarded: Vec<SurfaceChange>,
) -> Vec<SurfaceChange> {
    for change in guarded {
        if !changes.contains(&change) {
            changes.push(change);
        }
    }
    changes
}

/// The launcher's `result.json` in `call_dir`, if it wrote one that reads.
async fn read_result(call_dir: &Path) -> Option<SandboxResult> {
    let path = call_dir.join(RESULT_FILE);
    tokio::task::spawn_blocking(move || {
        let bytes =
            crate::sandbox::fs::DaemonFs::read(&path, efr_sandbox::MAX_RESULT_BYTES).ok()?;
        SandboxResult::from_json(&bytes).ok()
    })
    .await
    .ok()
    .flatten()
}

/// The size of the output file `path` and its last lines; nothing when it cannot be
/// read. It blocks.
fn read_tail(path: &Path) -> (u64, String) {
    let size = std::fs::symlink_metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let tail = output::read(path, None, output::TAIL_LINES, output::TAIL_BYTES).unwrap_or_default();
    (size, tail)
}

/// Removes a job's dirs: its call dir and its state dir.
async fn remove_dirs(dirs: Vec<PathBuf>) {
    if dirs.is_empty() {
        return;
    }
    let removed = tokio::task::spawn_blocking(move || {
        for dir in dirs {
            if let Err(error) = std::fs::remove_dir_all(&dir)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::debug!(%error, dir = %dir.display(), "a job's dir stays");
            }
        }
    })
    .await;
    if removed.is_err() {
        tracing::warn!("removing a job's dirs panicked");
    }
}

/// Removes `$R/sbx/*/jobs` and `$S/sandbox/*/jobs`. It blocks.
fn remove_all_job_dirs(dirs: &Dirs) {
    for root in [dirs.runtime().join(SHELL_DIR), dirs.state().join(SANDBOX_DIR)] {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let jobs = entry.path().join(efr_sandbox::JOB_DIR);
            if std::fs::symlink_metadata(&jobs).is_ok_and(|meta| meta.is_dir())
                && let Err(error) = std::fs::remove_dir_all(&jobs)
            {
                tracing::warn!(%error, dir = %jobs.display(), "a job dir of the last efrd stays");
            }
        }
    }
}

/// Hands each notice of `first`, then of `queue`, to its conversation's actor until
/// `cancel`.
pub(crate) async fn deliver(
    state: Arc<crate::state::State>,
    first: Vec<Notice>,
    mut queue: mpsc::UnboundedReceiver<Notice>,
    cancel: CancellationToken,
) {
    for notice in first {
        let conversation = notice.conversation;
        if let Err(error) = state.conversations.open(conversation).notice(notice.input).await {
            tracing::warn!(%conversation, %error, "a job notice could not reach its conversation");
        }
    }
    loop {
        let notice = tokio::select! {
            () = cancel.cancelled() => return,
            notice = queue.recv() => match notice {
                Some(notice) => notice,
                None => return,
            },
        };
        let conversation = notice.conversation;
        if let Err(error) = state.conversations.open(conversation).notice(notice.input).await {
            tracing::warn!(%conversation, %error, "a job notice could not reach its conversation");
        }
    }
}

#[cfg(test)]
mod tests;
