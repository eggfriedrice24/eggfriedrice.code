//! Status conversion and signal mapping as tables, and the reaper over real `sh`
//! children that run without a PTY.

use std::os::unix::process::ExitStatusExt as _;
use std::process::{ExitStatus, Stdio};

use efr_holder::{ChildStatus, Signal};
use pretty_assertions::assert_eq;
use rstest::rstest;
use tokio::runtime::Handle;

use super::{Child, UNKNOWN_EXIT_CODE, signal_number, status_of};

/// `sh -c script` in `/` with an empty environment. Its standard input is a pipe the
/// test keeps, so a script that reads blocks until the test drops it.
fn sh(script: &str) -> (tokio::process::Child, Option<tokio::process::ChildStdin>) {
    let mut command = efr_stdx::process::command("/bin/sh", "/");
    command
        .args(["-c", script])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().unwrap();
    let stdin = child.stdin.take();
    (child, stdin)
}

async fn reaped(child: &Child) -> ChildStatus {
    *child.subscribe().wait_for(|status| !status.is_running()).await.unwrap()
}

#[rstest]
#[case::exit_zero(ExitStatus::from_raw(0), ChildStatus::Exited { code: 0 })]
#[case::exit_code(ExitStatus::from_raw(3 << 8), ChildStatus::Exited { code: 3 })]
#[case::exit_255(ExitStatus::from_raw(255 << 8), ChildStatus::Exited { code: 255 })]
#[case::killed(ExitStatus::from_raw(libc::SIGKILL), ChildStatus::Signaled { signal: libc::SIGKILL })]
#[case::core_dump(
    ExitStatus::from_raw(libc::SIGSEGV | 0x80),
    ChildStatus::Signaled { signal: libc::SIGSEGV }
)]
#[case::stopped(
    ExitStatus::from_raw((libc::SIGSTOP << 8) | 0x7f),
    ChildStatus::Exited { code: UNKNOWN_EXIT_CODE }
)]
fn status_of_never_reports_running(#[case] exit: ExitStatus, #[case] expected: ChildStatus) {
    assert_eq!(status_of(exit), expected);
}

#[rstest]
#[case(Signal::Hangup, libc::SIGHUP)]
#[case(Signal::Interrupt, libc::SIGINT)]
#[case(Signal::Quit, libc::SIGQUIT)]
#[case(Signal::Terminate, libc::SIGTERM)]
#[case(Signal::Kill, libc::SIGKILL)]
fn every_contract_signal_has_its_platform_number(#[case] signal: Signal, #[case] number: i32) {
    assert_eq!(signal_number(signal).map(rustix::process::Signal::as_raw), Some(number));
}

#[tokio::test]
async fn the_reaper_records_the_exit_code() {
    let (child, _stdin) = sh("exit 4");
    let child = Child::adopt(child, &Handle::current()).unwrap();
    assert_eq!(reaped(&child).await, ChildStatus::Exited { code: 4 });
    assert_eq!(child.status(), ChildStatus::Exited { code: 4 });
}

#[tokio::test]
async fn a_signal_reaches_a_running_child_and_its_end_is_recorded() {
    let (child, _stdin) = sh("read line");
    let child = Child::adopt(child, &Handle::current()).unwrap();
    assert_eq!(child.status(), ChildStatus::Running);

    child.signal(rustix::process::Signal::TERM).unwrap();
    assert_eq!(reaped(&child).await, ChildStatus::Signaled { signal: libc::SIGTERM });
}

#[tokio::test]
async fn a_signal_to_a_reaped_child_fails_with_esrch() {
    let (child, _stdin) = sh("exit 0");
    let child = Child::adopt(child, &Handle::current()).unwrap();
    reaped(&child).await;
    assert_eq!(child.signal(rustix::process::Signal::TERM), Err(rustix::io::Errno::SRCH));
}

#[tokio::test]
async fn the_id_is_the_pid_tokio_reported() {
    let (child, stdin) = sh("read line");
    let pid = child.id().unwrap();
    let child = Child::adopt(child, &Handle::current()).unwrap();
    assert_eq!(child.id(), pid);
    drop(stdin);
    assert_eq!(reaped(&child).await, ChildStatus::Exited { code: 1 });
}

#[tokio::test]
async fn dropping_the_child_ends_a_pending_subscription() {
    let (child, _stdin) = sh("read line");
    let child = Child::adopt(child, &Handle::current()).unwrap();
    let mut status = child.subscribe();
    // The PTY's release drops the only strong sender; the reaper holds a weak one.
    drop(child);
    assert!(status.changed().await.is_err());
}
