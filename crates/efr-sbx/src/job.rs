//! `efr-sbx run --job`: a long command that efrd starts itself, with no terminal
//! (efr's auto spec, section 3.18).
//!
//! The hidden shell's wrapper never starts a job: efrd spawns the launcher with its own
//! scrubbed environment, the job's start dir as the working directory, a pipe as
//! stdin and the output pipe as stdout. The launcher in job mode, in order:
//!
//! 1. Calls `setsid`: no controlling terminal, a session and a process group of its
//!    own. The session id is the launcher's pid, which efrd keeps model-side.
//! 2. Catches SIGTERM and SIGHUP: either one stops the job ([`JobStop::Signal`]).
//! 3. Reads one start byte from stdin, the lifeline, and starts nothing before it:
//!    efrd writes the byte once the launcher's pid is model-side. When the pipe ends
//!    first, no child runs.
//! 4. Lowers its own priority on the main thread, before any child starts: nice to
//!    the higher of its own value and the spec's, the idle IO class, a hard
//!    `RLIMIT_FSIZE`, SIGXFSZ ignored, so a write past the limit fails with EFBIG, and
//!    an `oom_score_adj` of 1000. Every child inherits all five.
//! 5. Watches the lifeline: its end means that efrd is gone ([`JobStop::Lifeline`]).
//!    A deadline of efrd's limit plus the backstop ends the job too
//!    ([`JobStop::Deadline`]).
//! 6. Runs the job contained or in the exit child, with the environment of
//!    `$CALL/env`, stdin `/dev/null`, and stdout and stderr on the output pipe.
//!
//! A stop sends SIGTERM to every process of the job but bwrap's own (the namespace's
//! init ignores it, and bwrap's end would kill the rest at once), SIGKILL from 2 s, and
//! gives up at 4 s. Then the normal end writes `result.json` with the stop's reason.
//! No `apply`, no `state.json` and no `state.zsh`: a job changes no state of the hidden
//! shell. There is no `PR_SET_PDEATHSIG`: it fires when the thread that forked ends, and
//! efrd's blocking threads end when idle. The lifeline does the same job.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use efr_sandbox::{ENV_FILE, JOB_OOM_SCORE_ADJ, JobPlan, JobStop, MAX_ENV_BYTES};
use rustix::process::{Pid, Resource, Rlimit, Signal};

use crate::call_dir::CallDir;
use crate::error::SbxError;
use crate::exit_child::{self, Processes};
use crate::{fds, os, signals};

/// How often a stop that did not end the job runs again: a process that the job
/// started just as the first stop ran gets its own.
const RESTOP: Duration = Duration::from_secs(1);

/// What the stop thread hears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// SIGTERM or SIGHUP came.
    Signal,
    /// The lifeline ended.
    Lifeline,
    /// The job's child ended, and the launcher goes on to its end.
    Ended,
}

/// A job of this launcher: its environment, the stop thread and what it decided.
#[derive(Debug)]
pub(crate) struct Job {
    /// The job's environment, from `$CALL/env`.
    pub(crate) env: BTreeMap<OsString, OsString>,
    /// The nice value that the launcher, and with it every child, got.
    pub(crate) nice: u8,
    events: Sender<Event>,
    stopped: Arc<Mutex<Option<JobStop>>>,
    monitor: Option<JoinHandle<()>>,
}

/// The stop signals of a job, caught before the call dir is read. Returns the stop
/// thread's channel.
pub(crate) fn catch_stops() -> Result<(Sender<Event>, Receiver<Event>), SbxError> {
    let (send, receive) = std::sync::mpsc::channel();
    signals::catch_stop(send.clone())?;
    Ok((send, receive))
}

/// `setsid`, the first step of a job: the launcher leads a new session and process
/// group, with no controlling terminal. efrd spawns it as no group's leader; a
/// launcher that leads its session already goes on, and one that leads only a group
/// refuses, because its job would share the session of its parent.
pub(crate) fn new_session() -> Result<(), SbxError> {
    match rustix::process::setsid() {
        Ok(_) => Ok(()),
        Err(error) => {
            let me = rustix::process::getpid();
            if rustix::process::getsid(None).is_ok_and(|session| session == me) {
                Ok(())
            } else {
                Err(SbxError::os("start a session of its own", error.into()))
            }
        }
    }
}

/// Reads the start byte from stdin. False when the lifeline ended before it: efrd is
/// gone, and no child may start.
pub(crate) fn wait_for_start() -> Result<bool, SbxError> {
    let stdin = std::io::stdin();
    let mut byte = [0_u8; 1];
    loop {
        match rustix::io::read(stdin.as_fd(), &mut byte) {
            Ok(1) => return Ok(true),
            Ok(_) => return Ok(false),
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(SbxError::os("read the start byte", error.into())),
        }
    }
}

