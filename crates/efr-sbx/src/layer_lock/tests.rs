use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{Locked, lock, lock_dirs};

#[test]
fn the_lock_dirs_are_the_parents_of_the_layer_dirs_once_each_in_order() {
    let layers = ["/s/b/cache/x", "/s/a/cache/y", "/s/b/cache/z"].map(PathBuf::from);
    assert_eq!(lock_dirs(&layers), ["/s/a/cache", "/s/b/cache"].map(PathBuf::from));
}

#[test]
fn a_second_lock_waits_and_then_says_which_dir_is_busy() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cache");
    std::fs::create_dir(&dir).unwrap();
    let dirs = [dir.clone()];
    let Locked::Held(first) = lock(&dirs, Duration::ZERO).unwrap() else {
        panic!("a free dir did not lock");
    };
    let start = Instant::now();
    let Locked::Busy { dir: busy } = lock(&dirs, Duration::from_millis(50)).unwrap() else {
        panic!("two launches held the lock at once");
    };
    assert_eq!(busy, dir);
    assert!(start.elapsed() >= Duration::from_millis(50));
    drop(first);
    assert!(matches!(lock(&dirs, Duration::ZERO).unwrap(), Locked::Held(_)));
}
