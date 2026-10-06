//! The model-side peer check on real processes: a child tree stands in for a hidden
//! zsh, its descendants and its session.

use std::process::Stdio;
use std::time::Duration;

use super::{PeerSide, side};

/// Starts `script` under `sh` in a session of its own, as the holder starts a hidden
/// zsh, and returns it.
fn session_leader(script: &str) -> tokio::process::Child {
    efr_stdx::process::command("setsid", "/")
        .args(["sh", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

/// Kills every process of the session that `leader` leads, the orphans included.
fn end_session(leader: u32) {
    let mut members = session_members(leader);
    members.push(leader);
    for pid in members {
        if let Some(pid) = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
}

/// The pids of the processes whose parent is `parent`.
fn children(parent: u32) -> Vec<u32> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        if super::parent(pid) == Some(parent) {
            found.push(pid);
        }
    }
    found
}

/// The processes whose session is `session`, other than `session` itself.
fn session_members(session: u32) -> Vec<u32> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        if pid != session && super::session(pid) == Some(session) {
            found.push(pid);
        }
    }
    found
}

fn eventually<T>(mut look: impl FnMut() -> Option<T>) -> T {
    for _ in 0..500 {
        if let Some(found) = look() {
            return found;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the process tree did not form");
}

#[test]
fn the_daemons_own_process_and_its_parents_are_the_user() {
    let own = std::process::id();
    assert_eq!(side(Some(own), &[], own), PeerSide::User);
    // A shell pid that is one of efrd's own ancestors never counts, as with a fake
    // holder whose made-up pids may name a real process.
    let parent = super::parent(own).unwrap();
    assert_eq!(side(Some(own), &[parent, own], own), PeerSide::User);
}

#[tokio::test]
async fn hidden_shell_descendant_gets_read_only() {
    // The inner shell stays, because a command follows the sleep: the sleep is a
    // grandchild of the session leader.
    let mut shell = session_leader("sh -c 'sleep 30; true' & wait");
    let shell_pid = shell.id().unwrap();
    // setsid may fork once more when the caller leads a group; the shell is then its
    // child.
    let zsh = eventually(|| {
        let mut leaders = vec![shell_pid];
        leaders.extend(children(shell_pid));
        leaders.into_iter().find(|pid| super::session(*pid) == Some(*pid))
    });
    let grandchild = eventually(|| children(zsh).into_iter().flat_map(children).next());
    let own = std::process::id();
    assert_eq!(side(Some(grandchild), &[zsh], own), PeerSide::ModelSide);
    assert_eq!(side(Some(grandchild), &[], own), PeerSide::User);
    end_session(zsh);
    shell.kill().await.unwrap();
}

#[tokio::test]
async fn hidden_shell_session_peer_gets_read_only() {
    // The inner sleep is orphaned by a double fork: its parent chain leaves the shell,
    // but it keeps the shell's session.
    let mut shell = session_leader("(sleep 30 &) ; sleep 30");
    let shell_pid = shell.id().unwrap();
    let zsh = eventually(|| {
        let mut leaders = vec![shell_pid];
        leaders.extend(children(shell_pid));
        leaders.into_iter().find(|pid| super::session(*pid) == Some(*pid))
    });
    let orphan = eventually(|| {
        session_members(zsh).into_iter().find(|pid| super::parent(*pid) != Some(zsh))
    });
    let own = std::process::id();
    assert_eq!(side(Some(orphan), &[zsh], own), PeerSide::ModelSide);
    end_session(zsh);
    shell.kill().await.unwrap();
}

#[test]
fn gone_pid_gets_read_only() {
    let own = std::process::id();
    // pid_max is at most 2^22, so this pid never exists.
    assert_eq!(side(Some(4_194_305), &[], own), PeerSide::Unknown);
    assert_eq!(side(None, &[], own), PeerSide::Unknown);
}