impl Job {
    /// The job of `call` after its start byte: its priority set, its environment read,
    /// and the lifeline and the stop thread running. `contained` says whether bwrap
    /// runs it, whose own processes a stop leaves to the end.
    pub(crate) fn start(
        call: &CallDir,
        plan: JobPlan,
        events: (Sender<Event>, Receiver<Event>),
        contained: bool,
    ) -> Result<Job, SbxError> {
        let deadline = os::now() + plan.deadline();
        let nice = lower_priority(&plan)?;
        let env = read_env(call)?;
        let (send, receive) = events;
        watch_lifeline(send.clone())?;
        let stopped = Arc::new(Mutex::new(None));
        let decided = Arc::clone(&stopped);
        let monitor = std::thread::Builder::new()
            .name("efr-sbx-stop".to_owned())
            .spawn(move || {
                let mut stop = || {
                    let mut procs = JobProcesses::new(contained);
                    exit_child::end_descendants(&mut procs);
                };
                if monitor(&receive, deadline, &mut stop, &decided).is_none() {
                    return;
                }
                // NOTE: keep at it until the child ended: a process that the job started
                // while the first stop ran, or a child that started just after it, gets
                // a stop of its own.
                loop {
                    match receive.recv_timeout(RESTOP) {
                        Ok(Event::Ended) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(_) => {}
                        Err(RecvTimeoutError::Timeout) => stop(),
                    }
                }
            })
            .map_err(|error| SbxError::os("start the stop thread", error))?;
        Ok(Job { env, nice, events: send, stopped, monitor: Some(monitor) })
    }

    /// Why the job was stopped, when it was: the launcher starts no child after a stop.
    pub(crate) fn stopped(&self) -> Option<JobStop> {
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fails when a stop came before the child started.
    pub(crate) fn stop_if_stopped(&self) -> Result<(), SbxError> {
        match self.stopped() {
            Some(stop) => Err(SbxError::JobStopped { stop }),
            None => Ok(()),
        }
    }

    /// Tells the stop thread that the child ended, waits for it, and returns why the
    /// job was stopped, if it was.
    pub(crate) fn finish(&mut self) -> Option<JobStop> {
        let _ = self.events.send(Event::Ended);
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
        self.stopped()
    }
}

/// Waits for the first stop event, or the deadline, and runs `stop` for it. Returns
/// the reason, or `None` when the child ended first. The reason lands in `decided`
/// before `stop` runs, so the launcher starts no child once a stop began.
pub(crate) fn monitor(
    events: &Receiver<Event>,
    deadline: Instant,
    stop: &mut dyn FnMut(),
    decided: &Mutex<Option<JobStop>>,
) -> Option<JobStop> {
    let reason = loop {
        let left = deadline.saturating_duration_since(os::now());
        match events.recv_timeout(left) {
            Ok(Event::Signal) => break JobStop::Signal,
            Ok(Event::Lifeline) => break JobStop::Lifeline,
            Ok(Event::Ended) | Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {
                if os::now() >= deadline {
                    break JobStop::Deadline;
                }
            }
        }
    };
    *decided.lock().unwrap_or_else(PoisonError::into_inner) = Some(reason);
    stop();
    Some(reason)
}

/// Sets the job's priority on the calling thread, the main one, which starts every
/// child: nice to `max(current, plan.nice)`, the idle IO class, the hard file size
/// limit, SIGXFSZ ignored, and the launcher's own `oom_score_adj`. Returns the nice
/// value it got.
///
/// A nice value only goes up, which needs no privilege; a refusal still leaves the
/// launcher's own value. The IO class is a courtesy (a job may raise it again), so a
/// kernel that refuses it does not stop the job.
///
/// NOTE: the launcher sets `oom_score_adj` for itself and every child inherits it: in
/// the sandbox `/proc` is the host's, read-only, so the inner stage cannot write it.
/// The OOM killer then picks the largest process of the job, rarely the small
/// launcher; when it does, efrd's sweep by session id ends the rest.
fn lower_priority(plan: &JobPlan) -> Result<u8, SbxError> {
    let oom = "/proc/self/oom_score_adj";
    std::fs::write(oom, JOB_OOM_SCORE_ADJ.to_string())
        .map_err(|error| SbxError::io("write", oom, error))?;
    let current = rustix::process::getpriority_process(None)
        .map_err(|error| SbxError::os("read the nice value", error.into()))?;
    let wanted = current.max(i32::from(plan.nice));
    if wanted != current {
        let _ = rustix::process::setpriority_process(None, wanted);
    }
    let _ = fds::ioprio_set_idle();
    let hard = rustix::process::getrlimit(Resource::Fsize).maximum;
    let limit = hard.map_or(plan.fsize_bytes, |hard| hard.min(plan.fsize_bytes));
    rustix::process::setrlimit(
        Resource::Fsize,
        Rlimit { current: Some(limit), maximum: Some(limit) },
    )
    .map_err(|error| SbxError::os("set the file size limit", error.into()))?;
    fds::ignore_sigxfsz().map_err(|error| SbxError::os("ignore SIGXFSZ", error))?;
    let got = rustix::process::getpriority_process(None)
        .map_err(|error| SbxError::os("read the nice value", error.into()))?;
    Ok(u8::try_from(got.clamp(0, 19)).unwrap_or(19))
}

/// The job's environment: `$CALL/env`, NUL-separated `NAME=value` entries that efrd
/// copied from the hidden shell. An entry without `=` or with an empty name is left
/// out. A missing file refuses the job: it never runs with efrd's own environment.
pub(crate) fn read_env(call: &CallDir) -> Result<BTreeMap<OsString, OsString>, SbxError> {
    let Some(bytes) = call.dir.read(ENV_FILE, MAX_ENV_BYTES)? else {
        return Err(SbxError::CallDir {
            path: call.dir.path().join(ENV_FILE),
            problem: crate::error::CallDirProblem::File,
        });
    };
    Ok(parse_env(&bytes))
}

/// The entries of an `env` file.
pub(crate) fn parse_env(bytes: &[u8]) -> BTreeMap<OsString, OsString> {
    bytes
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let at = entry.iter().position(|byte| *byte == b'=')?;
            let (name, value) = (entry.get(..at)?, entry.get(at + 1..)?);
            (!name.is_empty())
                .then(|| (OsStr::from_bytes(name).to_owned(), OsStr::from_bytes(value).to_owned()))
        })
        .collect()
}

