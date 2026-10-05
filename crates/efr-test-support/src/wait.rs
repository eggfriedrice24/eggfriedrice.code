//! Waits for a condition that another task or thread makes true, with a real time
//! limit.
//!
//! A test that waits for work it does not drive (a file the daemon writes from its own
//! task, a terminal mode a key thread sets, a screen fed by a real zsh) cannot know how
//! many polls the work needs. A loop that counts yields passes on a fast machine and
//! fails on a busy CI runner; a loop without a bound hangs until the test runner kills
//! it. [`Wait`] polls until the condition holds and gives up after a real time limit,
//! with an error that names what it waited for.
//!
//! The limit only decides when a test that would fail anyway stops waiting. It never
//! decides the result of a test that passes, so it does not break the rule that time is
//! injected: code under test still takes a `Clock`.

use std::time::{Duration, Instant};

use efr_stdx::time::{Clock as _, SystemClock};

use crate::TestSupportError;

/// How long a [`Wait`] polls before it gives up, unless the test sets another limit.
///
/// Long enough for a loaded CI runner to start a process or write a file, and short
/// enough that a broken test fails well before the test runner's own timeout.
pub const WAIT_LIMIT: Duration = Duration::from_secs(10);

/// How many polls a wait makes with a plain yield before it starts to sleep between
/// polls. Work on the same runtime is usually done within a few yields, and a yield
/// keeps such a wait as fast as the loops it replaces.
const YIELDS: u32 = 100;

/// The real time between two polls after the first [`YIELDS`] polls. It lets threads
/// and processes outside the runtime make progress without a busy loop.
const PAUSE: Duration = Duration::from_millis(2);

/// A wait for a condition, with the description its error carries.
///
/// ```ignore
/// Wait::new("the notice file").until(|| path.exists()).await.unwrap();
/// let text = Wait::new("a notice").until_some(|| read(&path)).await.unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
#[must_use = "a wait does nothing until one of its until methods runs"]
pub struct Wait<'a> {
    what: &'a str,
    limit: Duration,
}

impl<'a> Wait<'a> {
    /// A wait for `what`, a phrase that completes "gave up waiting for ...", with the
    /// limit [`WAIT_LIMIT`].
    pub fn new(what: &'a str) -> Self {
        Wait { what, limit: WAIT_LIMIT }
    }

    /// The same wait with another real time limit.
    pub fn limit(self, limit: Duration) -> Self {
        Wait { limit, ..self }
    }

    /// Polls `condition` until it is true.
    ///
    /// # Errors
    ///
    /// [`TestSupportError::TimedOut`] when the condition is still false at the limit.
    pub async fn until(self, mut condition: impl FnMut() -> bool) -> Result<(), TestSupportError> {
        self.until_some(|| condition().then_some(())).await
    }

    /// Polls `poll` until it gives a value, and returns that value.
    ///
    /// # Errors
    ///
    /// [`TestSupportError::TimedOut`] when `poll` still gives `None` at the limit.
    pub async fn until_some<T>(
        self,
        mut poll: impl FnMut() -> Option<T>,
    ) -> Result<T, TestSupportError> {
        self.until_some_async(async || poll()).await
    }

    /// Polls `poll`, whose check must await (a request to an actor, for example), until
    /// it gives a value, and returns that value.
    ///
    /// # Errors
    ///
    /// [`TestSupportError::TimedOut`] when `poll` still gives `None` at the limit.
    pub async fn until_some_async<T>(
        self,
        mut poll: impl AsyncFnMut() -> Option<T>,
    ) -> Result<T, TestSupportError> {
        let start = Instant::now();
        let mut polls: u32 = 0;
        loop {
            // NOTE: a poll that never resolves, such as a request to a stuck actor, is cut
            // at the limit too, so the wait fails with its description.
            let left = self.limit.saturating_sub(start.elapsed());
            match SystemClock.timeout(left, poll()).await {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => {}
                Err(_) => return Err(self.timed_out()),
            }
            if start.elapsed() >= self.limit {
                return Err(self.timed_out());
            }
            if polls < YIELDS {
                polls += 1;
                tokio::task::yield_now().await;
            } else {
                SystemClock.sleep(PAUSE).await;
            }
        }
    }

    /// Polls `condition` on the calling thread, sleeping between polls, until it is
    /// true. For a check outside a runtime, or for a thread the runtime does not run.
    ///
    /// # Errors
    ///
    /// [`TestSupportError::TimedOut`] when the condition is still false at the limit.
    pub fn until_blocking(
        self,
        mut condition: impl FnMut() -> bool,
    ) -> Result<(), TestSupportError> {
        let start = Instant::now();
        loop {
            if condition() {
                return Ok(());
            }
            if start.elapsed() >= self.limit {
                return Err(self.timed_out());
            }
            std::thread::sleep(PAUSE);
        }
    }

    fn timed_out(&self) -> TestSupportError {
        TestSupportError::TimedOut { what: self.what.to_owned(), limit: self.limit }
    }
}

#[cfg(test)]
mod tests;
