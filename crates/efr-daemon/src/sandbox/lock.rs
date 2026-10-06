//! The plan lock (efr's auto spec, section 5.9): one lock per project root, held while
//! efrd builds a call's plan and until the launcher has opened every bind source and
//! recorded the git surface (`$CALL/started`), or until the call returns when that
//! comes first. `write_file` takes the same lock only for its own write.
//!
//! So a plan never races another plan or an efrd write in the same project, and a
//! contained call that runs past its timeout (a dev server, `cargo watch`) blocks
//! nobody.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_stdx::time::Clock;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// How often the wait for `$CALL/started` looks again.
const STARTED_POLL: Duration = Duration::from_millis(5);

/// The locks, one per project root, made on first use.
#[derive(Debug, Default)]
pub(crate) struct PlanLocks {
    roots: Mutex<HashMap<PathBuf, Arc<Semaphore>>>,
}

/// The locks of one plan or one write, released when dropped.
#[derive(Debug)]
pub(crate) struct PlanGuard {
    _held: Vec<OwnedSemaphorePermit>,
}

impl PlanLocks {
    /// Takes the lock of each root in `roots`, in the order of their paths, so two
    /// plans that share roots never wait for each other in a circle.
    pub(crate) async fn lock(&self, roots: &[PathBuf]) -> PlanGuard {
        let mut sorted: Vec<&PathBuf> = roots.iter().collect();
        sorted.sort();
        sorted.dedup();
        let mut held = Vec::with_capacity(sorted.len());
        for root in sorted {
            let semaphore = self.semaphore(root);
            // NOTE: the semaphores are never closed, so the acquire cannot fail; a
            // failure would only cost this one lock.
            if let Ok(permit) = semaphore.acquire_owned().await {
                held.push(permit);
            }
        }
        PlanGuard { _held: held }
    }

    fn semaphore(&self, root: &Path) -> Arc<Semaphore> {
        // The table only grows, so a poisoned lock still holds a usable one.
        let mut roots = self.roots.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(roots.entry(root.to_path_buf()).or_insert_with(|| Arc::new(Semaphore::new(1))))
    }
}

/// Runs `call` while it holds `guard`, and lets the guard go once `started` exists,
/// so a call that runs on after its launcher started blocks no plan or write.
pub(crate) async fn run_holding<F: Future>(
    guard: PlanGuard,
    started: &Path,
    clock: &dyn Clock,
    call: F,
) -> F::Output {
    let mut guard = Some(guard);
    tokio::pin!(call);
    loop {
        tokio::select! {
            output = &mut call => return output,
            () = appears(clock, started), if guard.is_some() => guard = None,
        }
    }
}

/// Waits until `path` exists, looking every few milliseconds on `clock`.
pub(crate) async fn appears(clock: &dyn Clock, path: &Path) {
    loop {
        if tokio::fs::symlink_metadata(path).await.is_ok() {
            return;
        }
        clock.sleep(STARTED_POLL).await;
    }
}

#[cfg(test)]
mod tests;
