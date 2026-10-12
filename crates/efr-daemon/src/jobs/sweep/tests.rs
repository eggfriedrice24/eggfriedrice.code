use std::path::Path;
use std::process::Stdio;

use pretty_assertions::assert_eq;

use super::{Process, kill_session, members, parse_stat, process};

#[test]
fn stat_fields_after_a_name_with_spaces_and_parens() {
    let stat = "4242 (my (odd) name) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 39 19 1 0 \
                98765 1000 100 0 0 0 0";
    let found = parse_stat(4242, stat).unwrap();
    assert_eq!(
        found,
        Process {
            pid: 4242,
            name: "my (odd) name".to_owned(),
            state: 'S',
            session: 4242,
            start: 98765
        }
    );
    assert_eq!(parse_stat(1, "garbage"), None);
}

#[test]
fn own_process_reads_and_own_session_is_never_swept() {
    let me = process(Path::new("/proc"), std::process::id()).unwrap();
    assert!(me.start > 0);
    // NOTE: efrd's own session holds efrd: a sweep of it would end efrd itself.
    assert_eq!(kill_session(me.session), Vec::<String>::new());
    assert!(process(Path::new("/proc"), std::process::id()).is_some());
}

#[tokio::test]
async fn sweep_ends_every_process_of_a_session() {
    let dir = tempfile::tempdir().unwrap();
    // NOTE: `setsid` runs `sleep` in a new session of its own pid, as a job's launcher
    // makes one; a process that is no group leader keeps its pid there.
    let Ok(mut child) = efr_stdx::process::command("setsid", dir.path())
        .arg("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    else {
        return;
    };
    let pid = child.id().unwrap();
    efr_test_support::Wait::new("the new session")
        .until(|| members(Path::new("/proc"), pid).iter().any(|member| member.pid == pid))
        .await
        .unwrap();
    let left = tokio::task::spawn_blocking(move || kill_session(pid)).await.unwrap();
    assert_eq!(left, Vec::<String>::new());
    assert!(members(Path::new("/proc"), pid).is_empty());
    let status = child.wait().await.unwrap();
    assert!(!status.success());
}
