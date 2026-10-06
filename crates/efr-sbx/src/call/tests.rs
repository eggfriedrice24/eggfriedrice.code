//! How the launcher reports an error of its own.

use pretty_assertions::assert_eq;

use super::{CallDir, STARTED_FILE, failure};
use crate::testing::TestCall;

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
