use std::num::NonZeroUsize;

use super::{MAX_WORKER_THREADS, build_runtime, worker_threads};

fn cores(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).unwrap()
}

#[test]
fn a_big_machine_gets_four_workers() {
    assert_eq!(worker_threads(cores(32)), 4);
    assert_eq!(worker_threads(cores(5)), 4);
}

#[test]
fn a_small_machine_gets_one_worker_per_core() {
    assert_eq!(worker_threads(cores(1)), 1);
    assert_eq!(worker_threads(cores(2)), 2);
    assert_eq!(worker_threads(cores(4)), 4);
}

#[test]
fn the_runtime_starts_at_most_four_workers() {
    let runtime = build_runtime().unwrap();

    let workers = runtime.metrics().num_workers();

    let available = std::thread::available_parallelism().unwrap().get();
    assert_eq!(workers, available.min(MAX_WORKER_THREADS));
    assert!(workers <= 4, "{workers} workers");
}
