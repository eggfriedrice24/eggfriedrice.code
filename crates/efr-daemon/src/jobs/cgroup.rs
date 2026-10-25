//! A cgroup for each job (ADR 0021): efrd holds the processes of each job in a cgroup
//! of its own below efrd's delegated cgroup, so the stop of a job kills every process of
//! it, also one that left its session, and the job's memory and CPU time can be seen.
//!
//! The systemd user unit has `Delegate=yes`, so systemd gives efrd the subtree of its
//! unit's cgroup `U`. At its start, before it starts any process of its own, efrd:
//!
//! 1. reads its own cgroup from `/proc/self/cgroup` and finds out whether `U` is
//!    delegated ([`delegation`]): a `user.delegate` or `trusted.delegate` mark of `1` on
//!    `U` is a yes at once. systemd's docs say that the service manager sets
//!    `user.delegate`, but systemd 262's user manager sets no mark on its units (only
//!    the system manager marks `user@<uid>.service`). So without a mark efrd asks the
//!    manager itself, once, with a time limit: `systemctl --user show` of the unit named
//!    by `U`'s last part, from the trusted `systemctl` (`trusted_programs.rs`). Only
//!    `LoadState=loaded`, `MainPID` equal to efrd's pid and `Delegate=yes` is a yes;
//!    every other answer, an error and no answer in time are a no;
//! 2. moves itself into `U/daemon`: cgroup v2 lets a cgroup give its controllers to
//!    its children only when it holds no process itself;
//! 3. gives `U` and `U/jobs` the `memory`, `cpu` and `pids` controllers that `U` has;
//! 4. kills and removes what a job of the last efrd left in `U/jobs`.
//!
//! For each job, efrd makes `U/jobs/<conversation>-j<n>` and moves the launcher there
//! before it writes the start byte, so every process that the launcher starts is born
//! in the cgroup. The launcher needs no change for this. A moved call's old hidden
//! shell and every process below it move there at the move. The end of the job writes
//! `1` to `cgroup.kill` (Linux 5.14), which kills every process of the cgroup, waits for
//! `cgroup.events` to say `populated 0` and removes the dir. The sweep by the job's
//! session still runs too.
//!
//! When the subtree is not delegated (an efrd started by hand, a test, an older unit
//! without `Delegate=yes`, or no cgroup v2), efrd holds a job's processes by its session
//! only, as before, and `JobCgroup::off` of the job says why. Only plain cgroup v2 files
//! are read and written; no library.
//!
//! NOTE: a job in the exit child runs as the user, who owns the delegated subtree, so
//! it can move its own processes out of its cgroup on purpose. The session sweep and
//! the peer check of `sandbox/peers.rs` (every process in `U` but efrd is model-side)
//! still hold it.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use efr_protocol::JobCgroup;
use efr_stdx::time::Clock;

/// Where the cgroup v2 file system is.
pub(crate) const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// The child of efrd's unit cgroup that holds efrd itself and its hidden shells.
const DAEMON_DIR: &str = "daemon";

/// The child of efrd's unit cgroup that holds the job cgroups.
const JOBS_DIR: &str = "jobs";

/// The controllers that efrd gives the job cgroups, when its cgroup has them.
const CONTROLLERS: &[&str] = &["memory", "cpu", "pids"];

/// The extended attributes that mark a delegated cgroup, `user.delegate` as systemd's
/// docs name it and `trusted.delegate` of the system manager.
const DELEGATE_MARKS: &[&str] = &["user.delegate", "trusted.delegate"];

/// How long efrd waits for `systemctl` to tell whether its unit is delegated.
pub(crate) const ASK_LIMIT: Duration = Duration::from_secs(2);

/// How long the kill of a job's cgroup waits for its processes to go.
const KILL_GRACE: Duration = Duration::from_secs(1);

/// How often that wait looks again.
const KILL_POLL: Duration = Duration::from_millis(20);

/// The reason of a job with no cgroup when efrd does not run as the systemd unit.
pub(crate) const NOT_A_UNIT: &str = "efrd does not run as its systemd unit";

/// The reason of a job with no cgroup when the unit's cgroup is not delegated.
pub(crate) const NOT_DELEGATED: &str = "efrd's cgroup is not delegated (Delegate=yes)";

