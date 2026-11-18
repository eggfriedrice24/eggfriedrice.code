use pretty_assertions::assert_eq;
use proptest::prelude::*;

use efr_shell::OutputGap;
use efr_stdx::text::omitted_marker;

use super::{DEFAULT_OUTPUT_LIMIT, Truncated, truncate_around, truncate_middle};

#[test]
fn text_that_fits_is_unchanged() {
    let cut = truncate_middle("short", 10);
    assert_eq!(cut.text, "short");
    assert!(!cut.truncated);
    assert_eq!(cut.omitted, 0);
}

#[test]
fn long_text_keeps_its_head_and_tail_around_a_marker() {
    let text = "a".repeat(100) + &"b".repeat(100);
    let cut = truncate_middle(&text, 100);
    assert!(cut.truncated);
    assert!(cut.text.len() <= 100, "{}", cut.text.len());
    assert!(cut.text.starts_with("aaaa"));
    assert!(cut.text.ends_with("bbbb"));
    assert!(cut.text.contains(&format!("[... {} bytes omitted ...]", cut.omitted)));
}

#[test]
fn cuts_move_to_line_ends_when_one_is_near() {
    let text: String = (0..200).map(|n| format!("line {n:03}\n")).collect();
    let cut = truncate_middle(&text, 400);
    let (head, rest) = cut.text.split_once("\n[...").unwrap();
    assert!(head.ends_with('\n'), "{head:?}");
    let last_line = head.trim_end_matches('\n').rsplit('\n').next().unwrap();
    assert_eq!(last_line.len(), "line 000".len(), "{head:?}");
    let tail = rest.split_once("...]\n").unwrap().1;
    assert!(tail.starts_with("line "), "{tail:?}");
}

#[test]
fn cuts_never_split_a_character() {
    let text = "\u{e9}".repeat(1000);
    let cut = truncate_middle(&text, 101);
    assert!(cut.truncated);
    assert!(cut.text.len() <= 101);
}

#[test]
fn a_limit_below_the_marker_gives_the_marker_alone() {
    let cut = truncate_middle(&"x".repeat(50), 5);
    assert_eq!(cut.text, "\n[... 50 bytes omitted ...]\n");
}

#[test]
fn the_default_limit_is_32_kib() {
    assert_eq!(DEFAULT_OUTPUT_LIMIT, 32 * 1024);
}

proptest! {
    #[test]
    fn the_cut_fits_and_keeps_a_prefix_and_a_suffix(text in "\\PC{0,400}", limit in 40usize..300) {
        let cut = truncate_middle(&text, limit);
        prop_assert!(cut.text.len() <= limit.max(text.len().min(limit)));
        if cut.truncated {
            let marker = format!("\n[... {} bytes omitted ...]\n", cut.omitted);
            let (head, tail) = cut.text.split_once(&marker).unwrap();
            prop_assert!(text.starts_with(head));
            prop_assert!(text.ends_with(tail));
            prop_assert_eq!(head.len() as u64 + cut.omitted + tail.len() as u64, text.len() as u64);
        } else {
            prop_assert_eq!(&cut.text, &text);
        }
    }

    /// Around the shell's marker, the cut keeps a prefix of the text before it and a
    /// suffix of the text after it, and its one marker counts every byte left out.
    #[test]
    fn the_cut_around_a_gap_counts_both_cuts(
        before in "\\PC{0,300}",
        after in "\\PC{0,300}",
        dropped in 0u64..10_000_000,
        limit in 60usize..400,
    ) {
        let (text, gap) = shell_output(&before, dropped, &after);
        let cut = truncate_around(&text, limit, Some(gap));
        if !cut.truncated {
            prop_assert_eq!(&cut.text, &text);
            return Ok(());
        }
        prop_assert!(cut.text.len() <= limit);
        let marker = omitted_marker(cut.omitted);
        prop_assert_eq!(cut.text.matches(&marker).count(), 1);
        let (head, tail) = cut.text.split_once(&marker).unwrap();
        prop_assert!(before.starts_with(head));
        prop_assert!(after.ends_with(tail));
        let whole = (before.len() + after.len()) as u64 + dropped;
        prop_assert_eq!(head.len() as u64 + cut.omitted + tail.len() as u64, whole);
    }
}

/// The lines `from`, `from + 1` and on, as `seq` prints them, up to `bytes` bytes.
fn numbered_lines(from: usize, bytes: usize) -> String {
    let mut text = String::with_capacity(bytes);
    for n in from.. {
        let line = format!("{n}\n");
        if text.len() + line.len() > bytes {
            break;
        }
        text.push_str(&line);
    }
    text
}

/// A text with the shell's marker of `dropped` bytes between `before` and `after`.
fn shell_output(before: &str, dropped: u64, after: &str) -> (String, OutputGap) {
    let text = format!("{before}{}{after}", omitted_marker(dropped));
    (text, OutputGap { at: before.len(), dropped })
}

#[test]
fn a_cut_of_a_cut_output_writes_one_marker_with_the_sum() {
    const KIB: usize = 1024;
    let before = numbered_lines(1, 512 * KIB);
    let after = numbered_lines(900_000, 512 * KIB);
    let dropped = 4 * 1024 * 1024;
    let (text, gap) = shell_output(&before, dropped, &after);
    let cut = truncate_around(&text, DEFAULT_OUTPUT_LIMIT, Some(gap));
    assert!(cut.truncated);
    assert!(cut.text.len() <= DEFAULT_OUTPUT_LIMIT);
    assert_eq!(cut.text.matches("bytes omitted").count(), 1, "{}", cut.text);
    let (head, tail) = cut.text.split_once(&omitted_marker(cut.omitted)).unwrap();
    let whole = (before.len() + after.len()) as u64 + dropped;
    assert_eq!(cut.omitted, whole - (head.len() + tail.len()) as u64);
    assert!(cut.omitted > 5_000_000, "{}", cut.omitted);
}

#[test]
fn a_cut_output_that_fits_keeps_the_shells_marker() {
    let (text, gap) = shell_output("head\n", 4096, "tail\n");
    let cut = truncate_around(&text, DEFAULT_OUTPUT_LIMIT, Some(gap));
    assert_eq!(cut, Truncated { text, truncated: false, omitted: 0 });
}

#[test]
fn a_gap_that_does_not_point_at_its_marker_is_no_gap() {
    let text = "a".repeat(100) + &"b".repeat(100);
    let gap = OutputGap { at: 100, dropped: 7 };
    assert_eq!(truncate_around(&text, 100, Some(gap)), truncate_middle(&text, 100));
}
