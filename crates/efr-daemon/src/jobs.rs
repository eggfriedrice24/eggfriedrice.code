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
//! model (`job_stop`), the user (`jobs.stop`) or efrd's stop ends it. Every stop by efrd
//! sends SIGCONT and SIGTERM to the launcher through its pidfd, SIGKILL after
//! [`STOP_GRACE`], and then the sweep: SIGKILL to every process of the job's session
//! (`jobs/sweep.rs`). The table records `job_ended` with the tail, the processes that
//! may still run and whether the launcher was lost, hands the actor the notice (not
//! while efrd stops), and removes the job's dirs.
//!
//! At start, a job that the last efrd left running gets `job_ended` with
//! `daemon_restart`: efrd kills its launcher only when the pid's start time still
//! matches the `job_started` event, sweeps its session, and reads a `result.json` that
//! the launcher left. Then every actor gets the notices of the last day that no turn
//! took, which never start a turn of their own.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_config::JobsSettings;
use efr_conversation::{JobNoticeInput, NoticeBody, notice_text, notice_text_refused};
use efr_protocol::{
    CallId, ConversationId, Event, JobId, JobInfo, JobState, JobStopBy, Launch, NoticeKind, Seq,
    SurfaceChange, TurnId,
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
use crate::sandbox::bus::{BusCall, BusReport};
use crate::sandbox::plan::{SANDBOX_DIR, SHELL_DIR};
use crate::sandbox::proxy::ProxyCall;

pub(crate) mod guard;
pub(crate) mod launch;
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
    /// The snapshot of the job's write roots right after its start.
    kept: Option<KeptSnapshot>,
    /// The changes that a guard check found while the job ran (`jobs/guard.rs`).
    guard: Vec<SurfaceChange>,
    /// The job's proxy socket until the job ends; dropping it closes the socket.
    proxy: Option<ProxyCall>,
    /// The job's filtered bus proxies until the job ends; dropping them kills them.
    bus: Option<BusCall>,
}