/// The cgroups of the jobs: the dir that holds them, or why there is none.
#[derive(Debug, Clone)]
pub(crate) struct Cgroups {
    held: Result<PathBuf, String>,
}

impl Cgroups {
    /// No job cgroups, for `reason`.
    pub(crate) fn off(reason: impl Into<String>) -> Cgroups {
        Cgroups { held: Err(reason.into()) }
    }

    /// The job cgroups in `jobs`, a dir that a test made: no check, no move.
    #[cfg(test)]
    pub(crate) fn in_dir(jobs: PathBuf) -> Cgroups {
        Cgroups { held: Ok(jobs) }
    }

    /// At efrd's start: the job cgroups below efrd's own cgroup when it is delegated
    /// ([`delegation`], with `systemctl` the command of the trusted `systemctl`, `None`
    /// when efrd found none), after efrd moved itself into `U/daemon` and gave the
    /// controllers on. Else no job cgroups, with the reason.
    pub(crate) async fn setup(
        systemctl: Option<tokio::process::Command>,
        clock: &Arc<dyn Clock>,
    ) -> Cgroups {
        let own = match tokio::fs::read_to_string("/proc/self/cgroup").await {
            Ok(text) => own_cgroup(&text),
            Err(_) => None,
        };
        let Some(own) = own else { return Cgroups::off("efrd found no cgroup v2") };
        let unit = Path::new(CGROUP_ROOT).join(own.trim_start_matches('/'));
        let pid = std::process::id();
        if let Err(reason) = delegation(&unit, &own, systemctl, pid, clock, ASK_LIMIT).await {
            return Cgroups::off(reason);
        }
        let moved = tokio::task::spawn_blocking(move || setup_in(&unit, pid)).await;
        let moved = moved.unwrap_or_else(|_| Err(io::Error::other("the setup task ended")));
        match moved {
            Ok(jobs) => {
                tracing::info!(dir = %jobs.display(), "background jobs run in cgroups of their own");
                Cgroups { held: Ok(jobs) }
            }
            Err(error) => {
                tracing::warn!(%error, "efrd could not set up the cgroups of its jobs; jobs end by their session only");
                Cgroups::off(format!("efrd could not set up its cgroups: {error}"))
            }
        }
    }

    /// Why the jobs have no cgroup, when they have none.
    pub(crate) fn reason(&self) -> Option<&str> {
        self.held.as_ref().err().map(String::as_str)
    }

    /// Makes the cgroup `name` of a job. `Err` is why the job has none: the job then
    /// runs with its session only.
    pub(crate) fn create(&self, name: &str) -> Result<JobGroup, String> {
        let jobs = self.held.as_ref().map_err(Clone::clone)?;
        let dir = jobs.join(name);
        match fs::create_dir(&dir) {
            Ok(()) => Ok(JobGroup { dir }),
            // NOTE: a dir of the same name is left from a job that ended badly; its
            // processes are not this job's.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let left = JobGroup { dir: dir.clone() };
                left.kill();
                left.remove();
                fs::create_dir(&dir)
                    .map(|()| JobGroup { dir })
                    .map_err(|error| format!("efrd could not make the job's cgroup: {error}"))
            }
            Err(error) => Err(format!("efrd could not make the job's cgroup: {error}")),
        }
    }

    /// At efrd's start, before any job runs: kills and removes every job cgroup that the
    /// last efrd left. Returns how many it found.
    pub(crate) fn clean(&self) -> usize {
        let Ok(jobs) = &self.held else { return 0 };
        let Ok(entries) = fs::read_dir(jobs) else { return 0 };
        let mut found = 0;
        for entry in entries.filter_map(Result::ok) {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                let left = JobGroup { dir: entry.path() };
                left.kill();
                left.remove();
                found += 1;
            }
        }
        found
    }
}

/// The name of the cgroup of the job `number` of `conversation`.
pub(crate) fn job_name(conversation: efr_protocol::ConversationId, number: u32) -> String {
    format!("{conversation}-j{number}")
}

