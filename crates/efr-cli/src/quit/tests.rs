use std::sync::atomic::{AtomicUsize, Ordering};

use futures::FutureExt as _;
use rustix::process::{Signal, getpid, kill_process};

use super::{CtrlBackslash, Quit as _};

/// The SIGQUITs that came while nothing waited, in place of the default action, which
/// would end the test process.
static UNARMED: AtomicUsize = AtomicUsize::new(0);

fn count_unarmed() {
    UNARMED.fetch_add(1, Ordering::SeqCst);
}

/// `Ctrl+\` at the terminal: SIGQUIT to this process.
fn press() {
    kill_process(getpid(), Signal::QUIT).unwrap();
}

/// Lets the listener task run, without real time.
async fn settle() {
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

// NOTE: one test, because a signal reaches every listener of the process, so two tests
// that press under `cargo test`, which shares a process, would see each other's keys.
#[tokio::test]
async fn ctrl_backslash_counts_only_while_a_wait_lives_and_otherwise_keeps_its_default() {
    let quit = CtrlBackslash::with_unarmed(count_unarmed);

    // A wait resolves at the key, and nothing takes the default action.
    let mut wait = quit.wait();
    assert!((&mut wait).now_or_never().is_none(), "the first poll installs the handler");
    press();
    wait.await;
    settle().await;
    assert_eq!(UNARMED.load(Ordering::SeqCst), 0);

    // With no wait alive, the key takes its default action.
    press();
    while UNARMED.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    // A key from before a wait does not resolve it; the next one does.
    let mut wait = quit.wait();
    assert!((&mut wait).now_or_never().is_none());
    settle().await;
    assert!((&mut wait).now_or_never().is_none(), "an earlier key counts for nothing");
    press();
    wait.await;
    settle().await;
    assert_eq!(UNARMED.load(Ordering::SeqCst), 1);

    // A wait dropped before the key leaves the key to its default action.
    let mut dropped = quit.wait();
    assert!((&mut dropped).now_or_never().is_none());
    drop(dropped);
    press();
    while UNARMED.load(Ordering::SeqCst) == 1 {
        tokio::task::yield_now().await;
    }
}
