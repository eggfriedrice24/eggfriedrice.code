use std::time::Duration;

use bytes::Bytes;
use jiff::{SignedDuration, Timestamp};

use super::{LiveTail, Step};
use crate::run::Progress;

const INTERVAL: Duration = Duration::from_millis(200);

/// A redraw of a two-line display, which needs a screen.
const REDRAW: &[u8] = b"\x1b[2Aone 2\r\ntwo 2\r\n";

fn at(millis: i64) -> Timestamp {
    Timestamp::UNIX_EPOCH + SignedDuration::from_millis(millis)
}

fn progress(bytes: &[u8]) -> Progress {
    Progress { bytes: bytes.len() as u64, tail: Bytes::copy_from_slice(bytes), started: true }
}

#[test]
fn text_without_cursor_movement_is_cleaned_at_every_change() {
    let mut live = LiveTail::new(INTERVAL);
    for millis in [0, 1, 2] {
        assert!(matches!(live.offer(progress(b"50%\r60%"), at(millis)), Step::Clean(_)));
    }
}

#[test]
fn the_first_redraw_is_read_on_a_screen_at_once() {
    let mut live = LiveTail::new(INTERVAL);
    match live.offer(progress(REDRAW), at(0)) {
        Step::Screen(window) => assert_eq!(window, REDRAW),
        step => panic!("{step:?}"),
    }
}

#[test]
fn a_redraw_within_the_interval_is_held_until_it_has_passed() {
    let mut live = LiveTail::new(INTERVAL);
    assert!(matches!(live.offer(progress(REDRAW), at(0)), Step::Screen(_)));
    match live.offer(progress(b"\x1b[2Aone 3\r\ntwo 3\r\n"), at(50)) {
        Step::Hold(left) => assert_eq!(left, Duration::from_millis(150)),
        step => panic!("{step:?}"),
    }
    // A newer redraw replaces the held one and waits for the same moment.
    match live.offer(progress(b"\x1b[2Aone 4\r\ntwo 4\r\n"), at(120)) {
        Step::Hold(left) => assert_eq!(left, Duration::from_millis(80)),
        step => panic!("{step:?}"),
    }
    assert_eq!(live.due(at(200)).unwrap(), &b"\x1b[2Aone 4\r\ntwo 4\r\n"[..]);
    assert!(live.due(at(200)).is_none(), "nothing is held twice");
    // The interval starts again at the read.
    assert!(matches!(live.offer(progress(REDRAW), at(300)), Step::Hold(_)));
    assert!(matches!(live.offer(progress(REDRAW), at(400)), Step::Screen(_)));
}

#[test]
fn a_change_that_needs_no_screen_drops_the_held_redraw() {
    let mut live = LiveTail::new(INTERVAL);
    assert!(matches!(live.offer(progress(REDRAW), at(0)), Step::Screen(_)));
    assert!(matches!(live.offer(progress(REDRAW), at(10)), Step::Hold(_)));
    // The window holds no cursor movement any more.
    assert!(matches!(live.offer(progress(b"done\r\n"), at(20)), Step::Clean(_)));
    assert!(live.due(at(200)).is_none());
}

#[test]
fn a_zero_interval_reads_every_redraw_on_a_screen() {
    let mut live = LiveTail::new(Duration::ZERO);
    for millis in [0, 0, 1] {
        assert!(matches!(live.offer(progress(REDRAW), at(millis)), Step::Screen(_)));
    }
}

#[test]
fn a_window_cut_out_of_the_output_starts_after_its_first_line_feed() {
    let cut = Progress {
        bytes: 10_000,
        tail: Bytes::from_static(b"5;1Hrest\r\nnext\r\n"),
        started: true,
    };
    assert_eq!(cut.window(), &b"next\r\n"[..]);
    // The whole output, or a cut one without a line feed to start at, stays as it is.
    let whole = progress(b"5;1Hrest\r\nnext\r\n");
    assert_eq!(whole.window(), &b"5;1Hrest\r\nnext\r\n"[..]);
    let one_line =
        Progress { bytes: 10_000, tail: Bytes::from_static(b"50%\r60%\n"), started: true };
    assert_eq!(one_line.window(), &b"50%\r60%\n"[..]);
}
