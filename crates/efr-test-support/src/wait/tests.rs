use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;

use super::{WAIT_LIMIT, Wait};
use crate::TestSupportError;

const SHORT: Duration = Duration::from_millis(50);

/// A flag that a std thread sets after `delay` of real time, as a process or a key
/// thread outside the runtime would.
fn set_later(delay: Duration) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let setter = Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        setter.store(true, Ordering::SeqCst);
    });
    flag
}

#[test]
fn a_wait_has_the_default_limit_until_one_is_set() {
    assert_eq!(Wait::new("x").limit, WAIT_LIMIT);
    assert_eq!(Wait::new("x").limit(SHORT).limit, SHORT);
    assert_eq!(WAIT_LIMIT, Duration::from_secs(10));
}

#[tokio::test]
async fn a_condition_that_holds_returns_at_the_first_poll() {
    let polls = AtomicU32::new(0);
    Wait::new("nothing")
        .until(|| {
            polls.fetch_add(1, Ordering::SeqCst);
            true
        })
        .await
        .unwrap();
    assert_eq!(polls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_condition_that_a_task_on_the_runtime_makes_true_is_seen() {
    let flag = Arc::new(AtomicBool::new(false));
    let setter = Arc::clone(&flag);
    tokio::spawn(async move {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        setter.store(true, Ordering::SeqCst);
    });
    Wait::new("the task").until(|| flag.load(Ordering::SeqCst)).await.unwrap();
}

#[tokio::test]
async fn a_condition_that_takes_real_time_is_seen_however_many_polls_it_needs() {
    // A hundred thousand yields, the count of the loop this replaces, take far less
    // than this on a fast machine; the wait does not count polls.
    let flag = set_later(Duration::from_millis(300));
    let start = Instant::now();
    Wait::new("the thread").until(|| flag.load(Ordering::SeqCst)).await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(300));
}

#[tokio::test]
async fn a_condition_that_never_holds_fails_at_the_limit_with_what_was_awaited() {
    let start = Instant::now();
    let error = Wait::new("the notice in /run/notices/pts-8")
        .limit(SHORT)
        .until(|| false)
        .await
        .unwrap_err();
    assert!(start.elapsed() >= SHORT);
    assert!(start.elapsed() < WAIT_LIMIT, "the limit set wins over the default");
    assert!(
        matches!(&error, TestSupportError::TimedOut { what, limit }
            if what == "the notice in /run/notices/pts-8" && *limit == SHORT),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "gave up waiting for the notice in /run/notices/pts-8 after 50ms"
    );
}

#[tokio::test]
async fn until_some_returns_the_first_value() {
    let mut polls = 0;
    let value = Wait::new("a value")
        .until_some(|| {
            polls += 1;
            (polls == 3).then(|| format!("poll {polls}"))
        })
        .await
        .unwrap();
    assert_eq!(value, "poll 3");
}

#[tokio::test]
async fn until_some_async_awaits_each_poll() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        for n in 0..3 {
            sender.send(n).await.unwrap();
        }
    });
    let mut seen = Vec::new();
    let last = Wait::new("the third message")
        .until_some_async(async || {
            let n = receiver.recv().await?;
            seen.push(n);
            (n == 2).then_some(n)
        })
        .await
        .unwrap();
    assert_eq!(last, 2);
    assert_eq!(seen, [0, 1, 2]);
}

#[tokio::test]
async fn until_some_async_fails_at_the_limit() {
    let error =
        Wait::new("a reply").limit(SHORT).until_some_async(async || None::<()>).await.unwrap_err();
    assert!(matches!(error, TestSupportError::TimedOut { .. }), "{error:?}");
}

#[test]
fn until_blocking_sees_a_condition_another_thread_makes_true() {
    let flag = set_later(Duration::from_millis(20));
    Wait::new("the thread").until_blocking(|| flag.load(Ordering::SeqCst)).unwrap();
}

#[test]
fn until_blocking_fails_at_the_limit() {
    let start = Instant::now();
    let error = Wait::new("the terminal mode").limit(SHORT).until_blocking(|| false).unwrap_err();
    assert!(start.elapsed() >= SHORT);
    assert_eq!(error.to_string(), "gave up waiting for the terminal mode after 50ms");
}

#[tokio::test]
async fn a_poll_that_never_resolves_gives_up_at_the_limit() {
    let (_sender, mut receiver) = tokio::sync::mpsc::channel::<u32>(1);
    let started = Instant::now();
    let error = Wait::new("a message that never comes")
        .limit(SHORT)
        .until_some_async(async || receiver.recv().await)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, TestSupportError::TimedOut { what, limit } if what == "a message that never comes" && *limit == SHORT),
        "{error:?}"
    );
    assert!(started.elapsed() >= SHORT);
}
