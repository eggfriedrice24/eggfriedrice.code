use efr_protocol::InputRespond;
use pretty_assertions::assert_eq;

use super::{AnswerLine, Edit};

/// Types `bytes` and returns the edit of each key.
fn type_bytes(line: &mut AnswerLine, bytes: &[u8]) -> Vec<Edit> {
    bytes.iter().map(|byte| line.key(*byte)).collect()
}

#[test]
fn printable_text_is_added_and_enter_submits_it() {
    let mut line = AnswerLine::new();
    let edits = type_bytes(&mut line, b"hunter2");
    assert!(edits.iter().all(|edit| *edit == Edit::Changed), "{edits:?}");
    assert_eq!(line.key(b'\r'), Edit::Submit);
    assert_eq!(line.text(), "hunter2");
    assert_eq!(line.take().expose_secret(), "hunter2");
    assert_eq!(line.text(), "");
}

#[test]
fn a_line_feed_submits_as_well() {
    let mut line = AnswerLine::new();
    type_bytes(&mut line, b"y");
    assert_eq!(line.key(b'\n'), Edit::Submit);
    assert_eq!(line.take().expose_secret(), "y");
}

#[test]
fn enter_on_an_empty_line_submits_an_empty_answer() {
    // A bare Enter takes the default of a `[Y/n]` question.
    let mut line = AnswerLine::new();
    assert_eq!(line.key(b'\r'), Edit::Submit);
    assert_eq!(line.take().expose_secret(), "");
}

#[test]
fn backspace_removes_one_whole_character() {
    for backspace in [0x7f, 0x08] {
        let mut line = AnswerLine::new();
        type_bytes(&mut line, "pa\u{df}\u{20ac}\u{1f511}".as_bytes());
        assert_eq!(line.text(), "pa\u{df}\u{20ac}\u{1f511}");
        assert_eq!(line.key(backspace), Edit::Changed);
        assert_eq!(line.text(), "pa\u{df}\u{20ac}");
        assert_eq!(line.key(backspace), Edit::Changed);
        assert_eq!(line.text(), "pa\u{df}");
        assert_eq!(line.key(backspace), Edit::Changed);
        assert_eq!(line.text(), "pa");
        type_bytes(&mut line, &[backspace, backspace]);
        assert_eq!(line.key(backspace), Edit::Unchanged, "nothing left to remove");
        assert_eq!(line.text(), "");
    }
}

#[test]
fn ctrl_u_clears_the_line() {
    let mut line = AnswerLine::new();
    type_bytes(&mut line, b"wrong");
    assert_eq!(line.key(0x15), Edit::Changed);
    assert_eq!(line.text(), "");
    assert_eq!(line.key(0x15), Edit::Unchanged);
    type_bytes(&mut line, b"right\r");
    assert_eq!(line.take().expose_secret(), "right");
}

#[test]
fn escape_sequences_are_ignored_whole() {
    let mut line = AnswerLine::new();
    type_bytes(&mut line, b"ab");
    // Left and up arrows, as CSI and as SS3, Delete, F5 and Alt+x.
    for sequence in
        [&b"\x1b[D"[..], b"\x1b[A", b"\x1bOD", b"\x1bOA", b"\x1b[3~", b"\x1b[15~", b"\x1bx"]
    {
        let edits = type_bytes(&mut line, sequence);
        assert!(edits.iter().all(|edit| *edit == Edit::Unchanged), "{sequence:?}: {edits:?}");
    }
    assert_eq!(line.text(), "ab");
    type_bytes(&mut line, b"c");
    assert_eq!(line.text(), "abc");
}

#[test]
fn a_lone_escape_leaves_enter_and_backspace_alone() {
    let mut line = AnswerLine::new();
    type_bytes(&mut line, b"ab\x1b\x7f");
    assert_eq!(line.text(), "a");
    assert_eq!(line.key(0x1b), Edit::Unchanged);
    assert_eq!(line.key(b'\r'), Edit::Submit);
}

#[test]
fn other_control_characters_are_ignored() {
    let mut line = AnswerLine::new();
    let edits = type_bytes(&mut line, b"\x00\x01\x04\x07\t\x0b\x1f");
    assert!(edits.iter().all(|edit| *edit == Edit::Unchanged), "{edits:?}");
    // C1 controls, written in UTF-8, are no text either.
    type_bytes(&mut line, "\u{85}\u{9b}".as_bytes());
    assert_eq!(line.text(), "");
}

#[test]
fn bytes_that_are_not_utf8_are_dropped() {
    let mut line = AnswerLine::new();
    // A stray continuation byte, a lead cut short by ASCII, an invalid lead.
    type_bytes(&mut line, &[0x80, b'a', 0xe2, 0x82, b'b', 0xff, 0xc0, b'c']);
    assert_eq!(line.text(), "abc");
}

#[test]
fn the_line_stops_growing_at_the_limit() {
    let mut line = AnswerLine::new();
    type_bytes(&mut line, &[b'a'; InputRespond::MAX_TEXT_BYTES]);
    assert_eq!(line.text().len(), InputRespond::MAX_TEXT_BYTES);
    assert_eq!(line.key(b'b'), Edit::Unchanged);
    // A multibyte character that would cross the limit is not cut either.
    line.key(0x7f);
    assert_eq!(type_bytes(&mut line, "\u{20ac}".as_bytes()).last(), Some(&Edit::Unchanged));
    assert_eq!(line.text().len(), InputRespond::MAX_TEXT_BYTES - 1);
}

#[test]
fn the_buffer_never_moves_while_it_grows() {
    let mut line = AnswerLine::new();
    let start = line.text().as_ptr();
    type_bytes(&mut line, &[b'x'; InputRespond::MAX_TEXT_BYTES + 10]);
    assert_eq!(line.text().as_ptr(), start);
    line.key(0x15);
    type_bytes(&mut line, b"again");
    assert_eq!(line.text().as_ptr(), start);
}

#[test]
fn debug_shows_the_length_and_never_the_text() {
    let mut line = AnswerLine::new();
    type_bytes(&mut line, b"hunter2");
    for shown in [format!("{line:?}"), format!("{line:#?}")] {
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains('7'), "{shown}");
    }
    assert!(!format!("{:?}", line.take()).contains("hunter2"));
}
