//! The daemon's tokio runtime.
//!
//! tokio starts one worker per core by default, which on a 32-core machine is 32
//! threads that a daemon serving one user never keeps busy: its work is waiting on
//! sockets, PTYs and the provider's stream. Every screen has its own thread and the
//! store writes on its own thread, and blocking reads go to the blocking pool, so the
//! workers only shuffle messages between actors.

use std::num::NonZeroUsize;

use tokio::runtime::{Builder, Runtime};

/// The most worker threads the daemon's runtime starts. Four keep a slow handler from
/// stalling the others while costing a few threads on any machine.
pub(crate) const MAX_WORKER_THREADS: usize = 4;

/// How many workers the runtime starts on a machine with `cores` cores: one per core,
/// at most [`MAX_WORKER_THREADS`].
pub(crate) fn worker_threads(cores: NonZeroUsize) -> usize {
    cores.get().min(MAX_WORKER_THREADS)
}

/// The multi-thread runtime that `efrd` runs on, with [`worker_threads`] workers for
/// the cores this process may use.
pub fn build_runtime() -> std::io::Result<Runtime> {
    let cores = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    Builder::new_multi_thread()
        .worker_threads(worker_threads(cores))
        .thread_name("efrd-worker")
        .enable_all()
        .build()
}

#[cfg(test)]
mod tests;
