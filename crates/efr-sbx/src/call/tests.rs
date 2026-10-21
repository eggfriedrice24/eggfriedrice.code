//! How the launcher reports an error of its own.

use std::os::unix::fs::PermissionsExt;

use pretty_assertions::assert_eq;

use efr_sandbox::{LaunchTiming, MOVED_FILE, SandboxResult};

use super::{CallDir, STARTED_FILE, failure, moved, setup_failure, stopped_before_run};
use crate::testing::{TestCall, private_file};

#[test]
fn an_error_after_the_start_is_a_launch_error_not_a_setup_failure() {
    let call = TestCall::new();
    let dir = CallDir::open(call.call_dir()).unwrap();
    let before = failure(&dir, "the bind sources".to_owned());
    assert_eq!(before.setup_error.as_deref(), Some("the bind sources"));
    assert_eq!(before.launch_error, None);
    assert!(!before.started);

    // bwrap or the exit child was started: the command may have run.
    dir.dir.touch(STARTED_FILE).unwrap();
    let after = failure(&dir, "wait for bwrap".to_owned());
    assert!(after.started);
    assert_eq!(after.setup_error, None);
    assert_eq!(after.launch_error.as_deref(), Some("wait for bwrap"));
}

#[test]
fn a_setup_that_ctrl_c_broke_is_an_interrupt_not_a_setup_failure() {
    // bwrap got SIGINT before the command started.
    let mut broken = setup_failure("bwrap got signal 2 before the command started".to_owned());
    broken.started = true;
    broken.timings = vec![LaunchTiming { phase: "plan".to_owned(), us: 7 }];
    let stopped = stopped_before_run(broken, 2);
    assert_eq!(
        stopped,
        SandboxResult {
            started: true,
            signal: Some(2),
            timings: vec![LaunchTiming { phase: "plan".to_owned(), us: 7 }],
            ..SandboxResult::default()
        }
    );
    assert_eq!(stopped.status(), 130);
}

#[test]
fn a_moved_file_that_cannot_be_checked_counts_as_moved() {
    let call = TestCall::new();
    let dir = CallDir::open(call.call_dir()).unwrap();
    let moved_file = call.call_dir().join(MOVED_FILE);
    assert!(!moved(&dir), "no file: the call keeps its shell state");
    private_file(&moved_file, b"");
    assert!(moved(&dir), "efrd's empty file");
    // NOTE: the safe side is to lose one call's shell state, never to write an old
    // state over the state of later calls.
    private_file(&moved_file, b"x");
    assert!(moved(&dir), "a file with a byte in it");
    std::fs::set_permissions(&moved_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(moved(&dir), "a file that others may read");
    std::fs::remove_file(&moved_file).unwrap();
    std::os::unix::fs::symlink(call.project(), &moved_file).unwrap();
    assert!(moved(&dir), "a link");
}
