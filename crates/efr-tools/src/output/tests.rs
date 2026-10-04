use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{DEFAULT_OUTPUT_LIMIT, truncate_middle};

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
            prop_assert_eq!(head.len() + cut.omitted + tail.len(), text.len());
        } else {
            prop_assert_eq!(&cut.text, &text);
        }
    }
}
