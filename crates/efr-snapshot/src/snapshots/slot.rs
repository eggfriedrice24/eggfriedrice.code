//! [`Slot`]: one store in this process, and [`Pin`]: a tree that a running turn or
//! call still needs.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::capture::{Captured, LeftOut, LeftOutReason};

/// How long a root that a snapshot skipped (too many files) stays skipped before the
/// next snapshot counts its files again. Counting walks the whole root, the cost that
/// the skip saves.
const SKIP_FOR: Duration = Duration::from_secs(10 * 60);

/// One store in this process: the gate that lets one snapshot, commit or collection of
/// the store run at a time, the tree of its index at its last snapshot, the trees
/// that running turns and calls still need ([`Pin`]), until when the root stays
/// skipped, and what its snapshots left out.
#[derive(Debug)]
pub(crate) struct Slot {
    gate: tokio::sync::Semaphore,
    tree: Mutex<Option<String>>,
    live: Mutex<Vec<String>>,
    skipped_until: Mutex<Option<jiff::Timestamp>>,
    /// True once this process checked the store's format (`capture::prepare`).
    pub(crate) ready: AtomicBool,
    record: Mutex<SlotRecord>,
    /// The directories at the root's top that its snapshots leave out.
    skip: Mutex<Vec<String>>,
}

/// What the snapshots of a store left out, and the modes they saw.
#[derive(Debug, Default)]
struct SlotRecord {
    /// The ignored files that the last snapshot with the ignored files left out.
    ignored: Vec<LeftOut>,
    /// The rest, from the last snapshot.
    latest: Vec<LeftOut>,
    /// Modes other than 0644 and 0755, by path.
    modes: BTreeMap<String, u32>,
}

/// True for a reason that only a look at the ignored files finds.
fn of_ignored(reason: LeftOutReason) -> bool {
    matches!(
        reason,
        LeftOutReason::IgnoredTooLarge
            | LeftOutReason::IgnoredCap
            | LeftOutReason::SkippedDir
            | LeftOutReason::IgnoredOff
    )
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            gate: tokio::sync::Semaphore::new(1),
            tree: Mutex::new(None),
            live: Mutex::new(Vec::new()),
            skipped_until: Mutex::new(None),
            ready: AtomicBool::new(false),
            record: Mutex::new(SlotRecord::default()),
            skip: Mutex::new(Vec::new()),
        }
    }
}

/// A tree of a store that a running turn (its first tree) or a running call (its tree
/// before) still needs. No ref names such a tree, and once the index moves on, the
/// index no longer reaches it either, so the collector's prune could remove its
/// objects; the collector pins every live tree under a ref first. Dropping the pin
/// ends the need.
#[derive(Debug)]
pub(crate) struct Pin {
    slot: Arc<Slot>,
    tree: String,
}

impl Pin {
    pub(crate) fn new(slot: Arc<Slot>, tree: &str) -> Pin {
        slot.lock_live().push(tree.to_owned());
        Pin { slot, tree: tree.to_owned() }
    }
}

impl Clone for Pin {
    fn clone(&self) -> Pin {
        Pin::new(Arc::clone(&self.slot), &self.tree)
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        let mut live = self.slot.lock_live();
        if let Some(at) = live.iter().position(|tree| tree == &self.tree) {
            live.swap_remove(at);
        }
    }
}

impl Slot {
    /// Waits for the store's turn. The gate is never closed, so the wait cannot fail;
    /// `None` would only mean that the store runs unguarded once.
    pub(crate) async fn enter(&self) -> Option<tokio::sync::SemaphorePermit<'_>> {
        self.gate.acquire().await.ok()
    }

    pub(crate) fn tree(&self) -> Option<String> {
        self.tree.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn set_tree(&self, tree: Option<String>) {
        *self.tree.lock().unwrap_or_else(PoisonError::into_inner) = tree;
    }

    /// The directories at the root's top that its snapshots leave out.
    pub(crate) fn skip_list(&self) -> Vec<String> {
        self.skip.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Adds `names` to the directories that the snapshots of the root leave out. A
    /// new name forgets the tree of the last snapshot, so the next one starts fresh and
    /// drops what an older snapshot took below it.
    pub(crate) fn add_skip(&self, names: &[String]) {
        let mut skip = self.skip.lock().unwrap_or_else(PoisonError::into_inner);
        let mut added = false;
        for name in names {
            if !skip.contains(name) {
                skip.push(name.clone());
                added = true;
            }
        }
        drop(skip);
        if added {
            self.set_tree(None);
        }
    }

    /// Forgets the store, for a store that the collector deleted.
    pub(crate) fn forget_store(&self) {
        self.set_tree(None);
        self.ready.store(false, Ordering::Release);
        *self.lock_record() = SlotRecord::default();
    }

    /// The trees that running turns and calls still need, each once, sorted.
    pub(crate) fn live_trees(&self) -> Vec<String> {
        let mut trees = self.lock_live().clone();
        trees.sort();
        trees.dedup();
        trees
    }

    /// True while the root stays skipped at `now`.
    pub(crate) fn skipped(&self, now: jiff::Timestamp) -> bool {
        self.lock_skipped().is_some_and(|until| now < until)
    }

    /// Skips the root from `now` for [`SKIP_FOR`].
    pub(crate) fn skip(&self, now: jiff::Timestamp) {
        *self.lock_skipped() = now.checked_add(SKIP_FOR).ok();
    }

    /// Takes in what one snapshot left out and the modes it saw.
    pub(crate) fn note(&self, captured: &Captured) {
        let mut record = self.lock_record();
        let left_out = &captured.record.left_out;
        if captured.scanned_ignored {
            record.ignored =
                left_out.iter().filter(|out| of_ignored(out.reason)).cloned().collect();
            record.latest =
                left_out.iter().filter(|out| !of_ignored(out.reason)).cloned().collect();
        } else {
            record.latest.clone_from(left_out);
        }
        for (path, mode) in &captured.record.modes {
            match mode {
                Some(mode) => record.modes.insert(path.clone(), *mode),
                None => record.modes.remove(path),
            };
        }
    }

    /// What the snapshots of the store left out now: the ignored files of the last
    /// look at them and the rest of the last snapshot, sorted, each once.
    pub(crate) fn left_out(&self) -> Vec<LeftOut> {
        let record = self.lock_record();
        let mut all: Vec<LeftOut> = record.ignored.iter().chain(&record.latest).cloned().collect();
        all.sort();
        all.dedup();
        all
    }

    /// The modes other than 0644 and 0755 that the snapshots saw, by path.
    pub(crate) fn modes(&self) -> BTreeMap<String, u32> {
        self.lock_record().modes.clone()
    }

    fn lock_skipped(&self) -> MutexGuard<'_, Option<jiff::Timestamp>> {
        self.skipped_until.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_live(&self) -> MutexGuard<'_, Vec<String>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_record(&self) -> MutexGuard<'_, SlotRecord> {
        self.record.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
