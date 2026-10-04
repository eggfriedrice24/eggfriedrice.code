use pretty_assertions::assert_eq;

use super::{CodeText, Kind, line_start, outline};

fn kinds(text: &str) -> Vec<Kind> {
    outline(text).iter().map(|block| block.kind).collect()
}

fn last_closed(text: &str) -> bool {
    outline(text).last().unwrap().is_closed(text)
}

fn cut(text: &str) -> Option<&str> {
    outline(text).last().unwrap().paragraph_cut(text).map(|offset| &text[offset..])
}

#[test]
fn top_level_blocks_in_order() {
    use Kind::{Code, Heading, List, Paragraph, Quote, Rule, Table};
    assert_eq!(
        kinds("# h\n\npara\n\n- a\n- b\n\n> q\n\n---\n\n```rs\nx\n```\n\n| a |\n|---|\n"),
        [Heading, Paragraph, List, Quote, Rule, Code, Table]
    );
}

#[test]
fn list_items_and_code_text_are_recorded() {
    let text = "- a\n  - nested\n- b\n";
    assert_eq!(outline(text)[0].items, [0, 15]);
    let text = "```rust title\nfn x() {}\n\nlet y;\n";
    assert_eq!(
        outline(text)[0].code,
        Some(CodeText {
            info: "rust title".to_owned(),
            text: "fn x() {}\n\nlet y;\n".to_owned(),
            fenced: true
        })
    );
}

#[test]
fn headings_and_rules_close_on_their_own_line() {
    assert!(last_closed("# title\n"));
    assert!(last_closed("title\n===\n"));
    assert!(last_closed("***\n"));
}

#[test]
fn paragraphs_tables_and_quotes_close_at_a_blank_line() {
    assert!(!last_closed("para\n"));
    assert!(last_closed("para\n\n"));
    assert!(!last_closed("| a |\n|---|\n| 1 |\n"));
    assert!(last_closed("| a |\n|---|\n| 1 |\n\n"));
    assert!(!last_closed("> q\n"));
    assert!(last_closed("> q\n\n"));
}

#[test]
fn lists_and_code_blocks_stay_open_across_blank_lines() {
    assert!(!last_closed("- a\n\n"));
    assert!(!last_closed("```\ncode\n\n"));
    assert!(!last_closed("    code\n\n"));
}

#[test]
fn short_paragraphs_stay_whole() {
    assert_eq!(cut("one\ntwo\nthree\n"), None);
}

#[test]
fn a_long_paragraph_commits_all_but_its_last_lines() {
    assert_eq!(cut("one\ntwo\nthree\nfour\n"), Some("four\n"));
    assert_eq!(cut("one\ntwo\nthree\nfour\nfive\n"), Some("five\n"));
}

#[test]
fn the_cut_is_before_a_line_that_starts_a_paragraph_on_its_own() {
    assert_eq!(cut("one\ntwo\nthree\nfour\n| a | b |\n"), Some("four\n| a | b |\n"));
    assert_eq!(
        cut("one\ntwo\nthree\nfour\n2. not a list here\n"),
        Some("four\n2. not a list here\n")
    );
    assert_eq!(cut("one\n(two)\n*three*\n`four`\n[five]\n"), None);
    assert_eq!(cut("one\ntwo\nthree\n    four\n"), Some("three\n    four\n"));
}

#[test]
fn line_start_finds_the_start_of_the_line() {
    assert_eq!(line_start("ab\n    cd", 7), 3);
    assert_eq!(line_start("abc", 2), 0);
    assert_eq!(line_start("ab\n", 3), 3);
}
