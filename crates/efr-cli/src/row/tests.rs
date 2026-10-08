use efr_render::WidthMethod;
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, proptest};

use super::{Action, Layout, MAX_BYTES, RowLine};
use crate::keys::Key;

/// Feeds `bytes` to `line` one byte at a time, as the key thread hands them over, and
/// returns the action of each key that asked for more than a redraw.
fn feed(line: &mut RowLine, bytes: &[u8]) -> Vec<Action> {
    bytes
        .iter()
        .map(|byte| line.key(Key::Byte(*byte)))
        .filter(|action| !matches!(action, Action::None | Action::Edited))
        .collect()
}

fn typed(bytes: &[u8]) -> RowLine {
    let mut line = RowLine::default();
    assert_eq!(feed(&mut line, bytes), [], "only edits");
    line
}

#[test]
fn printable_text_and_utf8_go_in_at_the_cursor() {
    let line = typed("grüß dich, 日本".as_bytes());
    assert_eq!(line.text(), "grüß dich, 日本");
    assert_eq!(line.cursor(), line.text().len());
    // The bytes of one character wait until it is whole.
    let mut line = RowLine::default();
    assert_eq!(line.key(Key::Byte(0xc3)), Action::None);
    assert_eq!(line.text(), "");
    assert_eq!(line.key(Key::Byte(0xbc)), Action::Edited);
    assert_eq!(line.text(), "ü");
}

#[test]
fn a_character_cut_short_is_dropped_and_the_next_byte_counts() {
    let line = typed(b"a\xc3b");
    assert_eq!(line.text(), "ab");
}

#[test]
fn backspace_and_delete_remove_one_grapheme_cluster() {
    // An e with a combining accent, and a flag of two regional indicators.
    let mut line = typed("e\u{301}\u{1f1e9}\u{1f1ea}".as_bytes());
    feed(&mut line, b"\x7f");
    assert_eq!(line.text(), "e\u{301}");
    feed(&mut line, b"\x08");
    assert_eq!(line.text(), "");
    let mut line = typed("ab\u{1f1e9}\u{1f1ea}".as_bytes());
    // Home, then Delete twice: the a, then the b; the flag stays whole.
    feed(&mut line, b"\x01\x1b[3~\x1b[3~");
    assert_eq!(line.text(), "\u{1f1e9}\u{1f1ea}");
    assert_eq!(line.cursor(), 0);
}

#[test]
fn arrows_move_over_grapheme_clusters_and_text_goes_in_where_the_cursor_is() {
    let mut line = typed("ae\u{301}c".as_bytes());
    // Left twice passes c and the accented e.
    feed(&mut line, b"\x1b[D\x1bOD");
    assert_eq!(line.cursor(), 1);
    feed(&mut line, b"X\x1b[CY");
    assert_eq!(line.text(), "aXe\u{301}Yc");
    // Ctrl+B and Ctrl+F do the same.
    feed(&mut line, b"\x02\x02Z\x06");
    assert_eq!(line.text(), "aXZe\u{301}Yc");
}

#[test]
fn home_and_end_go_to_the_ends_of_the_line_of_the_cursor() {
    let mut line = typed(b"first\nsecond");
    feed(&mut line, b"\x1b[H");
    assert_eq!(line.cursor(), "first\n".len());
    feed(&mut line, b"\x1b[D\x01");
    assert_eq!(line.cursor(), 0, "Ctrl+A on the first line");
    feed(&mut line, b"\x05");
    assert_eq!(line.cursor(), "first".len(), "Ctrl+E stops at the newline");
    feed(&mut line, b"\x1b[F\x1b[1~");
    assert_eq!(line.cursor(), 0, "End, then Home on the same line");
    feed(&mut line, b"\x1b[4~");
    assert_eq!(line.cursor(), "first".len());
}

#[test]
fn ctrl_u_k_and_w_remove_parts_of_the_line() {
    let mut line = typed(b"one\ntwo three four");
    feed(&mut line, b"\x17");
    assert_eq!(line.text(), "one\ntwo three ", "Ctrl+W takes the word before the cursor");
    feed(&mut line, b"\x17");
    assert_eq!(line.text(), "one\ntwo ", "and the blanks after the word before it");
    feed(&mut line, b"\x1b[D\x1b[D\x0b");
    assert_eq!(line.text(), "one\ntw", "Ctrl+K takes the rest of the line");
    feed(&mut line, b"\x15");
    assert_eq!(line.text(), "one\n", "Ctrl+U takes the line up to the cursor");
}

