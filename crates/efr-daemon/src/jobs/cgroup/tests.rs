//! The job cgroups over plain files of a test dir, and over the real cgroup v2 tree
//! when the test runs in a delegated cgroup.

use std::fs;
use std::path::Path;

use efr_protocol::JobCgroup;
use pretty_assertions::assert_eq;

use super::{Cgroups, NOT_DELEGATED, own_cgroup, setup_in, stat_value};

/// Makes the interface files of a cgroup dir in a test dir.
fn fake_cgroup(dir: &Path, files: &[(&str, &str)]) {
    fs::create_dir_all(dir).unwrap();
    for name in ["cgroup.procs", "cgroup.subtree_control", "cgroup.kill"] {
        fs::write(dir.join(name), "").unwrap();
    }
    for (name, text) in files {
        fs::write(dir.join(name), text).unwrap();
    }
}

#[test]
fn own_cgroup_reads_the_v2_line() {
    let text =
        "12:pids:/old\n0::/user.slice/user-1000.slice/user@1000.service/app.slice/efrd.service\n";
    assert_eq!(
        own_cgroup(text).as_deref(),
        Some("/user.slice/user-1000.slice/user@1000.service/app.slice/efrd.service")
    );
    assert_eq!(own_cgroup("12:pids:/old\n"), None);
}

#[test]
fn the_model_reads_the_usage_of_a_cgroup() {
    let usage =
        JobCgroup { off: None, memory_bytes: Some(183_500_800), cpu_usec: Some(42_150_000) };
    assert_eq!(
        crate::jobs::usage_text(&usage).as_deref(),
        Some("175 MiB of memory, 42.1 s of CPU")
    );
    assert_eq!(crate::jobs::usage_text(&JobCgroup::default()), None);
    let off = JobCgroup { off: Some(NOT_DELEGATED.to_owned()), ..JobCgroup::default() };
    assert_eq!(crate::jobs::usage_text(&off), None);
}

#[test]
fn stat_value_reads_a_flat_key() {
    let text = "usage_usec 42100000\nuser_usec 1\nsystem_usec 2\n";
    assert_eq!(stat_value(text, "usage_usec"), Some(42_100_000));
    assert_eq!(stat_value("populated 0\nfrozen 0\n", "populated"), Some(0));
    assert_eq!(stat_value("populated 1\n", "frozen"), None);
}

#[test]
fn setup_moves_the_daemon_and_gives_on_the_controllers_it_has() {
    let dir = tempfile::tempdir().unwrap();
    let unit = dir.path().join("efrd.service");
    fake_cgroup(&unit, &[("cgroup.controllers", "cpuset cpu io memory pids\n")]);
    fake_cgroup(&unit.join("daemon"), &[]);
    fake_cgroup(&unit.join("jobs"), &[]);
    let jobs = setup_in(&unit, 4242).unwrap();
    assert_eq!(jobs, unit.join("jobs"));
    assert_eq!(fs::read_to_string(unit.join("daemon/cgroup.procs")).unwrap(), "4242");
    for dir in [&unit, &jobs] {
        let control = fs::read_to_string(dir.join("cgroup.subtree_control")).unwrap();
        assert_eq!(control, "+memory +cpu +pids", "{}", dir.display());
    }
}

#[test]
fn setup_of_a_unit_with_no_memory_controller_gives_what_it_has() {
    let dir = tempfile::tempdir().unwrap();
    let unit = dir.path().join("efrd.service");
    fake_cgroup(&unit, &[("cgroup.controllers", "pids\n")]);
    fake_cgroup(&unit.join("daemon"), &[]);
    fake_cgroup(&unit.join("jobs"), &[]);
    setup_in(&unit, 7).unwrap();
    assert_eq!(fs::read_to_string(unit.join("cgroup.subtree_control")).unwrap(), "+pids");
}

