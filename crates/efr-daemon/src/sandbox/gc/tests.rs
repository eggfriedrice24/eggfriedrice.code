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
