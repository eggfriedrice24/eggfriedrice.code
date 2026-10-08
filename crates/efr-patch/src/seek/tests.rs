use pretty_assertions::assert_eq;

use super::{Found, find, nearest};
use crate::NearLine;

fn near(lines: &[(usize, &str)]) -> Vec<NearLine> {
    lines.iter().map(|&(number, text)| NearLine { number, text: text.to_owned() }).collect()
}

#[test]
fn an_exact_run_is_found_once() {
    assert_eq!(find(&["a", "b", "c"], &["b", "c"], 0, false), Found::Once(1));
}

#[test]
fn a_run_before_the_start_is_not_seen() {
    assert_eq!(find(&["a", "b", "a"], &["a"], 1, false), Found::Once(2));
    assert_eq!(find(&["a", "b"], &["a"], 2, false), Found::Nowhere);
}

#[test]
fn a_run_that_occurs_twice_is_many() {
    assert_eq!(find(&["x", "y", "x", "y"], &["x", "y"], 0, false), Found::Many(vec![0, 2]));
}

#[test]
fn an_exact_match_wins_over_a_tolerant_one() {
    // The second place matches only when trailing whitespace is ignored.
    assert_eq!(find(&["a", "a  "], &["a"], 0, false), Found::Once(0));
}

#[test]
fn trailing_whitespace_is_the_first_tolerance() {
    assert_eq!(find(&["foo  ", "bar\t"], &["foo", "bar"], 0, false), Found::Once(0));
}

#[test]
fn whitespace_at_both_ends_is_the_second_tolerance() {
    assert_eq!(find(&["  foo", "\tbar "], &["foo", "bar"], 0, false), Found::Once(0));
}

#[test]
fn unicode_punctuation_is_the_last_tolerance() {
    let file = ["say \u{201C}hi\u{201D} \u{2013} it\u{2019}s\u{00A0}fine"];
    assert_eq!(find(&file, &["say \"hi\" - it's fine"], 0, false), Found::Once(0));
}

#[test]
fn at_end_only_looks_at_the_last_place() {
    let file = ["x", "end", "x", "end"];
    assert_eq!(find(&file, &["x", "end"], 0, true), Found::Once(2));
    assert_eq!(find(&file, &["x"], 0, true), Found::Nowhere);
}

#[test]
fn a_pattern_longer_than_the_file_matches_nowhere() {
    assert_eq!(find(&["one"], &["one", "two"], 0, false), Found::Nowhere);
    assert_eq!(find(&[], &["one"], 0, true), Found::Nowhere);
}

#[test]
fn nearest_shows_the_place_with_the_most_shared_lines_and_one_line_around() {
    let file = ["fn a() {", "    one();", "    two();", "}", "fn b() {", "    three();", "}"];
    let pattern = ["    one();", "    TWO();", "}"];
    assert_eq!(
        nearest(&file, &pattern),
        near(&[(1, "fn a() {"), (2, "    one();"), (3, "    two();"), (4, "}"), (5, "fn b() {")])
    );
}

#[test]
fn nearest_falls_back_to_the_most_alike_line() {
    let file = ["alpha", "let total = count + 1;", "omega"];
    assert_eq!(
        nearest(&file, &["let totals = counts + 2;"]),
        near(&[(1, "alpha"), (2, "let total = count + 1;"), (3, "omega")])
    );
}

#[test]
fn nearest_is_empty_when_nothing_is_alike() {
    assert_eq!(nearest(&["aaaa"], &["zzzz"]), Vec::new());
    assert_eq!(nearest(&[], &["zzzz"]), Vec::new());
    assert_eq!(nearest(&["a"], &["", "  "]), Vec::new());
}

#[test]
fn nearest_shows_at_most_twelve_lines() {
    let file: Vec<String> = (0..40).map(|n| format!("line {n}")).collect();
    let file: Vec<&str> = file.iter().map(String::as_str).collect();
    let near = nearest(&file, &file[5..30]);
    assert_eq!(near.len(), 12);
    assert_eq!(near[0].number, 5);
}
