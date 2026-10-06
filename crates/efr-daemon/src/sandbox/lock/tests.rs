//! The plan lock: one plan at a time per project, released when the launcher started.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use efr_test_support::TestClock;
use futures::FutureExt as _;

use super::{PlanLocks, run_holding};

fn roots(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|name| PathBuf::from(format!("/home/u/p/{name}"))).collect()
}

#[tokio::test]
async fn plan_lock_serializes_plans() {
    let locks = PlanLocks::default();
    let first = locks.lock(&roots(&["app", "lib"])).await;
    // A plan that shares one project waits; one of another project does not.
    assert!(locks.lock(&roots(&["lib"])).now_or_never().is_none());
    assert!(locks.lock(&roots(&["other"])).now_or_never().is_some());
    drop(first);
    let second = locks.lock(&roots(&["lib", "app"])).now_or_never();
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
    assert!(locks.lock(&roots(&["app"])).now_or_never().is_none());
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