/// The cgroup v2 path of the `0::` line of the text of a `/proc/<pid>/cgroup` file.
pub(crate) fn own_cgroup(text: &str) -> Option<String> {
    text.lines().find_map(|line| line.strip_prefix("0::")).map(str::to_owned)
}

/// Whether efrd's cgroup `own` (its path in the cgroup tree, at `dir`) is delegated to
/// it: `Ok` for a delegate mark on `dir`, else for systemd's own yes. Without a mark it
/// asks the user manager with `systemctl` (`None`: efrd found no trusted one) about the
/// unit that the last part of `own` names, within `limit`. Only `LoadState=loaded`,
/// `MainPID` equal to `pid` and `Delegate=yes` is a yes. `Err` is the reason of the no.
pub(crate) async fn delegation(
    dir: &Path,
    own: &str,
    systemctl: Option<tokio::process::Command>,
    pid: u32,
    clock: &Arc<dyn Clock>,
    limit: Duration,
) -> Result<(), String> {
    if delegated(dir) {
        return Ok(());
    }
    let unit = own.rsplit('/').next().unwrap_or_default();
    if !unit_name(unit) {
        return Err(NOT_A_UNIT.to_owned());
    }
    let Some(mut command) = systemctl else {
        return Err(
            "efrd found no trusted systemctl to ask whether its cgroup is delegated".to_owned()
        );
    };
    // NOTE: `systemctl --user` finds the user manager by `$XDG_RUNTIME_DIR`, which the
    // clean environment of a trusted program lacks.
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw()))
        });
    command
        .env("XDG_RUNTIME_DIR", runtime)
        .args(["--user", "show", "-p", "LoadState", "-p", "MainPID", "-p", "Delegate", "--"])
        .arg(unit)
        .kill_on_drop(true);
    let output = match clock.timeout(limit, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(format!("efrd could not run systemctl: {error}")),
        Err(_) => {
            return Err(format!(
                "systemctl did not tell within {}s whether efrd's cgroup is delegated",
                limit.as_secs()
            ));
        }
    };
    if !output.status.success() {
        return Err(format!(
            "systemctl could not tell whether efrd's cgroup is delegated ({})",
            output.status
        ));
    }
    systemd_says(&String::from_utf8_lossy(&output.stdout), pid)
}

/// systemd's answer `text` (`Key=value` lines of `systemctl show`) about efrd's unit,
/// whose main process must be `pid`.
pub(crate) fn systemd_says(text: &str, pid: u32) -> Result<(), String> {
    let value =
        |key: &str| text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='));
    if value("LoadState") != Some("loaded") {
        return Err("systemd does not know efrd's unit".to_owned());
    }
    if value("MainPID").and_then(|main| main.parse::<u32>().ok()) != Some(pid) {
        return Err("efrd is not the main process of its unit".to_owned());
    }
    match value("Delegate") {
        Some("yes") => Ok(()),
        _ => Err(NOT_DELEGATED.to_owned()),
    }
}

/// True when `name` is a name of a systemd service unit that efrd may pass to
/// `systemctl`: letters, digits and `@._:-`, ending in `.service`.
fn unit_name(name: &str) -> bool {
    name.len() > ".service".len()
        && name.len() <= 256
        && name.ends_with(".service")
        && !name.starts_with('-')
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"@._:-".contains(&byte))
}

/// True when the cgroup dir `dir` carries a mark of a delegated subtree.
fn delegated(dir: &Path) -> bool {
    DELEGATE_MARKS.iter().any(|mark| {
        let mut value = [0_u8; 8];
        rustix::fs::getxattr(dir, *mark, &mut value[..])
            .is_ok_and(|len| value.get(..len).is_some_and(|value| value == b"1"))
    })
}

