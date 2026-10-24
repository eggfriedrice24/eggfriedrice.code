//! Model-side peers on the daemon's socket (efr's auto spec, section 13.5).
//!
//! A sandboxed call never reaches the socket: efr's runtime root is masked. But a
//! command in the exit child, or one that a `cautious` approval ran in the hidden
//! shell, runs with the user's rights and could run `efr` itself to approve its own
//! next call. So a peer whose process descends from a hidden zsh (efrd walks `PPid` in
//! `/proc/<pid>/status`), or that shares a hidden zsh's session (`getsid`), gets the
//! `read` scope only. The hidden zsh and the exit child's launcher are child
//! subreapers, so an orphan of the model's commands stays below them even after a
//! double fork, and a `setsid` one keeps the session or the parent chain. A peer whose
//! process is gone gets the reduced set too: the check fails closed.
//!
//! efrd's own ancestors never count: a hidden shell is efrd's descendant, never its
//! ancestor, so the walk stops where it meets efrd's own chain, and the session check
//! ignores efrd's own session.
//!
//! The walk up from the peer ends in one of five ways ([`WalkEnd`]). It clears the
//! peer only when it meets efrd's own chain or the top of the tree (pid 1, or a parent
//! of 0 outside efrd's pid namespace). A hidden shell on the way makes the peer
//! model-side. A parent that cannot be read, or a walk that does not end within
//! [`MAX_DEPTH`] steps, is unclear, and an unclear peer gets the reduced set.
//!
//! One more rule applies only under the systemd user unit (DV49). A process of an exit
//! child job that calls `setsid` and loses its launcher leaves the job's session and
//! the launcher's tree, so the walk clears it. But it stays in the cgroup of
//! `efrd.service`. So when efrd's own cgroup ends in [`UNIT_SUFFIX`] ([`unit_cgroup`]),
//! a peer in that same cgroup, or in a cgroup below it, that is not efrd itself is
//! model-side: with its job cgroups (ADR 0021) efrd runs in `<unit>/daemon` and each job
//! in `<unit>/jobs/<job>`. A hand-started efrd and a test daemon have no unit cgroup, and
//! the rule is off. The rule only reduces a peer that the walk cleared; it never makes a
//! peer the user's.

use std::fs;

/// How many parents the walk follows at most.
const MAX_DEPTH: usize = 4096;

/// The end of efrd's cgroup path when the systemd user unit runs it, such as
/// `/user.slice/user-1000.slice/user@1000.service/app.slice/efrd.service`.
const UNIT_SUFFIX: &str = "/efrd.service";

/// Who a socket peer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerSide {
    /// The user's own processes: every scope of its surface.
    User,
    /// A descendant or session member of a hidden shell: `read` only.
    ModelSide,
    /// The process is gone, its pid is unknown, or the walk up from it did not end
    /// clearly: `read` only.
    Unknown,
}

/// What the check reads about processes: `/proc` in efrd, a table in the tests.
trait Processes {
    /// The parent of `pid`, or `None` when the process is gone.
    fn parent(&self, pid: u32) -> Option<u32>;
    /// The session of `pid`, or `None` when it cannot be read or is 0.
    fn session(&self, pid: u32) -> Option<u32>;
    /// The cgroup v2 path of `pid`, or `None` when it cannot be read.
    fn cgroup(&self, pid: u32) -> Option<String>;
}

/// The processes of efrd's `/proc`.
struct Proc;

impl Processes for Proc {
    fn parent(&self, pid: u32) -> Option<u32> {
        parent(pid)
    }

    fn session(&self, pid: u32) -> Option<u32> {
        session(pid)
    }

    fn cgroup(&self, pid: u32) -> Option<String> {
        cgroup(pid)
    }
}

/// How the walk up from a peer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalkEnd {
    /// It met efrd's own chain.
    OwnChain,
    /// It reached pid 1, or a parent of 0.
    Top,
    /// It met a hidden shell.
    Shell,
    /// A process on the way is gone.
    Lost,
    /// It took [`MAX_DEPTH`] steps and did not end.
    TooDeep,
}

/// Where the peer `pid` stands, with `shells` the hidden shells' pids, `own` efrd's
/// pid and `unit` efrd's cgroup under the systemd unit ([`unit_cgroup`]). It reads
/// `/proc`, so it blocks for a few small reads.
pub(crate) fn side(pid: Option<u32>, shells: &[u32], own: u32, unit: Option<&str>) -> PeerSide {
    side_in(&Proc, pid, shells, own, unit)
}