impl JobTable {
    /// The table, and the queue of notices that `run.rs` hands to the actors.
    pub(crate) fn new(parts: JobParts) -> (JobTable, mpsc::UnboundedReceiver<Notice>) {
        let JobParts { clock, writer, readers, dirs, launcher, spawn_env } = parts;
        let (notices, queue) = mpsc::unbounded_channel();
        let inner = Inner {
            clock,
            writer,
            readers,
            dirs,
            launcher,
            spawn_env,
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
        remove_dirs(job.call_dir.clone(), job.state_dir.clone()).await;
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
            guard: Vec::new(),
            proxy,
            bus,
        };
        self.lock().running.insert(key, entry);
        let watching = (job.matcher.is_some() && job.wakes > 0)
            .then(|| tokio::spawn(self.clone().follow_matches(info.clone(), matches, job.wakes)));
        let task = JobTask {
            table: self.clone(),
            key,
            child,
            pid,
            pidfd,
            output,
            stop: stop_rx,
            ended: ended_tx,
            ends_by: info.ends_by,
            call_dir: job.call_dir,
            state_dir: job.state_dir,
            output_path: info.output.clone(),
            watching,
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

    /// Stops the job `key` with the cause `by` and waits for its end. A job that ended
    /// already keeps its final state; an unknown job is `JobNotFound`.
    pub(crate) async fn stop(&self, key: JobKey, by: JobStopBy) -> Result<JobInfo, DaemonError> {
        let ended = {
            let table = self.lock();
            table.running.get(&key).map(|entry| {
                entry.stop.send_if_modified(|stop| {
                    if stop.is_none() {
                        *stop = Some(by);
                        true
                    } else {
                        false
                    }
                });
                entry.ended.clone()
            })
        };
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
    /// with `daemon_restart`, and removes every job dir.
    pub(crate) async fn recover(&self) -> Result<(), DaemonError> {
        self.recover_with(sweep::sweep_left).await
    }

    /// [`recover`](Self::recover) with `sweeper` in place of [`sweep::sweep_left`].
    async fn recover_with(
        &self,
        sweeper: fn(u32, u64, i64) -> Vec<String>,
    ) -> Result<(), DaemonError> {
        let rows = self.inner.readers.with(efr_store::jobs::running).await?;
        let dirs = self.inner.dirs.clone();
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
            let call_dir = job_call_dir(&dirs, info.conversation_id, info.call_id);
            let result = read_result(&call_dir).await;
            let launcher_lost = result.is_none();
            // NOTE: a contained job that left no result had no end guard: its guard
            // check runs now, before its state dir goes.
            let lost_guard = if launcher_lost && matches!(info.launch, Launch::Contained { .. }) {
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
            self.inner.writer.append(Batch::new().event(info.conversation_id, event)).await?;
            tracing::info!(conversation = %info.conversation_id, job = %info.id, "a job of the last efrd was recorded as stopped");
        }
        let removed = tokio::task::spawn_blocking(move || remove_all_job_dirs(&dirs)).await;
        if removed.is_err() {
            tracing::warn!("removing the job dirs of the last efrd panicked");
        }
        Ok(())
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
    ) -> Result<(), DaemonError> {
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
    child: tokio::process::Child,
    pid: u32,
    pidfd: Option<std::os::fd::OwnedFd>,
    output: output::Output,
    stop: watch::Receiver<Option<JobStopBy>>,
    ended: watch::Sender<Option<JobInfo>>,
    ends_by: Timestamp,
    call_dir: PathBuf,
    state_dir: PathBuf,
    output_path: PathBuf,
    /// The task that records the job's watch matches.
    watching: Option<tokio::task::JoinHandle<()>>,
}

impl JobTask {
    async fn run(mut self) {
        let clock = Arc::clone(&self.table.inner.clock);
        let mut stop_rx = self.stop.clone();
        // NOTE: from the time limit, not from now: a clock that moved before this task
        // first ran still ends the job at its limit.
        let left = self.ends_by.duration_since(clock.now());
        let left = Duration::try_from(left).unwrap_or(Duration::ZERO);
        let requested = tokio::select! {
            status = self.child.wait() => {
                if let Err(error) = status {
                    tracing::warn!(%error, job = self.key.number, "waiting for a job's launcher failed");
                }
                None
            }
            stop = stop_rx.wait_for(Option::is_some) => stop.ok().and_then(|stop| *stop),
            () = clock.sleep(left) => Some(JobStopBy::TimeLimit),
        };
        let stopped_by = match requested {
            Some(by) => {
                // NOTE: the first stop wins: a stop and the limit at once keep the stop.
                self.table.lock().running.get(&self.key).map(|entry| {
                    entry.stop.send_if_modified(|stop| {
                        if stop.is_none() {
                            *stop = Some(by);
                            true
                        } else {
                            false
                        }
                    })
                });
                let by = (*self.stop.borrow()).unwrap_or(by);
                self.escalate(&clock).await;
                Some(by)
            }
            None => None,
        };
        self.finish(stopped_by).await;
    }

    /// SIGCONT and SIGTERM to the launcher, SIGKILL after [`STOP_GRACE`].
    async fn escalate(&mut self, clock: &Arc<dyn Clock>) {
        launch::signal(self.pidfd.as_ref(), self.pid, Signal::CONT);
        launch::signal(self.pidfd.as_ref(), self.pid, Signal::TERM);
        if clock.timeout(STOP_GRACE, self.child.wait()).await.is_err() {
            tracing::warn!(
                job = self.key.number,
                "a job's launcher did not end after SIGTERM; killing it"
            );
            launch::signal(self.pidfd.as_ref(), self.pid, Signal::KILL);
            let _ = self.child.wait().await;
        }
    }

    /// Everything after the launcher ended: the sweep, the rest of the output, the
    /// result, the record, the notice and the dirs.
    async fn finish(mut self, stopped_by: Option<JobStopBy>) {
        let table = &self.table;
        let clock = Arc::clone(&table.inner.clock);
        let (lifeline, kept, guarded, info, proxy, bus) = {
            let mut guard = table.lock();
            match guard.running.get_mut(&self.key) {
                Some(entry) => (
                    entry.lifeline.take(),
                    entry.kept.take(),
                    std::mem::take(&mut entry.guard),
                    Some(entry.info.clone()),
                    entry.proxy.take(),
                    entry.bus.take(),
                ),
                None => (None, None, Vec::new(), None, None, None),
            }
        };
        drop(lifeline);
        let Some(info) = info else { return };
        let session = self.pid;
        let swept = tokio::task::spawn_blocking(move || sweep::kill_session(session))
            .await
            .unwrap_or_default();
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
        let result = read_result(&self.call_dir).await;
        let launcher_lost = result.is_none();
        let ended = end_of(result.as_ref(), stopped_by);
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
        // joined the turn that started it.
        if let Some((snapshot, limits)) = kept {
            join_start_turn(snapshot, limits, info.started_at).await;
        }
        table.lock().running.remove(&self.key);
        remove_dirs(self.call_dir.clone(), self.state_dir.clone()).await;
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
                // turns. Every other end wakes the model, also a stop by the user.
                let wakes = !matches!(ended.stopped_by, Some(JobStopBy::Model));
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
    format!(
        "{} {}: {command} ({when}, {} bytes of output)",
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

/// Removes a job's call dir and state dir.
async fn remove_dirs(call_dir: PathBuf, state_dir: PathBuf) {
    let removed = tokio::task::spawn_blocking(move || {
        for dir in [call_dir, state_dir] {
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
