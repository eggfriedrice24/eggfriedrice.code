use jiff::{SignedDuration, Timestamp};

use super::DaemonWakes;

fn at(minutes: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000).unwrap() + SignedDuration::from_mins(minutes)
}

/// The limit counts the wake turns of every conversation in the last hour; a start
/// that leaves the hour frees its place.
#[test]
fn wake_limit_is_daemon_wide() {
    let wakes = DaemonWakes::default();
    assert!(wakes.take(at(0), 2));
    assert!(wakes.take(at(10), 2));
    assert!(!wakes.take(at(20), 2), "two wake turns in the hour already");
    assert!(wakes.take(at(61), 2), "the first one left the hour");
    assert!(!DaemonWakes::default().take(at(0), 0), "0 never wakes");
}

/// The window starts with the wake turns that the store recorded, so a restart does not
/// reset it.
#[test]
fn wake_window_survives_restart() {
    let wakes = DaemonWakes::seeded([at(-30), at(-50), at(-90)]);
    assert!(!wakes.take(at(0), 2), "two of the stored starts are in the last hour");
    assert!(wakes.take(at(11), 2), "the start of 50 minutes ago left the hour");
}
