//! The stop of one search (the plan's DQ20).
//!
//! efrd makes a [`SearchStop`] for each search and keeps a clone. [`SearchStop::soft`]
//! ends the row loop of the search at the next row: the search ranks what it has seen
//! and says that it stopped early. [`SearchStop::hard`] does that too, and interrupts
//! the statement that runs on the search's connection, so a long step of SQLite ends
//! between two of its opcodes. One step inside FTS5 (the merge of a prefix, the count
//! of `bm25()`) reads no interrupt, so it runs to its end first.
//!
//! The lane puts its connection's interrupt handle into the stop when the search
//! starts and takes it out under the same lock before the connection serves another
//! search, so a stop that comes late never interrupts the next search on that lane.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::InterruptHandle;

/// A hook that a test runs after every `n` rows of the row loop.
type RowHook = Arc<dyn Fn(u32) + Send + Sync>;

/// The stop of one search: two flags and the interrupt of the connection it runs on.
/// Clones share the flags.
#[derive(Clone, Default)]
pub struct SearchStop {
    shared: Arc<Shared>,
    hook: Option<(u32, RowHook)>,
}

#[derive(Default)]
struct Shared {
    soft: AtomicBool,
    hard: AtomicBool,
    interrupt: Mutex<Option<InterruptHandle>>,
}

impl SearchStop {
    /// A stop that has not stopped.
    pub fn new() -> Self {
        SearchStop::default()
    }

    /// Ends the row loop at the next row; the search ranks what it has seen.
    pub fn soft(&self) {
        self.shared.soft.store(true, Ordering::Release);
    }

    /// Ends the row loop and interrupts the statement that runs, if any. A search that
    /// has not started yet does not start.
    pub fn hard(&self) {
        self.soft();
        let interrupt = self.shared.interrupt.lock().unwrap_or_else(PoisonError::into_inner);
        self.shared.hard.store(true, Ordering::Release);
        if let Some(handle) = interrupt.as_ref() {
            handle.interrupt();
        }
    }

    /// True after [`SearchStop::soft`] or [`SearchStop::hard`].
    pub fn is_stopped(&self) -> bool {
        self.shared.soft.load(Ordering::Acquire)
    }

    /// True after [`SearchStop::hard`].
    pub fn is_hard(&self) -> bool {
        self.shared.hard.load(Ordering::Acquire)
    }

    /// A test seam: the row loop runs `hook` with the count of rows after every `n`
    /// rows, so a test can hold a search between two rows while it moves a clock.
    pub fn on_rows(mut self, n: u32, hook: Arc<dyn Fn(u32) + Send + Sync>) -> Self {
        self.hook = Some((n.max(1), hook));
        self
    }

    /// Called by the row loop after each row, with the rows read so far.
    pub(crate) fn row(&self, rows: u32) {
        if let Some((every, hook)) = &self.hook
            && rows.is_multiple_of(*every)
        {
            hook(rows);
        }
    }

    /// Puts the interrupt handle of the connection that starts this search into the
    /// stop. False when the stop is already hard: the search must not start.
    pub(crate) fn arm(&self, handle: InterruptHandle) -> bool {
        let mut interrupt = self.shared.interrupt.lock().unwrap_or_else(PoisonError::into_inner);
        if self.is_hard() {
            return false;
        }
        *interrupt = Some(handle);
        true
    }

    /// Takes the interrupt handle out again, before the connection serves another
    /// search.
    pub(crate) fn disarm(&self) {
        self.shared.interrupt.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

impl fmt::Debug for SearchStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchStop")
            .field("soft", &self.is_stopped())
            .field("hard", &self.is_hard())
            .field("hook", &self.hook.as_ref().map(|(every, _)| every))
            .finish()
    }
}

#[cfg(test)]
mod tests;
