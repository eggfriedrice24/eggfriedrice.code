use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{Ending, Line, join, split};

#[test]
fn split_keeps_each_ending_and_the_final_newline() {
    let lines = split("a\r\nb\nc");
    assert_eq!(
        lines.lines,
        vec![
            Line { text: "a", ending: Some(Ending::CrLf) },
            Line { text: "b", ending: Some(Ending::Lf) },
            Line { text: "c", ending: None },
        ]
    );
    assert_eq!(lines.preferred, Ending::CrLf);
    assert!(!lines.final_newline);
}

#[test]
fn an_empty_text_has_no_lines_and_counts_as_ending_in_a_newline() {
    let lines = split("");
    assert!(lines.lines.is_empty());
    assert_eq!(lines.preferred, Ending::Lf);
    assert!(lines.final_newline);
    assert_eq!(join(&lines.lines, lines.preferred, lines.final_newline), "");
}

#[test]
fn a_lone_carriage_return_is_text() {
    let lines = split("a\rb\n");
    assert_eq!(lines.lines, vec![Line { text: "a\rb", ending: Some(Ending::Lf) }]);
}

#[test]
fn join_gives_new_lines_the_preferred_ending_and_keeps_a_missing_final_newline() {
    let lines = [
        Line { text: "a", ending: None },
        Line { text: "b", ending: Some(Ending::Lf) },
        Line { text: "c", ending: None },
    ];
    assert_eq!(join(&lines, Ending::CrLf, false), "a\r\nb\nc");
    assert_eq!(join(&lines, Ending::CrLf, true), "a\r\nb\nc\r\n");
}

proptest! {
    #[test]
    fn split_then_join_gives_the_text_back(text in "[ab\r\n]{0,40}") {
        let lines = split(&text);
        prop_assert_eq!(join(&lines.lines, lines.preferred, lines.final_newline), text);
    }
}
