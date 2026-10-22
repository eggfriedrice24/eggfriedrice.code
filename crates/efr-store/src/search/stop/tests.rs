use std::sync::atomic::AtomicU32;

use super::*;

#[test]
fn soft_and_hard() {
    let stop = SearchStop::new();
    assert!(!stop.is_stopped() && !stop.is_hard());
    let clone = stop.clone();
    clone.soft();
    assert!(stop.is_stopped() && !stop.is_hard(), "clones share the flags");
    stop.hard();
    assert!(clone.is_stopped() && clone.is_hard());
}

#[test]
fn a_hard_stop_refuses_to_arm() {
    let conn = crate::db::open_in_memory().unwrap();
    let stop = SearchStop::new();
    assert!(stop.arm(conn.get_interrupt_handle()));
    stop.disarm();
    stop.hard();
    assert!(!stop.arm(conn.get_interrupt_handle()));
}

#[test]
fn the_row_hook_runs_every_n_rows() {
    let seen = Arc::new(AtomicU32::new(0));
    let hook_seen = Arc::clone(&seen);
    let stop =
        SearchStop::new().on_rows(3, Arc::new(move |rows| hook_seen.store(rows, Ordering::SeqCst)));
    for rows in 1..=7 {
        stop.row(rows);
    }
    assert_eq!(seen.load(Ordering::SeqCst), 6);
    assert!(format!("{stop:?}").contains("hook: Some(3)"));
}