/// Moves the process `pid` from the unit's cgroup `unit` into `unit/daemon`, gives the
/// controllers on to `unit/jobs` and returns that dir.
pub(crate) fn setup_in(unit: &Path, pid: u32) -> io::Result<PathBuf> {
    let daemon = unit.join(DAEMON_DIR);
    let jobs = unit.join(JOBS_DIR);
    for dir in [&daemon, &jobs] {
        match fs::create_dir(dir) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
    }
    write_file(&daemon.join("cgroup.procs"), &pid.to_string())?;
    // NOTE: a controller that the unit lacks only costs the numbers that it would give;
    // a job cgroup without it still kills every process.
    let available = fs::read_to_string(unit.join("cgroup.controllers")).unwrap_or_default();
    let wanted: Vec<String> = CONTROLLERS
        .iter()
        .filter(|name| available.split_whitespace().any(|found| found == **name))
        .map(|name| format!("+{name}"))
        .collect();
    if !wanted.is_empty() {
        let line = wanted.join(" ");
        for dir in [unit, jobs.as_path()] {
            if let Err(error) = write_file(&dir.join("cgroup.subtree_control"), &line) {
                tracing::warn!(%error, dir = %dir.display(), "a cgroup did not take the controllers; the job numbers may be missing");
                break;
            }
        }
    }
    Ok(jobs)
}

/// Writes `text` to the cgroup file `path`. The kernel makes every interface file of a
/// cgroup and refuses a new name, so the create only serves the plain files of a test.
fn write_file(path: &Path, text: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(false).open(path)?;
    file.write_all(text.as_bytes())
}

/// The cgroup of one job.
#[derive(Debug, Clone)]
pub(crate) struct JobGroup {
    dir: PathBuf,
}

impl JobGroup {
    /// The job's cgroup dir.
    #[cfg(test)]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Moves the process `pid` into the cgroup.
    pub(crate) fn add(&self, pid: u32) -> io::Result<()> {
        write_file(&self.dir.join("cgroup.procs"), &pid.to_string())
    }

    /// The memory and the CPU time of the job now; at the end the most memory that it
    /// used, when the kernel says.
    pub(crate) fn usage(&self, ended: bool) -> JobCgroup {
        let memory = if ended { "memory.peak" } else { "memory.current" };
        let memory_bytes = fs::read_to_string(self.dir.join(memory))
            .ok()
            .and_then(|text| text.trim().parse().ok());
        let cpu_usec = fs::read_to_string(self.dir.join("cpu.stat"))
            .ok()
            .and_then(|text| stat_value(&text, "usage_usec"));
        JobCgroup { off: None, memory_bytes, cpu_usec }
    }

    /// True while a process is in the cgroup, by `cgroup.events`; also true when the
    /// file cannot be read.
    pub(crate) fn populated(&self) -> bool {
        match fs::read_to_string(self.dir.join("cgroup.events")) {
            Ok(text) => stat_value(&text, "populated") != Some(0),
            Err(_) => true,
        }
    }

    /// Kills every process of the cgroup: `cgroup.kill`, else SIGKILL to each pid of
    /// `cgroup.procs` through a pidfd. Waits up to [`KILL_GRACE`] for them to go and
    /// returns true when the cgroup is empty. It blocks.
    pub(crate) fn kill(&self) -> bool {
        let begin = Instant::now();
        loop {
            if !self.populated() {
                return true;
            }
            if write_file(&self.dir.join("cgroup.kill"), "1").is_err() {
                for pid in self.pids() {
                    super::sweep::kill(pid, rustix::process::Signal::KILL);
                }
            }
            if begin.elapsed() >= KILL_GRACE {
                return !self.populated();
            }
            std::thread::sleep(KILL_POLL);
        }
    }

    /// The names of the processes that are still in the cgroup, each once, sorted.
    pub(crate) fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .pids()
            .into_iter()
            .filter_map(|pid| fs::read_to_string(format!("/proc/{pid}/comm")).ok())
            .map(|name| name.trim().to_owned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The pids of `cgroup.procs`.
    fn pids(&self) -> Vec<u32> {
        fs::read_to_string(self.dir.join("cgroup.procs"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
    }

    /// Removes the cgroup dir; a dir that still holds a process stays.
    pub(crate) fn remove(&self) {
        if let Err(error) = fs::remove_dir(&self.dir)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(%error, dir = %self.dir.display(), "a job's cgroup could not be removed");
        }
    }
}

/// The value of the key `key` of a flat-keyed cgroup file, such as `usage_usec 42` of
/// `cpu.stat` or `populated 0` of `cgroup.events`.
pub(crate) fn stat_value(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(' ')?;
        (name == key).then(|| value.trim().parse().ok()).flatten()
    })
}

#[cfg(test)]
mod tests;