/// A thread that blocks in a read of stdin, the lifeline, and reports its end. efrd
/// holds the only write end; it never writes after the start byte.
fn watch_lifeline(events: Sender<Event>) -> Result<(), SbxError> {
    let stdin = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| SbxError::os("copy the lifeline", error))?;
    std::thread::Builder::new()
        .name("efr-sbx-lifeline".to_owned())
        .spawn(move || {
            let mut lifeline = File::from(stdin);
            let mut bytes = [0_u8; 64];
            loop {
                match lifeline.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = events.send(Event::Lifeline);
        })
        .map(drop)
        .map_err(|error| SbxError::os("start the lifeline thread", error))
}

/// The processes of a job, for a stop on the stop thread. It never reaps (the main
/// thread waits for the child and reaps), so it skips zombies. In a contained job,
/// bwrap's own processes, the launcher's child and the namespace's init, get no
/// SIGTERM: the init ignores it from outside its namespace, and bwrap's end would
/// kill every process inside at once, with no grace. SIGKILL still reaches them.
pub(crate) struct JobProcesses<P: Processes> {
    procs: P,
    spared: BTreeSet<i32>,
}

impl JobProcesses<Live> {
    fn new(contained: bool) -> Self {
        let spared = if contained { bwrap_pids() } else { BTreeSet::new() };
        JobProcesses { procs: Live { start: os::now() }, spared }
    }
}

#[cfg(test)]
impl<P: Processes> JobProcesses<P> {
    /// `procs` with `spared` left out of SIGTERM.
    pub(crate) fn with(procs: P, spared: BTreeSet<i32>) -> Self {
        JobProcesses { procs, spared }
    }
}

impl<P: Processes> Processes for JobProcesses<P> {
    fn descendants(&mut self) -> Vec<(Pid, String)> {
        self.procs.descendants()
    }

    fn signal(&mut self, pid: Pid, signal: Signal) {
        if signal == Signal::TERM && self.spared.contains(&pid.as_raw_nonzero().get()) {
            return;
        }
        self.procs.signal(pid, signal);
    }

    fn reap(&mut self) {}

    fn elapsed(&self) -> Duration {
        self.procs.elapsed()
    }

    fn pause(&mut self) {
        self.procs.pause();
    }
}

/// The real descendants of the launcher, without zombies.
pub(crate) struct Live {
    start: Instant,
}

impl Processes for Live {
    fn descendants(&mut self) -> Vec<(Pid, String)> {
        exit_child::descendants().into_iter().filter(|(pid, _)| !zombie(*pid)).collect()
    }

    fn signal(&mut self, pid: Pid, signal: Signal) {
        let _ = rustix::process::kill_process(pid, signal);
    }

    fn reap(&mut self) {}

    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn pause(&mut self) {
        os::sleep(exit_child::POLL);
    }
}

/// True when `pid` exited and waits to be reaped.
fn zombie(pid: Pid) -> bool {
    let stat =
        std::fs::read_to_string(format!("/proc/{}/stat", pid.as_raw_nonzero())).unwrap_or_default();
    // The state follows the name, which ends with the last `)`.
    stat.rsplit_once(')').is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
}

/// bwrap's own processes: each child of the launcher named `bwrap`, and its children,
/// the namespace's init.
fn bwrap_pids() -> BTreeSet<i32> {
    let mut pids = BTreeSet::new();
    for child in children(rustix::process::getpid().as_raw_nonzero().get()) {
        let name = std::fs::read_to_string(format!("/proc/{child}/comm")).unwrap_or_default();
        if name.trim() == "bwrap" {
            pids.insert(child);
            pids.extend(children(child));
        }
    }
    pids
}

/// The direct children of `pid`.
fn children(pid: i32) -> Vec<i32> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return Vec::new() };
    tasks
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|list| {
            list.split_whitespace().filter_map(|word| word.parse().ok()).collect::<Vec<i32>>()
        })
        .collect()
}

#[cfg(test)]
mod tests;