#[test]
fn off_cgroups_say_why_and_make_none() {
    let cgroups = Cgroups::off(NOT_DELEGATED);
    assert_eq!(cgroups.reason(), Some(NOT_DELEGATED));
    assert_eq!(cgroups.create("x-j1").unwrap_err(), NOT_DELEGATED);
    assert_eq!(cgroups.clean(), 0);
}

#[test]
fn a_job_cgroup_takes_its_launcher_and_tells_its_usage() {
    let dir = tempfile::tempdir().unwrap();
    let cgroups = Cgroups::in_dir(dir.path().to_path_buf());
    assert_eq!(cgroups.reason(), None);
    let group = cgroups.create("c-j1").unwrap();
    fake_cgroup(
        group.dir(),
        &[
            ("memory.current", "1048576\n"),
            ("memory.peak", "2097152\n"),
            ("cpu.stat", "usage_usec 1500\nuser_usec 1000\n"),
            ("cgroup.events", "populated 0\nfrozen 0\n"),
        ],
    );
    group.add(4242).unwrap();
    assert_eq!(fs::read_to_string(group.dir().join("cgroup.procs")).unwrap(), "4242");
    assert_eq!(
        group.usage(false),
        JobCgroup { off: None, memory_bytes: Some(1_048_576), cpu_usec: Some(1500) }
    );
    assert_eq!(group.usage(true).memory_bytes, Some(2_097_152));
    assert!(!group.populated());
    assert!(group.kill(), "an empty cgroup is killed at once");
}

#[test]
fn a_cgroup_with_no_numbers_says_none() {
    let dir = tempfile::tempdir().unwrap();
    let group = Cgroups::in_dir(dir.path().to_path_buf()).create("c-j2").unwrap();
    assert_eq!(group.usage(false), JobCgroup::default());
    // NOTE: a cgroup whose events cannot be read counts as one that holds a process.
    assert!(group.populated());
}

#[test]
fn clean_removes_the_cgroups_of_the_last_efrd() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a-j1", "b-j2"] {
        fake_cgroup(&dir.path().join(name), &[("cgroup.events", "populated 1\n")]);
    }
    fs::write(dir.path().join("a-j1/cgroup.events"), "populated 0\n").unwrap();
    fs::write(dir.path().join("cgroup.procs"), "").unwrap();
    assert_eq!(Cgroups::in_dir(dir.path().to_path_buf()).clean(), 2);
    // NOTE: the kill of a cgroup that holds a process writes `cgroup.kill`; an empty one
    // needs none. A plain dir of a test keeps its files, so it stays.
    assert_eq!(fs::read_to_string(dir.path().join("b-j2/cgroup.kill")).unwrap(), "1");
    assert_eq!(fs::read_to_string(dir.path().join("a-j1/cgroup.kill")).unwrap(), "");
}

/// The real kill: only in a delegated cgroup, such as a test run with
/// `systemd-run --user --scope -p Delegate=yes`; elsewhere the test passes with no
/// check, as `just test-sandbox` does on a machine with no sandbox.
#[tokio::test]
async fn a_real_job_cgroup_kills_a_process_that_left_its_session() {
    let own = fs::read_to_string("/proc/self/cgroup").ok().and_then(|text| own_cgroup(&text));
    let Some(own) = own else { return };
    let mine = Path::new(super::CGROUP_ROOT).join(own.trim_start_matches('/'));
    if !super::delegated(&mine) {
        return;
    }
    let cgroups = Cgroups::in_dir(mine.clone());
    let group = cgroups.create(&format!("efr-test-{}", std::process::id())).unwrap();
    // NOTE: `setsid` leaves the session of the test, as a job's escape would.
    let mut child = efr_stdx::process::command("setsid", "/")
        .args(["sleep", "60"])
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    group.add(child.id().unwrap()).unwrap();
    assert!(group.populated());
    assert!(group.kill(), "the kill of the cgroup ends the process");
    let _ = child.wait().await;
    group.remove();
    assert!(!group.dir().exists());
}
