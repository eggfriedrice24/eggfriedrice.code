//! The model-side peer check on real processes: a child tree stands in for a hidden
//! zsh, its descendants and its session. The ends of the walk run over a table.

use std::collections::HashMap;
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
    assert_eq!(side(Some(own), &[], own, None), PeerSide::User);
    // A shell pid that is one of efrd's own ancestors never counts, as with a fake
    // holder whose made-up pids may name a real process.
    let parent = super::parent(own).unwrap();
    assert_eq!(side(Some(own), &[parent, own], own, None), PeerSide::User);
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
    assert_eq!(side(Some(grandchild), &[zsh], own, None), PeerSide::ModelSide);
    assert_eq!(side(Some(grandchild), &[], own, None), PeerSide::User);
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
    // Wait until the double fork is done: the orphan's parent is outside the session.
    // The subshell in the middle lives only for a moment. A test that takes any member
    // whose parent is not the shell can take that subshell just as it ends, and then
    // the check sees a gone process.
    let orphan = eventually(|| {
        session_members(zsh).into_iter().find(|pid| {
            matches!(super::parent(*pid), Some(parent) if super::session(parent) != Some(zsh))
        })
    });
    let own = std::process::id();
    assert_eq!(side(Some(orphan), &[zsh], own, None), PeerSide::ModelSide);
    end_session(zsh);
    shell.kill().await.unwrap();
}

#[test]
fn gone_pid_gets_read_only() {
    let own = std::process::id();
    // pid_max is at most 2^22, so this pid never exists.
    assert_eq!(side(Some(4_194_305), &[], own, None), PeerSide::Unknown);
    assert_eq!(side(None, &[], own, None), PeerSide::Unknown);
}

/// A process table: pid to `(parent, session)`, and the cgroup of each pid that has
/// one.
struct Table(HashMap<u32, (u32, u32)>, HashMap<u32, String>);

impl super::Processes for Table {
    fn parent(&self, pid: u32) -> Option<u32> {
        self.0.get(&pid).map(|(parent, _)| *parent)
    }

    fn session(&self, pid: u32) -> Option<u32> {
        self.0.get(&pid).map(|(_, session)| *session)
    }

    fn cgroup(&self, pid: u32) -> Option<String> {
        self.1.get(&pid).cloned()
    }
}

/// efrd is 50, below 1; a hidden shell is 60, below efrd. The peer is the bottom of a
/// line of `len` processes from 1000 up, whose top has `top` as its parent. Every
/// process is in session 7, so only the walk decides.
fn table(len: u32, top: u32) -> Table {
    let mut procs = HashMap::from([(1, (0, 1)), (50, (1, 7)), (60, (50, 7))]);
    for pid in 1000..1000 + len {
        let parent = if pid == 1000 + len - 1 { top } else { pid + 1 };
        procs.insert(pid, (parent, 7));
    }
    Table(procs, HashMap::new())
}

#[test]
fn the_walk_clears_a_peer_only_at_the_top_or_efrds_own_chain() {
    let peer = Some(1000);
    let side = |table: &Table| super::side_in(table, peer, &[60], 50, None);
    // The top of the tree, a parent outside efrd's pid namespace and efrd's own chain.
    assert_eq!(side(&table(3, 1)), PeerSide::User);
    assert_eq!(side(&table(3, 0)), PeerSide::User);
    assert_eq!(side(&table(3, 50)), PeerSide::User);
    assert_eq!(side(&table(3, 60)), PeerSide::ModelSide);
    // A parent that is gone, and a line longer than the walk follows, are unclear.
    assert_eq!(side(&table(3, 999)), PeerSide::Unknown);
    let deep = u32::try_from(super::MAX_DEPTH).unwrap() + 10;
    assert_eq!(side(&table(deep, 1)), PeerSide::Unknown);
    assert_eq!(side(&table(deep, 60)), PeerSide::Unknown);
}

/// efrd is 50, below 1, in session 7; a job's launcher is 70, below efrd, and leads its
/// own session 70 (it calls `setsid`). efrd passes the launcher's pid with the hidden
/// shells (`methods::model_side_roots`).
fn job_table() -> Table {
    Table(
        HashMap::from([
            (1, (0, 1)),
            (50, (1, 7)),
            (70, (50, 70)),
            // A process of the job, two levels down.
            (71, (70, 70)),
            (72, (71, 70)),
            // A process of the job that went to pid 1 after a double fork.
            (73, (1, 70)),
            // A process of the user in efrd's session.
            (80, (1, 7)),
        ]),
        HashMap::new(),
    )
}

#[test]
fn job_launcher_pid_is_model_side_peer() {
    let table = job_table();
    let side = |pid| super::side_in(&table, Some(pid), &[70], 50, None);
    assert_eq!(side(70), PeerSide::ModelSide, "the launcher itself");
    assert_eq!(side(72), PeerSide::ModelSide, "a process below it");
    assert_eq!(side(73), PeerSide::ModelSide, "an orphan that kept its session");
    assert_eq!(side(80), PeerSide::User);
    // NOTE: no hidden shell runs: the job's launcher alone makes the list.
    assert_eq!(super::side_in(&table, Some(80), &[], 50, None), PeerSide::User);
}