#[test]
fn alt_b_and_alt_f_move_by_words_of_letters_and_digits() {
    let mut line = typed(b"cargo test -p efr-cli");
    feed(&mut line, b"\x1bb");
    assert_eq!(line.cursor(), "cargo test -p efr-".len());
    feed(&mut line, b"\x1bb\x1bb");
    assert_eq!(line.cursor(), "cargo test -".len());
    feed(&mut line, b"\x1bf");
    assert_eq!(line.cursor(), "cargo test -p".len());
    // Ctrl+Left and Alt+Right in xterm's form.
    feed(&mut line, b"\x1b[1;5D\x1b[1;3C");
    assert_eq!(line.cursor(), "cargo test -p".len());
    // Alt+Backspace takes the word before the cursor.
    feed(&mut line, b"\x1b\x7f");
    assert_eq!(line.text(), "cargo test - efr-cli");
}

#[test]
fn enter_steers_tab_queues_esc_interrupts_and_alt_up_takes_back() {
    let mut line = typed(b"fix it");
    assert_eq!(line.key(Key::Byte(b'\r')), Action::Steer);
    assert_eq!(line.key(Key::Byte(b'\t')), Action::Queue);
    assert_eq!(line.key(Key::Esc), Action::Interrupt);
    assert_eq!(feed(&mut line, b"\x1b[1;3A"), [Action::Withdraw]);
    assert_eq!(
        feed(&mut line, b"\x1b\x1b[A"),
        [Action::Withdraw],
        "Alt+Up as some terminals send it"
    );
    assert_eq!(feed(&mut line, b"\x1b\x1bOA"), [Action::Withdraw]);
    // Up and Down alone mean nothing.
    assert_eq!(feed(&mut line, b"\x1b[A\x1b[B\x1bOA"), []);
    assert_eq!(line.text(), "fix it", "the keys that ask for something leave the text");
}

#[test]
fn ctrl_j_and_alt_enter_add_a_newline() {
    let mut line = typed(b"one\ntwo");
    assert_eq!(feed(&mut line, b"\x1b\rthree"), []);
    assert_eq!(line.text(), "one\ntwo\nthree");
}

#[test]
fn esc_in_the_middle_of_a_sequence_still_interrupts() {
    let mut line = typed(b"x\x1b[1");
    assert_eq!(line.key(Key::Esc), Action::Interrupt);
    assert_eq!(feed(&mut line, b"y"), []);
    assert_eq!(line.text(), "xy", "the sequence was dropped");
}

#[test]
fn a_paste_keeps_its_newlines_and_never_sends() {
    let mut line = typed(b"see: ");
    let paste = b"\x1b[200~line one\r\nline two\rthree\x07\tend\x1b[201~";
    assert_eq!(feed(&mut line, paste), [], "Enter and Tab in a paste are text");
    assert_eq!(line.text(), "see: line one\nline two\nthree\tend");
    assert_eq!(line.key(Key::Byte(b'\r')), Action::Steer, "a typed Enter after it sends");
}

#[test]
fn a_paste_may_hold_what_looks_like_the_start_of_its_end() {
    let mut line = RowLine::default();
    feed(&mut line, b"\x1b[200~a\x1b[20b\x1b\x1b[201~");
    assert_eq!(line.text(), "a[20b", "escape bytes are control characters and go");
    let mut line = RowLine::default();
    line.key(Key::Byte(0x1b));
    feed(&mut line, b"[200~");
    assert_eq!(line.key(Key::Esc), Action::None, "an escape byte alone in a paste is text");
    feed(&mut line, b"x\x1b[201~");
    assert_eq!(line.text(), "x");
}

#[test]
fn unknown_sequences_and_control_characters_change_nothing() {
    let mut line = typed(b"keep");
    feed(&mut line, b"\x1b[15~\x1b[Z\x1bOP\x1bx\x00\x07\x1b[?1;2c");
    assert_eq!(line.text(), "keep");
    assert_eq!(line.cursor(), 4);
}

#[test]
fn appended_text_goes_on_a_line_of_its_own_after_the_draft() {
    let mut line = RowLine::default();
    line.append("first prompt");
    assert_eq!(line.text(), "first prompt");
    line.append("second\n");
    assert_eq!(line.text(), "first prompt\nsecond\n");
    line.append("third");
    assert_eq!(line.text(), "first prompt\nsecond\nthird");
    assert_eq!(line.cursor(), line.text().len());
}

#[test]
fn take_and_clear_empty_the_line() {
    let mut line = typed(b"  ");
    assert!(line.is_blank() && !line.is_empty());
    assert_eq!(line.take(), "  ");
    assert!(line.is_empty());
    assert!(!line.clear(), "nothing to clear");
    let mut line = typed(b"x");
    assert!(line.clear());
    assert_eq!((line.text(), line.cursor()), ("", 0));
}

