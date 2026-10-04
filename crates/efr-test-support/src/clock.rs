//! A clock that moves only when a test moves it.
//!
//! Code under test takes an `Arc<dyn Clock>`; the test keeps a [`TestClock`] handle,
//! waits until the code sleeps, and moves time forward. Nothing waits on real time, so
//! a token refresh five minutes ahead of expiry or a backoff of thirty seconds takes no
//! time at all.

use std::collections::BTreeMap;
use std::fmt;
use std::future::{Future, ready};
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use tokio::sync::Notify;

/// A manual [`Clock`]: `now` changes only through [`advance`](TestClock::advance),
/// [`set`](TestClock::set) and [`advance_to_next_deadline`](TestClock::advance_to_next_deadline),
/// and a sleep ends when the clock reaches its deadline.
///
/// Clones share one time, so a test hands [`shared`](TestClock::shared) to the code
/// under test and keeps the clone to move it.
#[derive(Clone)]
pub struct TestClock {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    /// Notified each time a sleep starts to wait, for [`TestClock::wait_for_sleeps`].
    started: Notify,
}

struct State {
    now: Timestamp,
    next_id: u64,
    /// The sleeps that have not reached their deadline, by id.
    waiting: BTreeMap<u64, Waiting>,
    /// Every duration passed to `sleep`, in order.
    requested: Vec<Duration>,
}

struct Waiting {
    /// `None` when the deadline is later than the last `Timestamp`: that sleep never
    /// ends, as with the system clock.
    deadline: Option<Timestamp>,
    waker: Option<Waker>,
}

impl TestClock {
    /// The instant every test clock starts at unless told otherwise:
    /// 2026-10-04T12:00:00Z. Fixtures that contain times are written against it.
    pub const START: Timestamp = Timestamp::constant(1_791_115_200, 0);

    /// A clock at [`TestClock::START`] with no sleeps.
    pub fn new() -> Self {
        TestClock::starting_at(TestClock::START)
    }

    /// A clock at `now` with no sleeps.
    pub fn starting_at(now: Timestamp) -> Self {
        let state = State { now, next_id: 0, waiting: BTreeMap::new(), requested: Vec::new() };
        TestClock { shared: Arc::new(Shared { state: Mutex::new(state), started: Notify::new() }) }
    }

    /// This clock as the `Arc<dyn Clock>` that code under test takes. It shares the
    /// time of `self`.
    pub fn shared(&self) -> Arc<dyn Clock> {
        Arc::new(self.clone())
    }

    /// Moves the clock forward by `by` and ends every sleep whose deadline it reaches.
    /// The clock stops at the last `Timestamp` instead of overflowing.
    pub fn advance(&self, by: Duration) {
        let state = self.shared.lock();
        let now = state.now.checked_add(by).unwrap_or(Timestamp::MAX);
        move_to(state, now);
    }

    /// Sets the clock to `to`, forward or back, and ends every sleep whose deadline is
    /// not after it. A sleep keeps the deadline it got when it started, so a step back
    /// makes it last longer, as a wall-clock timer does.
    pub fn set(&self, to: Timestamp) {
        move_to(self.shared.lock(), to);
    }

    /// Moves the clock to the earliest deadline of the waiting sleeps, ends the sleeps
    /// due then, and returns how far it moved. Returns `None` and leaves the clock alone
    /// when no sleep can end. A test that does not know an exact delay, such as a
    /// backoff with jitter, uses this to let the code under test go on.
    pub fn advance_to_next_deadline(&self) -> Option<Duration> {
        let state = self.shared.lock();
        let deadline = next_deadline(&state)?;
        let moved = Duration::try_from(deadline.duration_since(state.now)).unwrap_or_default();
        move_to(state, deadline);
        Some(moved)
    }

    /// The earliest deadline of the waiting sleeps, if any can end.
    pub fn next_deadline(&self) -> Option<Timestamp> {
        next_deadline(&self.shared.lock())
    }

    /// The number of sleeps that have started and not reached their deadline. A sleep
    /// that is dropped stops counting.
    pub fn pending_sleeps(&self) -> usize {
        self.shared.lock().waiting.len()
    }

    /// Every duration that code has passed to [`Clock::sleep`] on this clock, in order,
    /// including sleeps of zero and sleeps that were dropped.
    pub fn requested_sleeps(&self) -> Vec<Duration> {
        self.shared.lock().requested.clone()
    }

    /// Waits until at least `count` sleeps are pending, without real time. A test calls
    /// it before [`advance`](TestClock::advance), so that the task under test is
    /// already asleep when the clock moves.
    pub async fn wait_for_sleeps(&self, count: usize) {
        loop {
            // Registered before the check, so a sleep that starts between the check and
            // the await still wakes this task.
            let mut started = pin!(self.shared.started.notified());
            started.as_mut().enable();
            if self.pending_sleeps() >= count {
                return;
            }
            started.await;
        }
    }
}

impl Default for TestClock {
    fn default() -> Self {
        TestClock::new()
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        self.shared.lock().now
    }

    fn sleep(&self, duration: Duration) -> Sleep {
        let mut state = self.shared.lock();
        state.requested.push(duration);
        let deadline = state.now.checked_add(duration).ok();
        if deadline.is_some_and(|deadline| deadline <= state.now) {
            return Box::pin(ready(()));
        }
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state.waiting.insert(id, Waiting { deadline, waker: None });
        drop(state);
        self.shared.started.notify_waiters();
        Box::pin(TestSleep { shared: Arc::clone(&self.shared), id })
    }
}

impl fmt::Debug for TestClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.shared.lock();
        f.debug_struct("TestClock")
            .field("now", &state.now)
            .field("pending_sleeps", &state.waiting.len())
            .finish()
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Nothing panics while it holds the lock, and a test that panicked elsewhere
        // leaves a consistent state, so a poisoned lock is still usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn next_deadline(state: &State) -> Option<Timestamp> {
    state.waiting.values().filter_map(|waiting| waiting.deadline).min()
}

/// Sets `now` and wakes the sleeps it makes due, earliest deadline first, after the
/// lock is released so that a woken task never finds it held.
fn move_to(mut state: MutexGuard<'_, State>, now: Timestamp) {
    state.now = now;
    let mut due: Vec<(Timestamp, u64)> = state
        .waiting
        .iter()
        .filter_map(|(&id, waiting)| waiting.deadline.filter(|&at| at <= now).map(|at| (at, id)))
        .collect();
    due.sort_unstable();
    let wakers: Vec<Waker> = due
        .iter()
        .filter_map(|(_, id)| state.waiting.remove(id))
        .filter_map(|waiting| waiting.waker)
        .collect();
    drop(state);
    for waker in wakers {
        waker.wake();
    }
}

/// The future of one sleep. It is done once the clock has removed it from the waiting
/// sleeps; dropping it early removes it too.
struct TestSleep {
    shared: Arc<Shared>,
    id: u64,
}

impl Future for TestSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.shared.lock();
        let now = state.now;
        let Some(waiting) = state.waiting.get_mut(&self.id) else {
            return Poll::Ready(());
        };
        if waiting.deadline.is_some_and(|deadline| deadline <= now) {
            state.waiting.remove(&self.id);
            return Poll::Ready(());
        }
        if !waiting.waker.as_ref().is_some_and(|waker| waker.will_wake(cx.waker())) {
            waiting.waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for TestSleep {
    fn drop(&mut self) {
        self.shared.lock().waiting.remove(&self.id);
    }
}

#[cfg(test)]
mod tests;