#[test]
fn exit_child_job_orphan_after_launcher_kill_is_model_side() {
    // NOTE: the launcher 70 is gone (killed by the job); its orphan 73 went to pid 1 and
    // still has the session 70, which efrd keeps model-side while a process has it.
    let mut table = job_table();
    table.0.remove(&70);
    table.0.remove(&71);
    table.0.remove(&72);
    assert_eq!(super::side_in(&table, Some(73), &[70], 50, None), PeerSide::ModelSide);
    // Without the session in the list, the orphan would pass as the user's.
    assert_eq!(super::side_in(&table, Some(73), &[], 50, None), PeerSide::User);
}

/// The cgroup of efrd under the systemd user unit.
const UNIT: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice/efrd.service";

/// efrd is 50, below 1, in the unit's cgroup. 73 is a process of an exit child job
/// that called `setsid` and lost its launcher: its own session, a parent of 1, and
/// still efrd's cgroup. 80 is a process of the user in a terminal's scope.
fn unit_table() -> Table {
    let terminal = "/user.slice/user-1000.slice/user@1000.service/app.slice/app-ghostty.scope";
    Table(
        HashMap::from([(1, (0, 1)), (50, (1, 7)), (73, (1, 73)), (80, (1, 80))]),
        HashMap::from([(50, UNIT.to_owned()), (73, UNIT.to_owned()), (80, terminal.to_owned())]),
    )
}

#[test]
fn peer_in_efrd_cgroup_is_model_side() {
    let table = unit_table();
    let side = |pid| super::side_in(&table, Some(pid), &[], 50, Some(UNIT));
    assert_eq!(side(73), PeerSide::ModelSide, "an orphan of a job in efrd's cgroup");
    assert_eq!(side(80), PeerSide::User, "a process of a terminal");
    assert_eq!(side(50), PeerSide::User, "efrd itself");
    // The unit's cgroup comes from the `0::` line; only the unit's own name counts.
    let file = format!("12:pids:/x\n0::{UNIT}\n");
    assert_eq!(super::unit_of(super::cgroup_path(&file)).as_deref(), Some(UNIT));
    assert_eq!(super::unit_of(Some("/app.slice/efrd.service/sub".to_owned())), None);
    assert_eq!(super::unit_of(Some("/app.slice/my-efrd.service".to_owned())), None);
}

/// With the job cgroups of ADR 0021, efrd runs in `<unit>/daemon` and each job in
/// `<unit>/jobs/<job>`: a peer in any cgroup below the unit's, but efrd, is model-side,
/// and a cgroup whose name only starts with the unit's is not below it.
#[test]
fn peer_below_efrd_unit_cgroup_is_model_side() {
    let mut table = unit_table();
    let below = [
        (74, format!("{UNIT}/jobs/0192f0c1-7a00-7000-8000-000000000001-j3")),
        (75, format!("{UNIT}/daemon")),
        (76, format!("{UNIT}-other")),
    ];
    for (pid, cgroup) in below {
        table.0.insert(pid, (1, pid));
        table.1.insert(pid, cgroup);
    }
    table.1.insert(50, format!("{UNIT}/daemon"));
    let side = |pid| super::side_in(&table, Some(pid), &[], 50, Some(UNIT));
    assert_eq!(side(74), PeerSide::ModelSide, "a process of a job's cgroup");
    assert_eq!(side(75), PeerSide::ModelSide, "a process in efrd's own cgroup");
    assert_eq!(side(76), PeerSide::User, "a cgroup that only starts with the unit's name");
    assert_eq!(side(50), PeerSide::User, "efrd itself");
}

#[test]
fn peer_rule_off_for_a_hand_started_efrd() {
    // NOTE: an efrd that the user started in a terminal has the terminal's cgroup, and
    // so has every process of that terminal: the rule must not apply.
    let terminal = "/user.slice/user-1000.slice/session-1.scope";
    let mut table = unit_table();
    table.1.insert(50, terminal.to_owned());
    table.1.insert(80, terminal.to_owned());
    let unit = super::unit_of(table.1.get(&50).cloned());
    assert_eq!(unit, None);
    assert_eq!(super::side_in(&table, Some(80), &[], 50, unit.as_deref()), PeerSide::User);
    assert_eq!(super::side_in(&table, Some(73), &[], 50, unit.as_deref()), PeerSide::User);
}

#[test]
fn cgroup_rule_never_raises_a_peer() {
    let mut table = unit_table();
    // A hidden shell 60 below efrd, with a child 61 in efrd's cgroup and a child 62 in
    // another cgroup; 90 is gone.
    table.0.insert(60, (50, 60));
    table.0.insert(61, (60, 60));
    table.0.insert(62, (60, 60));
    table.1.insert(61, UNIT.to_owned());
    table.1.insert(62, "/elsewhere.scope".to_owned());
    let side = |pid| super::side_in(&table, Some(pid), &[60], 50, Some(UNIT));
    assert_eq!(side(61), PeerSide::ModelSide);
    assert_eq!(side(62), PeerSide::ModelSide, "another cgroup does not make it the user's");
    assert_eq!(side(90), PeerSide::Unknown, "a gone peer stays unknown");
    assert_eq!(super::side_in(&table, None, &[60], 50, Some(UNIT)), PeerSide::Unknown);
}
