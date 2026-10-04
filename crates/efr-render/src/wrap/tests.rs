use pretty_assertions::assert_eq;

use super::wrap;
use crate::style::{Span, Style, line_text, line_width};

fn texts(spans: &[Span], width: usize) -> Vec<String> {
    wrap(spans, width).iter().map(|line| line_text(line)).collect()
}

#[test]
fn short_text_stays_on_one_line() {
    assert_eq!(texts(&[Span::plain("one two")], 20), ["one two"]);
}

#[test]
fn breaks_at_spaces_and_drops_them() {
    assert_eq!(
        texts(&[Span::plain("the quick brown fox jumps")], 10),
        ["the quick", "brown fox", "jumps"]
    );
}

#[test]
fn a_word_wider_than_the_line_is_split() {
    assert_eq!(texts(&[Span::plain("abcdefghij xy")], 4), ["abcd", "efgh", "ij", "xy"]);
}

#[test]
fn words_span_styles_without_breaking_inside() {
    let spans = [Span::plain("aa "), Span::new("bb", Style::PLAIN.bold()), Span::plain("cc dd")];
    let lines = wrap(&spans, 5);
    assert_eq!(lines.iter().map(|line| line_text(line)).collect::<Vec<_>>(), ["aa", "bbcc", "dd"]);
    assert_eq!(lines[1], [Span::new("bb", Style::PLAIN.bold()), Span::plain("cc")]);
}

#[test]
fn wide_characters_count_two_columns() {
    let lines = wrap(&[Span::plain("\u{4e16}\u{754c}\u{4e16}\u{754c}")], 5);
    assert_eq!(lines.iter().map(|line| line_width(line)).collect::<Vec<_>>(), [4, 4]);
}

#[test]
fn links_stay_on_their_pieces() {
    let link = Some("https://x".to_owned());
    let spans = [Span::linked("one two", Style::PLAIN, link.clone())];
    let lines = wrap(&spans, 3);
    assert_eq!(
        lines,
        [
            vec![Span::linked("one", Style::PLAIN, link.clone())],
            vec![Span::linked("two", Style::PLAIN, link)]
        ]
    );
}

#[test]
fn empty_input_is_one_empty_line() {
    assert_eq!(wrap(&[], 10), [Vec::<Span>::new()]);
}

#[test]
fn zero_width_still_makes_progress() {
    assert_eq!(texts(&[Span::plain("ab")], 0), ["a", "b"]);
}
