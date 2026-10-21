use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{STREAM_LINE_LIMIT, StreamClean};
use crate::capture::clean;

/// The text of `chunks` through one [`StreamClean`], with its end.
fn streamed(chunks: &[&[u8]]) -> String {
    let mut cleaner = StreamClean::default();
    let mut text: String = chunks.iter().map(|chunk| cleaner.push(chunk)).collect();
    text.push_str(&cleaner.finish());
    text
}

#[test]
fn stream_clean_drops_csi_and_osc_split_across_chunks() {
    assert_eq!(streamed(&[b"a\x1b[1;3", b"1mred\x1b[0", b"m\n"]), "ared\n");
    assert_eq!(streamed(&[b"a\x1b]7;kitty-shell-cwd://h/t", b"mp\x07b\n"]), "ab\n");
    // ST split between its ESC and its backslash.
    assert_eq!(streamed(&[b"a\x1b]2;title\x1b", b"\\c\n"]), "ac\n");
    assert_eq!(streamed(&[b"x\x1bP1$r", b"0m\x1b\\y\x1b(", b"Bz\n"]), "xyz\n");
}

#[test]
fn stream_clean_gives_out_a_line_only_once_it_ends() {
    let mut cleaner = StreamClean::default();
    assert_eq!(cleaner.push(b"one\ntw"), "one\n");
    assert_eq!(cleaner.push(b"o"), "");
    assert_eq!(cleaner.push(b"\r\nthree"), "two\n");
    assert_eq!(cleaner.finish(), "three");
}

#[test]
fn stream_clean_keeps_the_last_state_of_a_progress_line() {
    assert_eq!(streamed(&[b"10%\r", b"50%", b"\r100%\n", b"done\n"]), "100%\ndone\n");
    assert_eq!(streamed(&[b"working", b"\r"]), "working");
    assert_eq!(streamed(&[b"abc\xc3", b"\xa9\x08", b"d\n"]), "abcd\n");
}

#[test]
fn stream_clean_keeps_a_utf8_character_split_across_chunks() {
    let text = "gr\u{fc}\u{df}e \u{1f600}\n".as_bytes();
    for cut in 0..=text.len() {
        assert_eq!(streamed(&[&text[..cut], &text[cut..]]), "gr\u{fc}\u{df}e \u{1f600}\n");
    }
}

#[test]
fn stream_clean_reads_a_bare_esc_at_a_chunk_end_with_the_next_chunk() {
    assert_eq!(streamed(&[b"a\x1b", b"[31mb\n"]), "ab\n");
    assert_eq!(streamed(&[b"a\x1b", b"=b\n"]), "ab\n");
    assert_eq!(streamed(&[b"ok\x1b"]), "ok");
    assert_eq!(streamed(&[b"ok\x1b]133;"]), "ok");
}

#[test]
fn stream_clean_gives_out_a_long_line_without_an_end() {
    let mut cleaner = StreamClean::default();
    let mut line = vec![b'x'; STREAM_LINE_LIMIT];
    line.extend_from_slice("\u{e9}".as_bytes());
    // The cut falls inside the last character, which stays whole.
    let given = cleaner.push(&line[..line.len() - 1]);
    assert_eq!(given.len(), STREAM_LINE_LIMIT);
    assert!(given.bytes().all(|byte| byte == b'x'));
    assert_eq!(cleaner.push(&line[line.len() - 1..]), "");
    assert_eq!(cleaner.push(b"\rnew"), "");
    assert_eq!(cleaner.finish(), "new");
}

#[test]
fn stream_clean_mark_drops_the_start_of_a_sequence() {
    let mut cleaner = StreamClean::default();
    assert_eq!(cleaner.push(b"out\n\x1b]133;"), "out\n");
    cleaner.mark();
    assert_eq!(cleaner.push(b"next\n"), "next\n");
}

#[test]
fn stream_clean_breaks_a_line_for_missing_bytes() {
    let mut cleaner = StreamClean::default();
    assert_eq!(cleaner.push(b"half a li"), "");
    assert_eq!(cleaner.break_line(), "half a li\n");
    assert_eq!(cleaner.break_line(), "");
    assert_eq!(cleaner.push(b"\x1b[3"), "");
    assert_eq!(cleaner.break_line(), "");
    assert_eq!(cleaner.push(b"1mred\n"), "1mred\n");
}

proptest! {
    /// However the output is split into chunks, the streaming cleaner gives the text of
    /// the cleaner over the whole output.
    #[test]
    fn stream_clean_split_anywhere_gives_the_text_of_clean(
        output in proptest::collection::vec(
            prop_oneof![
                any::<u8>(),
                Just(0x1b_u8),
                Just(b'\r'),
                Just(b'\n'),
                Just(0x08_u8),
                Just(b'['),
                Just(b']'),
                Just(0x07_u8),
                Just(b'\\'),
            ],
            0..300,
        ),
        cuts in proptest::collection::vec(0usize..300, 0..12),
    ) {
        let mut cuts: Vec<usize> = cuts.into_iter().map(|cut| cut.min(output.len())).collect();
        cuts.sort_unstable();
        let mut chunks: Vec<&[u8]> = Vec::new();
        let mut from = 0;
        for cut in cuts {
            chunks.push(&output[from..cut]);
            from = cut;
        }
        chunks.push(&output[from..]);
        prop_assert_eq!(streamed(&chunks), clean(&output));
    }

    /// No bytes, however they are cut and whatever comes between, make it panic.
    #[test]
    fn stream_clean_never_panics(
        chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..16),
        steps in proptest::collection::vec(0u8..4, 0..16),
    ) {
        let mut cleaner = StreamClean::default();
        for (chunk, step) in chunks.iter().zip(steps.iter().chain(std::iter::repeat(&0))) {
            let _ = cleaner.push(chunk);
            match step {
                1 => cleaner.mark(),
                2 => {
                    let _ = cleaner.break_line();
                }
                _ => {}
            }
        }
        let _ = cleaner.finish();
    }
}
