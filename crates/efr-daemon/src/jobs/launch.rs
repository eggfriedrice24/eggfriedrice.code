//! The spawn of a job's launcher (efr's auto spec, section 3.18): efrd starts
//! `efr-sbx run --call-dir <dir> --job` itself, never through the hidden shell.
//!
//! The launcher gets a small environment of efrd's (its `PATH`, `HOME` and
//! `XDG_RUNTIME_DIR`), so no value of the shell, such as `LD_PRELOAD`, reaches it; the
//! job's own environment is the `env` file in its call dir. Its working directory is
//! the job's start dir. Its stdin is the lifeline: the read end of a pipe whose write
//! end only efrd holds (close-on-exec, so no other child inherits it). Its stdout is the
//! output pipe, which efrd reads, and its stderr is `$CALL/launcher.log`. efrd does not
//! make it a process group leader, so its `setsid` works. Every signal goes through a
//! pidfd opened at the spawn.

use std::io::{self, Write as _};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use crate::jobs::sweep;

/// The flag of the launcher's job mode.
const JOB_FLAG: &str = "--job";

/// The longest wait for a launcher to write `started` or end. A launcher that does
/// neither is stopped.
const START_LIMIT: Duration = Duration::from_secs(30);

/// How often the wait for `started` looks.
const START_POLL: Duration = Duration::from_millis(2);

/// efrd's own variables that a launcher gets.
#[derive(Debug, Clone, Default)]
pub(crate) struct SpawnEnv {
    /// efrd's `PATH`, where the launcher finds nothing of a write root.
    pub(crate) path: String,
    pub(crate) home: PathBuf,
    pub(crate) user_runtime: PathBuf,
}

/// A launcher that runs and waits for its start byte.
#[derive(Debug)]
pub(crate) struct Spawned {
    pub(crate) child: tokio::process::Child,
    pub(crate) pid: u32,
    /// The start time of the launcher's process, in clock ticks after boot.
    pub(crate) start: u64,
    pub(crate) pidfd: Option<OwnedFd>,
    /// The write end of the lifeline.
    pub(crate) lifeline: io::PipeWriter,
    /// The read end of the output pipe.
    pub(crate) output: io::PipeReader,
}

/// Starts `launcher` for the job in `call_dir` with `cwd` as its working directory.
pub(crate) fn spawn(
    launcher: &Path,
    call_dir: &Path,
    cwd: &Path,
    env: &SpawnEnv,
) -> io::Result<Spawned> {
    let (lifeline_read, lifeline) = io::pipe()?;
    let (output, output_write) = io::pipe()?;
    let log = efr_stdx::fs::create_private(&call_dir.join(efr_sandbox::LAUNCHER_LOG_FILE))
        .map_err(io::Error::other)?;
    let mut command = efr_stdx::process::command(launcher, cwd);
    command
        .env_clear()
        .env("PATH", &env.path)
        .env("HOME", &env.home)
        .env("XDG_RUNTIME_DIR", &env.user_runtime)
        .env("PWD", cwd)
        .arg("run")
        .arg("--call-dir")
        .arg(call_dir)
        .arg(JOB_FLAG)
        .stdin(Stdio::from(OwnedFd::from(lifeline_read)))
        .stdout(Stdio::from(OwnedFd::from(output_write)))
        .stderr(Stdio::from(log))
        .kill_on_drop(false);
    let child = command.spawn()?;
    // NOTE: the command holds the other ends of both pipes; they must close here, or
    // the output never ends and the lifeline never breaks.
    drop(command);
    let pid = child.id().ok_or_else(|| io::Error::other("the launcher has no pid"))?;
    let raw = i32::try_from(pid).ok().and_then(Pid::from_raw);
    // NOTE: opened before anything waits for the child, so the pid still names it.
    let pidfd = raw.and_then(|pid| pidfd_open(pid, PidfdFlags::empty()).ok());
    let start = sweep::start_time(pid).unwrap_or(0);
    Ok(Spawned { child, pid, start, pidfd, lifeline, output })
}

/// Writes the start byte: the launcher starts nothing before it.
pub(crate) fn start(lifeline: &mut io::PipeWriter) -> io::Result<()> {
    lifeline.write_all(b"s")
}

/// How the wait for a launcher's `started` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartWait {
    /// The launcher wrote `started`: every bind source is open.
    Started,
    /// The launcher ended first.
    Ended,
    /// Neither came in time.
    TimedOut,
}

/// Waits until `started` exists or the process `pid` ended (a zombie counts). It
/// blocks, with a real-time poll: the launcher writes the file within milliseconds,
/// and the wait holds the plan lock.
pub(crate) fn wait_started(started: &Path, pid: u32) -> StartWait {
    let begin = Instant::now();
    let proc_root = Path::new("/proc");
    loop {
        if std::fs::symlink_metadata(started).is_ok() {
            return StartWait::Started;
        }
        match sweep::process(proc_root, pid) {
            None => return StartWait::Ended,
            Some(process) if process.state == 'Z' => return StartWait::Ended,
            Some(_) => {}
        }
        if begin.elapsed() >= START_LIMIT {
            return StartWait::TimedOut;
        }
        std::thread::sleep(START_POLL);
    }
}

/// Sends `signal` to the launcher through its pidfd, else by its pid.
pub(crate) fn signal(pidfd: Option<&OwnedFd>, pid: u32, signal: Signal) {
    match pidfd {
        Some(fd) => {
            let _ = pidfd_send_signal(fd, signal);
        }
        None => sweep::kill(pid, signal),
    }
}
