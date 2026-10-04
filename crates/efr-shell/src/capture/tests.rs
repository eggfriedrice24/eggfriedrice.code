use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{Capture, clean};

#[test]
fn crlf_becomes_lf() {
    assert_eq!(clean(b"one\r\ntwo\r\n"), "one\ntwo\n");
}

#[test]
fn colours_and_cursor_moves_are_dropped() {
    assert_eq!(clean(b"\x1b[1;31merror\x1b[0m: \x1b[Kgone\x1b[2J"), "error: gone");
}

#[test]
fn osc_sequences_end_at_bel_or_st() {
    assert_eq!(clean(b"a\x1b]7;kitty-shell-cwd://h/tmp\x07b\x1b]2;title\x1b\\c"), "abc");
}

#[test]
fn dcs_and_charset_sequences_are_dropped() {
    assert_eq!(clean(b"\x1bP1$r0m\x1b\\x\x1b(By\x1b=z"), "xyz");
}

#[test]
fn a_progress_bar_leaves_its_last_state() {
    assert_eq!(clean(b"10%\r50%\r100%\ndone\n"), "100%\ndone\n");
}

#[test]
fn a_trailing_carriage_return_keeps_the_line() {
    assert_eq!(clean(b"working\r"), "working");
}

#[test]
fn backspace_removes_the_character_before_it() {
    assert_eq!(clean("abc\u{e9}\x08d\n".as_bytes()), "abcd\n");
    assert_eq!(clean(b"\n\x08x"), "\nx");
}

#[test]
fn other_control_characters_go_but_tabs_stay() {
    assert_eq!(clean(b"a\tb\x07c\x00d\x7f"), "a\tbcd");
}

#[test]
fn an_unfinished_escape_at_the_end_is_dropped() {
    assert_eq!(clean(b"ok\x1b]133;"), "ok");
    assert_eq!(clean(b"ok\x1b["), "ok");
    assert_eq!(clean(b"ok\x1b"), "ok");
}

#[test]
fn bytes_that_are_not_utf8_are_replaced() {
    assert_eq!(clean(b"a\xffb"), "a\u{fffd}b");
}

#[test]
fn a_capture_under_its_limit_keeps_everything() {
    let mut capture = Capture::new(16);
    capture.push(b"hello ");
    capture.push(b"world");
    let captured = capture.finish();
    assert_eq!(captured.text, "hello world");
    assert!(!captured.truncated);
    assert_eq!(captured.bytes, 11);
}

#[test]
fn a_capture_over_its_limit_keeps_the_head_and_the_tail() {
    let mut capture = Capture::new(8);
    capture.push(b"0123");
    capture.push(b"456789");
    capture.push(b"abcdef");
    let captured = capture.finish();
    assert_eq!(captured.text, "0123\n[... 8 bytes omitted ...]\ncdef");
    assert!(captured.truncated);
    assert_eq!(captured.bytes, 16);
}

#[test]
fn trimming_takes_back_the_latest_bytes() {
    let mut capture = Capture::new(64);
    capture.push(b"out\r\n\x1b]13");
    capture.trim_end(4);
    assert_eq!(capture.finish().text, "out\n");
    assert_eq!(capture.total(), 5);
}

#[test]
fn trimming_after_a_gap_only_touches_the_tail() {
    let mut capture = Capture::new(4);
    capture.push(b"abcdefgh");
    capture.trim_end(3);
    let captured = capture.finish();
    assert_eq!(captured.text, "ab\n[... 3 bytes omitted ...]\n");
    assert_eq!(captured.bytes, 5);
}

#[test]
fn the_preview_is_the_latest_bytes() {
    let mut capture = Capture::new(8);
    capture.push(b"0123");
    assert_eq!(&capture.tail(2)[..], b"23");
    capture.push(b"45");
    assert_eq!(&capture.tail(4)[..], b"2345");
    capture.push(b"6789ab");
    assert_eq!(&capture.tail(3)[..], b"9ab");
}

proptest! {
    /// However the output is split into pushes, the capture is the same.
    #[test]
    fn pushes_split_anywhere_give_the_same_capture(
        output in proptest::collection::vec(any::<u8>(), 0..200),
        cuts in proptest::collection::vec(0usize..200, 0..8),
        limit in 2usize..64,
    ) {
        let mut whole = Capture::new(limit);
        whole.push(&output);
        let mut split = Capture::new(limit);
        let mut cuts: Vec<usize> = cuts.into_iter().map(|cut| cut.min(output.len())).collect();
        cuts.sort_unstable();
        let mut from = 0;
        for cut in cuts {
            split.push(&output[from..cut]);
            from = cut;
        }
        split.push(&output[from..]);
        prop_assert_eq!(whole.finish(), split.finish());
    }
}
