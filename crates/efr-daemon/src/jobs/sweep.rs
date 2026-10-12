//! The processes of a job by its session (efr's auto spec, section 3.18).
//!
//! A job's launcher calls `setsid`, so the job's session id is the launcher's pid. A
//! process of the job that outlives its launcher (a double fork of an exit child job,
//! or a launcher that was lost) keeps that session id unless it calls `setsid` itself.
//! So efrd keeps the id model-side on its socket while any process has it, and every
//! stop by efrd ends with a sweep: SIGKILL to every process of the session, each
//! through a pidfd, so a pid that a new process took meanwhile is never hit.
//!
//! The scan reads `/proc/<pid>/stat` of every process: one `readdir` of `/proc` and one
//! small read per process.

use std::path::Path;
use std::time::{Duration, Instant};

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

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
    // NOTE: the fields after the name start with the state (field 3); the session is
    // field 6 and the start time field 22.
    let state = fields.first()?.chars().next()?;
    let session = fields.get(3)?.parse().ok()?;
    let start = fields.get(19)?.parse().ok()?;
    Some(Process { pid, name, state, session, start })
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

/// True while a process of the session `session` lives.
pub(crate) fn has_members(session: u32) -> bool {
    !members(Path::new("/proc"), session).is_empty()
}

/// Kills every process of the session `session` with SIGKILL, waits up to a second for
/// them to go, and returns the names of those that are still there. efrd's own session
/// is never swept. It blocks.
pub(crate) fn kill_session(session: u32) -> Vec<String> {
    let own = std::process::id();
    let own_session = process(Path::new("/proc"), own).map(|process| process.session);
    if session == 0 || session == own || Some(session) == own_session {
        return Vec::new();
    }
    let proc_root = Path::new("/proc");
    let start = Instant::now();
    loop {
        let left = members(proc_root, session);
        if left.is_empty() {
            return Vec::new();
        }
        for member in &left {
            if member.pid == own {
                continue;
            }
            kill(member.pid, Signal::KILL);
        }
        // NOTE: a real-time grace, as the launcher's own: a process that SIGKILL ends is
        // gone within a few milliseconds, and a stopped clock of a test must not keep
        // the sweep waiting forever.
        if start.elapsed() >= SWEEP_GRACE {
            let mut names: Vec<String> =
                members(proc_root, session).into_iter().map(|member| member.name).collect();
            names.sort();
            names.dedup();
            return names;
        }
        std::thread::sleep(SWEEP_POLL);
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