#[test]
fn the_line_stops_growing_at_its_limit_on_a_character() {
    let mut line = RowLine::default();
    line.append(&"x".repeat(MAX_BYTES - 1));
    line.append("\u{e4}");
    assert_eq!(line.text().len(), MAX_BYTES, "the newline fits, the two-byte character does not");
    assert_eq!(feed(&mut line, b"y"), []);
    assert_eq!(line.text().len(), MAX_BYTES);
}

#[test]
fn the_layout_wraps_lines_and_finds_the_cursor() {
    let mut line = typed(b"0123456789abc\nxy");
    assert_eq!(
        line.layout(5, WidthMethod::CodePoint),
        Layout {
            rows: vec!["01234".into(), "56789".into(), "abc".into(), "xy".into()],
            cursor_row: 3,
            cursor_col: 2,
        }
    );
    feed(&mut line, b"\x1b[A\x01");
    let layout = line.layout(5, WidthMethod::CodePoint);
    assert_eq!((layout.cursor_row, layout.cursor_col), (3, 0));
    // The cursor before the sixth character starts the second row.
    let mut line = typed(b"0123456789");
    for _ in 0..5 {
        feed(&mut line, b"\x1b[D");
    }
    let layout = line.layout(5, WidthMethod::CodePoint);
    assert_eq!((layout.cursor_row, layout.cursor_col), (1, 0));
}

#[test]
fn the_layout_never_splits_a_cluster_and_shows_no_control_character() {
    let line = typed("ab\u{1f468}\u{200d}\u{1f469}".as_bytes());
    let by_cluster = line.layout(3, WidthMethod::Grapheme);
    assert_eq!(by_cluster.rows, ["ab", "\u{1f468}\u{200d}\u{1f469}"]);
    let mut line = RowLine::default();
    feed(&mut line, b"\x1b[200~a\tb\x1b[201~");
    assert_eq!(line.layout(10, WidthMethod::CodePoint).rows, ["a b"]);
}

#[test]
fn an_empty_line_has_one_empty_row() {
    let layout = RowLine::default().layout(10, WidthMethod::CodePoint);
    assert_eq!(layout, Layout { rows: vec![String::new()], cursor_row: 0, cursor_col: 0 });
}

proptest! {
    #[test]
    fn any_keys_keep_the_cursor_on_a_boundary_and_every_row_within_the_width(
        bytes in prop::collection::vec(prop::sample::select(vec![
            b'a', b'z', b' ', b'\n', 0x7f, 0x01, 0x05, 0x15, 0x0b, 0x17, 0x1b, b'[', b'C',
            b'D', b'3', b'~', b'b', b'f', 0xc3, 0xa4, 0xe6, 0x97, 0xa5,
        ]), 0..80),
        columns in 1_usize..12,
    ) {
        let mut line = RowLine::default();
        for byte in bytes {
            line.key(Key::Byte(byte));
            assert!(line.text().is_char_boundary(line.cursor()));
        }
        let layout = line.layout(columns, WidthMethod::CodePoint);
        for row in &layout.rows {
            let width = efr_render::text_width(row, WidthMethod::CodePoint);
            // A character wider than a row gets a row of its own.
            assert!(width <= columns.max(2), "{row:?} at {columns}");
        }
        assert!(layout.cursor_row < layout.rows.len());
    }
}

#[test]
fn a_cut_sequence_is_dropped_and_a_paste_goes_on_or_ends_with_its_text() {
    // An escape sequence that a question cut: the next key starts afresh.
    let mut line = typed(b"ab\x1b[");
    line.drop_partial();
    assert_eq!(feed(&mut line, b"\r"), [Action::Steer], "Enter steers again");
    assert_eq!(line.text(), "ab");

    // A paste goes on after the question.
    let mut line = typed(b"\x1b[200~one");
    assert!(line.pasting());
    line.drop_partial();
    assert!(line.pasting());
    assert_eq!(feed(&mut line, b"\ntwo\x1b[201~"), []);
    assert!(!line.pasting());
    assert_eq!(line.text(), "one\ntwo");

    // A paste whose end never comes ends with the text that came, also a part of the
    // end that turned out to be text.
    let mut line = typed(b"\x1b[200~one\x1b[2");
    line.end_paste();
    assert!(!line.pasting());
    assert_eq!(line.text(), "one[2", "control characters stay out, as in a paste");
    assert_eq!(feed(&mut line, b"\r"), [Action::Steer]);
}
