//! The processes of a job by its session (efr's auto spec, section 3.18).
//!
//! A job's launcher calls `setsid`, so the job's session id is the launcher's pid. A
//! process of the job that outlives its launcher (a double fork of an exit child job,
//! or a launcher that was lost) keeps that session id unless it calls `setsid` itself.
//! So efrd keeps the id model-side on its socket while any process has it, and every
//! stop by efrd ends with a sweep: SIGKILL to every process of the session, each
//! through a pidfd that efrd checks again after it opened it, so a pid that a new
//! process took meanwhile is never hit.
//!
//! Linux frees a pid only when no process uses it as its pid, its group or its session.
//! So while a process of the job's session lives, no other process can get the
//! launcher's pid, and its session is the job's. That holds within one boot only: at
//! efrd's start, [`sweep_left`] sweeps the session of a job of the last efrd only when
//! its launcher still runs with its recorded start time, or when the launcher is gone
//! and the system did not boot again since the job started.
//!
//! The scan reads `/proc/<pid>/stat` of every process: one `readdir` of `/proc` and one
//! small read per process.

use std::path::Path;
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

/// How much later than its job's recorded start a launcher may seem to have started,
/// by the boot time and its start in clock ticks, and still count as of this boot: the
/// whole seconds of the boot time and a clock that the system adjusted since.
const BOOT_SLACK_SECS: i64 = 30;

/// How long the sweep waits for the processes that it killed to go.
const SWEEP_GRACE: Duration = Duration::from_secs(1);

/// How often the sweep looks again within its grace.
const SWEEP_POLL: Duration = Duration::from_millis(20);

/// One process, as `/proc/<pid>/stat` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Process {
    pub(crate) pid: u32,
    /// The command name, at most 15 bytes.
    pub(crate) name: String,
    /// The state letter, such as `S`, `T` or `Z`.
    pub(crate) state: char,
    /// The parent's pid.
    pub(crate) parent: u32,
    pub(crate) session: u32,
    /// The start time in clock ticks after boot.
    pub(crate) start: u64,
}

/// The fields of one `/proc/<pid>/stat` text: the name ends at the last `)`.
pub(crate) fn parse_stat(pid: u32, stat: &str) -> Option<Process> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // NOTE: the fields after the name start with the state (field 3); the parent is
    // field 4, the session field 6 and the start time field 22.
    let state = fields.first()?.chars().next()?;
    let parent = fields.get(1)?.parse().ok()?;
    let session = fields.get(3)?.parse().ok()?;
    let start = fields.get(19)?.parse().ok()?;
    Some(Process { pid, name, state, parent, session, start })
}

/// The process `pid` below `proc_root`, or `None` when it is gone.
pub(crate) fn process(proc_root: &Path, pid: u32) -> Option<Process> {
    let stat = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    parse_stat(pid, &stat)
}

/// The start time of `pid` in clock ticks after boot, or `None` when it is gone.
pub(crate) fn start_time(pid: u32) -> Option<u64> {
    process(Path::new("/proc"), pid).map(|process| process.start)
}

/// Every live process below `proc_root` whose session is `session`; a zombie is gone
/// already and does not count.
pub(crate) fn members(proc_root: &Path, session: u32) -> Vec<Process> {
    if session == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(proc_root) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| process(proc_root, pid))
        .filter(|process| process.session == session && process.state != 'Z')
        .collect()
}

/// Every live process below `proc_root` that descends from `pid`, by the parent of
/// each, parents before children; `pid` itself is not in the list. A hidden shell is a
/// child subreaper, so every process that a command of it left behind stays below it,
/// also one that called `setsid`, and one in a pid namespace of its own: `/proc` of
/// efrd's namespace names each by its pid there.
pub(crate) fn descendants(proc_root: &Path, pid: u32) -> Vec<Process> {
    if pid == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(proc_root) else { return Vec::new() };
    let all: Vec<Process> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| process(proc_root, pid))
        .filter(|process| process.state != 'Z')
        .collect();
    let mut found: Vec<Process> = Vec::new();
    let mut parents = vec![pid];
    while let Some(parent) = parents.pop() {
        for child in all.iter().filter(|process| process.parent == parent) {
            if child.pid != pid && !found.iter().any(|known| known.pid == child.pid) {
                parents.push(child.pid);
                found.push(child.clone());
            }
        }
    }
    found
}

/// Kills every process that descends from `pid` with SIGKILL (DS55), each through a
/// pidfd that is checked again after it opened, waits up to a second for them to go,
/// and returns the names of those that are still there. `pid` itself lives on; efrd's
/// own process is never hit. It blocks.
pub(crate) fn kill_descendants(pid: u32) -> Vec<String> {
    let own = std::process::id();
    if pid == 0 || pid == own {
        return Vec::new();
    }
    let proc_root = Path::new("/proc");
    let begin = Instant::now();
    loop {
        let left: Vec<Process> =
            descendants(proc_root, pid).into_iter().filter(|process| process.pid != own).collect();
        if left.is_empty() {
            return Vec::new();
        }
        for member in &left {
            kill_child(proc_root, member, Signal::KILL);
        }
        // NOTE: a real-time grace, as the sweep of a session.
        if begin.elapsed() >= SWEEP_GRACE {
            let mut names: Vec<String> =
                descendants(proc_root, pid).into_iter().map(|member| member.name).collect();
            names.sort();
            names.dedup();
            return names;
        }
        std::thread::sleep(SWEEP_POLL);
    }
}

