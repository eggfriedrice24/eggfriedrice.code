//! The plan lock: one plan at a time per project, released when the launcher started.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_test_support::TestClock;
use futures::FutureExt as _;

use super::{PlanLocks, real_roots, run_holding};

fn roots(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|name| PathBuf::from(format!("/home/u/p/{name}"))).collect()
}

#[tokio::test]
async fn plan_lock_serializes_plans() {
    let locks = PlanLocks::default();
    let first = locks.lock(&roots(&["app", "lib"])).await;
    // A plan that shares one project waits; one of another project does not. NOTE: the
    // real paths are looked up on the blocking pool, so the checks that must not wait
    // take the locks of the paths that `lock` would look up.
    assert!(locks.lock_real(roots(&["lib"])).now_or_never().is_none());
    assert!(locks.lock_real(roots(&["other"])).now_or_never().is_some());
    drop(first);
    let second = locks.lock_real(roots(&["lib", "app"])).now_or_never();
    assert!(second.is_some(), "the lock is free once the first plan ends");
}

#[tokio::test]
async fn write_file_not_blocked_by_still_running_call() {
    let dir = tempfile::tempdir().unwrap();
    let started = dir.path().join("started");
    let clock = TestClock::new();
    let locks = Arc::new(PlanLocks::default());
    let guard = locks.lock(&roots(&["app"])).await;
    let (end, ended) = tokio::sync::oneshot::channel::<()>();
    let call = {
        let (started, clock) = (started.clone(), clock.clone());
        tokio::spawn(async move { run_holding(guard, &started, &clock, ended).await })
    };
    // While the launcher has not started, the plan lock holds a write back.
    clock.wait_for_sleeps(1).await;
    assert!(locks.lock_real(roots(&["app"])).now_or_never().is_none());
    // The launcher opened its bind sources: the call runs on, past its timeout, but
    // the lock is free for a write of the same project.
    std::fs::write(&started, "").unwrap();
    clock.advance(Duration::from_millis(5));
    let write = tokio::time::timeout(Duration::from_secs(5), locks.lock(&roots(&["app"])))
        .await
        .expect("the write took the lock while the call still ran");
    assert!(!call.is_finished());
    drop(write);
    end.send(()).unwrap();
    call.await.unwrap().unwrap();
}

#[tokio::test]
async fn one_root_under_two_spellings_shares_one_lock() {
    // `~/p` may be a link to `/data/p`: a plan that names the root through the link and
    // a write that names it direct must wait for each other.
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let locks = PlanLocks::default();
    let first = locks.lock(std::slice::from_ref(&link)).await;
    let spellings = real_roots(&[real.clone(), link.clone()]).await;
    let canonical = std::fs::canonicalize(&real).unwrap();
    assert_eq!(spellings, [canonical.clone(), canonical.clone()]);
    assert!(locks.lock_real(vec![canonical]).now_or_never().is_none());
    drop(first);
    let both = locks.lock_real(spellings).now_or_never();
    assert!(both.is_some(), "one root under two spellings is one lock, taken once");
    // A root with no real path keeps the path as given.
    let gone = dir.path().join("gone");
    assert_eq!(real_roots(std::slice::from_ref(&gone)).await, [gone]);
}
