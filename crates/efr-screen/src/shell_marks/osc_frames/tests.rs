use pretty_assertions::assert_eq;

use super::{MAX_BODY, OscFrames};

/// A frame as `(start, end, body)`, which reads well in assertions.
type Frame = (u64, u64, String);

/// Feeds `chunks` in order, starting at stream offset `base`.
fn frames_from(base: u64, chunks: &[&[u8]]) -> Vec<Frame> {
    let mut scanner = OscFrames::default();
    let mut found = Vec::new();
    let mut offset = base;
    for chunk in chunks {
        scanner.push(chunk, offset, |frame| {
            found.push((frame.start, frame.end, String::from_utf8_lossy(frame.body).into_owned()));
        });
        offset += chunk.len() as u64;
    }
    found
}

fn frames(bytes: &[u8]) -> Vec<Frame> {
    frames_from(0, &[bytes])
}

fn frame(start: u64, end: u64, body: &str) -> Frame {
    (start, end, body.to_owned())
}

#[test]
fn bel_ends_an_osc() {
    assert_eq!(frames(b"\x1b]133;A\x07"), vec![frame(0, 8, "133;A")]);
}

#[test]
fn esc_backslash_ends_an_osc() {
    assert_eq!(frames(b"\x1b]133;B\x1b\\"), vec![frame(0, 9, "133;B")]);
}

#[test]
fn offsets_count_from_the_chunk_base() {
    assert_eq!(frames_from(1000, &[b"ab\x1b]7;x\x07cd"]), vec![frame(1002, 1008, "7;x")]);
}

#[test]
fn plain_output_holds_no_frames() {
    assert_eq!(frames(b"hello world\r\n\x07"), vec![]);
}

#[test]
fn two_frames_in_one_chunk() {
    assert_eq!(
        frames(b"\x1b]133;C\x07output\x1b]133;D;0\x07"),
        vec![frame(0, 8, "133;C"), frame(14, 24, "133;D;0")]
    );
}

#[test]
fn a_frame_split_at_every_position_is_found_once() {
    let bytes = b"ab\x1b]133;A;aid=7\x1b\\cd";
    let whole = frames(bytes);
    assert_eq!(whole, vec![frame(2, 17, "133;A;aid=7")]);
    for split in 1..bytes.len() {
        let (left, right) = bytes.split_at(split);
        assert_eq!(frames_from(0, &[left, right]), whole, "split at {split}");
    }
}

#[test]
fn one_byte_chunks_find_the_same_frames() {
    let bytes = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07";
    let chunks: Vec<&[u8]> = bytes.chunks(1).collect();
    assert_eq!(frames_from(0, &chunks), frames(bytes));
}

#[test]
fn a_body_at_the_cap_is_kept() {
    let body = "x".repeat(MAX_BODY);
    let bytes = format!("\x1b]{body}\x07");
    let found = frames(bytes.as_bytes());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2.len(), MAX_BODY);
}

#[test]
fn a_body_over_the_cap_is_dropped_and_scanning_resumes() {
    let body = "x".repeat(MAX_BODY + 1);
    let bytes = format!("\x1b]{body}\x07\x1b]133;A\x07");
    let start = (bytes.len() - 8) as u64;
    assert_eq!(frames(bytes.as_bytes()), vec![frame(start, start + 8, "133;A")]);
}

#[test]
fn an_overlong_body_ended_by_esc_backslash_is_dropped() {
    let body = "x".repeat(MAX_BODY + 10);
    let bytes = format!("\x1b]{body}\x1b\\done");
    assert_eq!(frames(bytes.as_bytes()), vec![]);
}

#[test]
fn an_osc_inside_a_dcs_payload_is_not_a_frame() {
    assert_eq!(frames(b"\x1bPq]133;A\x07#0;2;0;0;0\x1b\\"), vec![]);
}

#[test]
fn every_string_kind_is_skipped() {
    for &intro in b"PX^_" {
        let bytes = [b"\x1b".as_slice(), &[intro], b"]7;x\x07\x1b\\\x1b]133;B\x07"].concat();
        assert_eq!(frames(&bytes), vec![frame(9, 17, "133;B")], "intro {}", intro as char);
    }
}

#[test]
fn a_tmux_passthrough_hides_the_inner_osc() {
    let bytes = b"\x1bPtmux;\x1b\x1b]133;A\x07\x1b\x1b\\\x1b\\\x1b]133;B\x07";
    let found = frames(bytes);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2, "133;B");
}

#[test]
fn a_stray_dcs_does_not_hide_the_next_osc() {
    let bytes = b"\x1bP\x00\x7f garbage \x1b]133;D;0\x07";
    assert_eq!(frames(bytes), vec![frame(13, 23, "133;D;0")]);
}

#[test]
fn esc_without_backslash_aborts_the_osc_and_starts_a_new_sequence() {
    let bytes = b"\x1b]133;D;1\x1b[0m\x1b]133;A\x07";
    assert_eq!(frames(bytes), vec![frame(13, 21, "133;A")]);
}

#[test]
fn an_osc_opened_inside_an_osc_replaces_it() {
    assert_eq!(frames(b"\x1b]0;tit\x1b]133;B\x07"), vec![frame(7, 15, "133;B")]);
}

#[test]
fn a_doubled_esc_before_the_bracket_starts_at_the_second_esc() {
    assert_eq!(frames(b"\x1b\x1b]133;C\x07"), vec![frame(1, 9, "133;C")]);
}

#[test]
fn can_and_sub_abort_an_osc() {
    for abort in [0x18_u8, 0x1a] {
        let bytes = [b"\x1b]133;A".as_slice(), &[abort], b"\x07\x1b]133;B\x07"].concat();
        assert_eq!(frames(&bytes), vec![frame(9, 17, "133;B")]);
    }
}

#[test]
fn can_aborts_a_skipped_string() {
    assert_eq!(frames(b"\x1b_payload\x18\x1b]7;x\x07"), vec![frame(10, 16, "7;x")]);
}

#[test]
fn c0_controls_inside_a_body_are_ignored() {
    assert_eq!(frames(b"\x1b]133;\nA\x07"), vec![frame(0, 9, "133;A")]);
}

#[test]
fn a_c0_control_between_esc_and_bracket_keeps_the_escape() {
    assert_eq!(frames(b"\x1b\r]7;x\x07"), vec![frame(0, 7, "7;x")]);
}

#[test]
fn other_escape_sequences_are_not_frames() {
    assert_eq!(frames(b"\x1b[31mred\x1b[0m\x1b(B\x1b7\x1b8"), vec![]);
}

#[test]
fn a_bracket_after_an_intermediate_is_not_an_osc() {
    assert_eq!(frames(b"\x1b ]133;A\x07"), vec![]);
}

#[test]
fn non_ascii_body_bytes_are_kept() {
    let found = frames("\x1b]7;file:///tmp/caf\u{e9}\x07".as_bytes());
    assert_eq!(found, vec![frame(0, 22, "7;file:///tmp/caf\u{e9}")]);
}

#[test]
fn reset_forgets_a_partial_frame() {
    let mut scanner = OscFrames::default();
    let mut found = Vec::new();
    scanner.push(b"\x1b]133;", 0, |frame| found.push(frame.body.to_vec()));
    scanner.reset();
    scanner.push(b"A\x07\x1b]133;B\x07", 100, |frame| found.push(frame.body.to_vec()));
    assert_eq!(found, vec![b"133;B".to_vec()]);
}