/// efrd's own cgroup when the systemd user unit runs it: the path of the `0::` line of
/// `/proc/self/cgroup` when it ends in [`UNIT_SUFFIX`]; `None` for an efrd that was
/// started by hand, or when the file cannot be read.
pub(crate) fn unit_cgroup() -> Option<String> {
    let text = fs::read_to_string("/proc/self/cgroup").ok()?;
    unit_of(cgroup_path(&text))
}

/// `cgroup` when it is the cgroup of the systemd unit.
fn unit_of(cgroup: Option<String>) -> Option<String> {
    cgroup.filter(|path| path.ends_with(UNIT_SUFFIX))
}

/// [`side`] over `processes`.
fn side_in(
    processes: &dyn Processes,
    pid: Option<u32>,
    shells: &[u32],
    own: u32,
    unit: Option<&str>,
) -> PeerSide {
    let Some(pid) = pid else { return PeerSide::Unknown };
    if processes.parent(pid).is_none() {
        return PeerSide::Unknown;
    }
    if !shells.is_empty() {
        if let (Some(peer_session), Some(own_session)) =
            (processes.session(pid), processes.session(own))
            && peer_session != own_session
            && shells.contains(&peer_session)
        {
            return PeerSide::ModelSide;
        }
        match walk(processes, pid, shells, &chain(processes, own)) {
            WalkEnd::Shell => return PeerSide::ModelSide,
            WalkEnd::Lost | WalkEnd::TooDeep => return PeerSide::Unknown,
            WalkEnd::OwnChain | WalkEnd::Top => {}
        }
    }
    // NOTE: only a peer that the walk cleared gets here, so the rule can only reduce it.
    if let Some(unit) = unit
        && pid != own
        && processes.cgroup(pid).is_some_and(|cgroup| in_unit(&cgroup, unit))
    {
        return PeerSide::ModelSide;
    }
    // NOTE: read once more, so a peer that ended during the walk counts as gone.
    if processes.parent(pid).is_none() { PeerSide::Unknown } else { PeerSide::User }
}

/// True when `cgroup` is the unit's cgroup `unit` or a cgroup below it: efrd's own
/// `daemon` and each job's cgroup below `jobs` (ADR 0021).
fn in_unit(cgroup: &str, unit: &str) -> bool {
    cgroup.strip_prefix(unit).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Walks up from `pid` until one of the ends of [`WalkEnd`].
fn walk(processes: &dyn Processes, pid: u32, shells: &[u32], own_chain: &[u32]) -> WalkEnd {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if own_chain.contains(&current) {
            return WalkEnd::OwnChain;
        }
        if shells.contains(&current) {
            return WalkEnd::Shell;
        }
        match processes.parent(current) {
            None => return WalkEnd::Lost,
            Some(0 | 1) => return WalkEnd::Top,
            Some(up) => current = up,
        }
    }
    WalkEnd::TooDeep
}

/// `pid` and its parents up to pid 1.
fn chain(processes: &dyn Processes, pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        match processes.parent(current) {
            Some(up) if up > 0 && !chain.contains(&up) => {
                chain.push(up);
                current = up;
            }
            _ => break,
        }
    }
    chain
}

/// The parent of `pid` from `/proc/<pid>/status`, or `None` when the process is gone.
fn parent(pid: u32) -> Option<u32> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))
        .and_then(|value| value.trim().parse().ok())
}

/// The session of `pid`, from `/proc/<pid>/stat`: the fourth field after the command
/// name, which ends at the last `)`. Read from `/proc` rather than through `getsid`,
/// because a kernel thread's session is 0.
fn session(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(3)?.parse().ok().filter(|session| *session > 0)
}

/// The cgroup v2 path of `pid`, from `/proc/<pid>/cgroup`.
fn cgroup(pid: u32) -> Option<String> {
    cgroup_path(&fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?)
}

/// The path of the cgroup v2 line (`0::<path>`) of the text of a `cgroup` file.
fn cgroup_path(text: &str) -> Option<String> {
    text.lines().find_map(|line| line.strip_prefix("0::")).map(str::to_owned)
}

#[cfg(test)]
mod tests;