/// Sends `signal` to `member` through a pidfd, only when the process that the pidfd
/// holds still has the parent and the start time of `member`.
fn kill_child(proc_root: &Path, member: &Process, signal: Signal) {
    let Some(pid) = i32::try_from(member.pid).ok().and_then(Pid::from_raw) else { return };
    let Ok(fd) = pidfd_open(pid, PidfdFlags::empty()) else { return };
    let same = process(proc_root, member.pid)
        .is_some_and(|now| now.parent == member.parent && now.start == member.start);
    if same {
        let _ = pidfd_send_signal(&fd, signal);
    }
}

/// True while a process of the session `session` lives.
pub(crate) fn has_members(session: u32) -> bool {
    !members(Path::new("/proc"), session).is_empty()
}

/// Kills every process of the session `session` with SIGKILL, waits up to a second for
/// them to go, and returns the names of those that are still there. efrd's own session
/// is never swept. It blocks.
pub(crate) fn kill_session(session: u32) -> Vec<String> {
    kill_session_since(session, 0)
}

/// At efrd's start: sweeps what is left of a job of the last efrd, whose launcher had
/// the pid `pid` and the start time `start` (clock ticks after boot), and whose job
/// started at `started_at` (seconds since the epoch). It kills the launcher when the
/// pid still has that start time. It sweeps the session only when it is still the
/// job's: the launcher runs with that start time, or it is gone and the system did not
/// boot again since. Then only the processes that started at or after the launcher
/// count. Returns the names of the processes that are still there. It blocks.
pub(crate) fn sweep_left(pid: u32, start: u64, started_at: i64) -> Vec<String> {
    if pid == 0 {
        return Vec::new();
    }
    let proc_root = Path::new("/proc");
    match process(proc_root, pid) {
        Some(launcher) if launcher.start == start => {
            kill_member(proc_root, &launcher, Signal::KILL)
        }
        // NOTE: another process has the pid now, so no process had the old session when
        // the pid was freed: what has the session now is not the job's.
        Some(_) => return Vec::new(),
        None => {
            let booted = boot_time(proc_root);
            let hz = rustix::param::clock_ticks_per_second();
            if !booted.is_some_and(|booted| same_boot(booted, hz, start, started_at)) {
                return Vec::new();
            }
        }
    }
    kill_session_since(pid, start)
}

/// True when a launcher that started `start` clock ticks (of `hz` per second) after a
/// boot at `booted` (seconds since the epoch) can be the launcher of a job that started
/// at `started_at`: a launcher starts before its job, and a process of a later boot
/// seems to have started after the job.
pub(crate) fn same_boot(booted: i64, hz: u64, start: u64, started_at: i64) -> bool {
    let after_boot = i64::try_from(start / hz.max(1)).unwrap_or(i64::MAX);
    booted.saturating_add(after_boot) <= started_at.saturating_add(BOOT_SLACK_SECS)
}

/// The time of the last boot in seconds since the epoch: the `btime` line of
/// `/proc/stat` below `proc_root`.
pub(crate) fn boot_time(proc_root: &Path) -> Option<i64> {
    let stat = std::fs::read_to_string(proc_root.join("stat")).ok()?;
    stat.lines().find_map(|line| line.strip_prefix("btime ")?.trim().parse().ok())
}

/// [`kill_session`] for the processes of `session` that started at or after `since`
/// (clock ticks after boot).
pub(crate) fn kill_session_since(session: u32, since: u64) -> Vec<String> {
    let own = std::process::id();
    let own_session = process(Path::new("/proc"), own).map(|process| process.session);
    if session == 0 || session == own || Some(session) == own_session {
        return Vec::new();
    }
    let proc_root = Path::new("/proc");
    let start = Instant::now();
    let of_job = |process: &Process| process.start >= since;
    loop {
        let left: Vec<Process> = members(proc_root, session).into_iter().filter(of_job).collect();
        if left.is_empty() {
            return Vec::new();
        }
        for member in &left {
            if member.pid == own {
                continue;
            }
            kill_member(proc_root, member, Signal::KILL);
        }
        // NOTE: a real-time grace, as the launcher's own: a process that SIGKILL ends is
        // gone within a few milliseconds, and a stopped clock of a test must not keep
        // the sweep waiting forever.
        if start.elapsed() >= SWEEP_GRACE {
            let mut names: Vec<String> = members(proc_root, session)
                .into_iter()
                .filter(of_job)
                .map(|member| member.name)
                .collect();
            names.sort();
            names.dedup();
            return names;
        }
        std::thread::sleep(SWEEP_POLL);
    }
}

/// Sends `signal` to `member` through a pidfd, only when the process that the pidfd
/// holds still has the session and the start time of `member`: a member that ended
/// after the scan, whose pid a new process took before the pidfd opened, is never hit.
fn kill_member(proc_root: &Path, member: &Process, signal: Signal) {
    let Some(pid) = i32::try_from(member.pid).ok().and_then(Pid::from_raw) else { return };
    let Ok(fd) = pidfd_open(pid, PidfdFlags::empty()) else { return };
    // NOTE: the pidfd holds the process that had the pid when it opened; a read after
    // that which still finds the member's session and start time is of that process.
    let same = process(proc_root, member.pid)
        .is_some_and(|now| now.session == member.session && now.start == member.start);
    if same {
        let _ = pidfd_send_signal(&fd, signal);
    }
}

/// Sends `signal` to `pid` through a pidfd: a pid that a new process took after the
/// pidfd opened is never hit.
pub(crate) fn kill(pid: u32, signal: Signal) {
    let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else { return };
    if let Ok(fd) = pidfd_open(pid, PidfdFlags::empty()) {
        let _ = pidfd_send_signal(&fd, signal);
    }
}

#[cfg(test)]
mod tests;
