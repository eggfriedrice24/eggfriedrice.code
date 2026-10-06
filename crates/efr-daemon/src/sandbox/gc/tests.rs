//! The rule of the cache layers' collector.

use std::path::PathBuf;
use std::time::Duration;

use super::{Layers, pick};

const DAY: Duration = Duration::from_secs(24 * 3600);

fn layers(name: &str, days: u64, bytes: u64, busy: bool) -> Layers {
    Layers { dir: PathBuf::from(name), idle: DAY * u32::try_from(days).unwrap(), bytes, busy }
}

#[test]
fn idle_layers_go_then_the_oldest_until_the_rest_fit() {
    let all = [
        layers("old", 20, 10, false),
        layers("old-but-busy", 30, 10, true),
        layers("big-older", 5, 100, false),
        layers("big-newer", 1, 100, false),
        layers("fresh", 0, 10, false),
    ];
    let gone = pick(&all, DAY * 14, 150);
    assert_eq!(gone, [PathBuf::from("old"), PathBuf::from("big-older")]);
    assert!(pick(&all, DAY * 100, 10_000).is_empty());
    // The busy one stays even when it alone is too big.
    let busy = [layers("busy", 1, 500, true)];
    assert!(pick(&busy, DAY, 10).is_empty());
}

/// Sets the modification time of `path` to `ago` before `now`.
fn age(path: &std::path::Path, now: std::time::SystemTime, ago: Duration) {
    let file = std::fs::File::open(path).unwrap();
    file.set_modified(now - ago).unwrap();
}

#[test]
fn a_cache_whose_last_call_is_recent_is_not_idle() {
    let root = tempfile::tempdir().unwrap();
    let now = std::time::SystemTime::now();
    let cache = root.path().join("conv/cache");
    std::fs::create_dir_all(cache.join("cargo/upper")).unwrap();
    // Calls write below cache/<name>/upper, which leaves the time of cache old.
    age(&cache, now, DAY * 30);
    let stamp = root.path().join("conv").join(super::LAST_CALL_FILE);
    std::fs::write(&stamp, "").unwrap();
    age(&stamp, now, DAY);
    let [layers] = super::scan(root.path(), now, &|_| false).try_into().unwrap();
    assert!(layers.idle < DAY * 2, "{layers:?}");
    // Without the stamp, the time of cache counts.
    std::fs::remove_file(&stamp).unwrap();
    let [layers] = super::scan(root.path(), now, &|_| false).try_into().unwrap();
    assert!(layers.idle >= DAY * 30, "{layers:?}");
}

#[test]
fn a_call_left_running_keeps_its_conversation_busy() {
    let shell = tempfile::tempdir().unwrap();
    assert!(!super::launcher_running(shell.path()));
    let call = shell.path().join("01920000-0000-7000-8000-000000000001");
    std::fs::create_dir_all(&call).unwrap();
    assert!(!super::launcher_running(shell.path()), "a call that has not started");
    std::fs::write(call.join(efr_sandbox::STARTED_FILE), "").unwrap();
    assert!(super::launcher_running(shell.path()), "its launcher runs");
    std::fs::write(call.join(efr_sandbox::RESULT_FILE), "{}").unwrap();
    assert!(!super::launcher_running(shell.path()), "it ended");
}

#[test]
fn layers_set_aside_leave_room_for_new_ones_and_are_found_again() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("conv/cache");
    std::fs::create_dir_all(cache.join("cargo/upper")).unwrap();
    let aside = super::set_aside(&cache, std::time::SystemTime::now()).unwrap();
    assert!(!cache.exists());
    assert!(aside.join("cargo/upper").is_dir());
    assert_eq!(super::left_aside(root.path()), [aside.clone()]);
    super::remove(std::slice::from_ref(&aside));
    assert!(super::left_aside(root.path()).is_empty());
}
