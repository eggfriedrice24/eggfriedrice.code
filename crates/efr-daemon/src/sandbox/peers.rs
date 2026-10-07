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

use std::fs;

/// How many parents the walk follows at most.
const MAX_DEPTH: usize = 4096;

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

/// Where the peer `pid` stands, with `shells` the hidden shells' pids and `own` efrd's
/// pid. It reads `/proc`, so it blocks for a few small reads.
pub(crate) fn side(pid: Option<u32>, shells: &[u32], own: u32) -> PeerSide {
    side_in(&Proc, pid, shells, own)
}

/// [`side`] over `processes`.
fn side_in(processes: &dyn Processes, pid: Option<u32>, shells: &[u32], own: u32) -> PeerSide {
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
    // NOTE: read once more, so a peer that ended during the walk counts as gone.
    if processes.parent(pid).is_none() { PeerSide::Unknown } else { PeerSide::User }
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

#[cfg(test)]
mod tests;
