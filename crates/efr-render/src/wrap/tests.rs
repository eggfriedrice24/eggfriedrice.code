use pretty_assertions::assert_eq;

use super::wrap;
use crate::style::{Line, Span, Style, line_text, line_width};
use crate::width::WidthMethod;

fn lines(spans: &[Span], width: usize) -> Vec<Line> {
    wrap(spans, width, WidthMethod::CodePoint)
}

fn texts(spans: &[Span], width: usize) -> Vec<String> {
    lines(spans, width).iter().map(|line| line_text(line)).collect()
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
    let lines = lines(&spans, 5);
    assert_eq!(lines.iter().map(|line| line_text(line)).collect::<Vec<_>>(), ["aa", "bbcc", "dd"]);
    assert_eq!(lines[1], [Span::new("bb", Style::PLAIN.bold()), Span::plain("cc")]);
}

#[test]
fn wide_characters_count_two_columns() {
    let lines = lines(&[Span::plain("\u{4e16}\u{754c}\u{4e16}\u{754c}")], 5);
    assert_eq!(lines.iter().map(|line| line_width(line)).collect::<Vec<_>>(), [4, 4]);
}

#[test]
fn links_stay_on_their_pieces() {
    let link = Some("https://x".to_owned());
    let spans = [Span::linked("one two", Style::PLAIN, link.clone())];
    let lines = lines(&spans, 3);
    assert_eq!(
        lines,
        [
            vec![Span::linked("one", Style::PLAIN, link.clone())],
            vec![Span::linked("two", Style::PLAIN, link)]
        ]
    );
}

/// A citation mark of web search goes to the next row with the word before it, so it
/// never starts a row apart from the words that it backs (TM-Web 10, row 2).
#[test]
fn a_citation_mark_stays_with_the_word_before_it() {
    let link = Some("https://x".to_owned());
    let spans = [
        Span::plain("on Steam Deck "),
        Span::linked("[3]", Style::PLAIN, link.clone()),
        Span::plain(". Your"),
    ];
    assert_eq!(texts(&spans, 13), ["on Steam", "Deck [3].", "Your"]);
    // Only a link whose text is `[`, digits, `]` sticks; plain brackets and other links
    // break as any word does.
    let plain = [Span::plain("on Steam Deck [3]. Your")];
    assert_eq!(texts(&plain, 13), ["on Steam Deck", "[3]. Your"]);
    let other = [Span::plain("on Steam Deck "), Span::linked("[a]", Style::PLAIN, link)];
    assert_eq!(texts(&other, 13), ["on Steam Deck", "[a]"]);
}

#[test]
fn empty_input_is_one_empty_line() {
    assert_eq!(lines(&[], 10), [Vec::<Span>::new()]);
}

#[test]
fn zero_width_still_makes_progress() {
    assert_eq!(texts(&[Span::plain("ab")], 0), ["a", "b"]);
}

#[test]
fn widths_count_as_the_terminal_counts_them() {
    // A family emoji: three wide emoji joined into one cluster of two columns on a
    // terminal that counts by grapheme cluster, six on one that counts by code point.
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    let spans = [Span::plain(format!("ab {family} cd"))];
    let by_cluster: Vec<String> =
        wrap(&spans, 8, WidthMethod::Grapheme).iter().map(|line| line_text(line)).collect();
    assert_eq!(by_cluster, [format!("ab {family} cd")]);
    let by_code_point: Vec<String> =
        wrap(&spans, 8, WidthMethod::CodePoint).iter().map(|line| line_text(line)).collect();
    assert_eq!(by_code_point, ["ab", family, "cd"]);
}

#[test]
fn a_word_wider_than_the_line_splits_between_clusters() {
    // A flag is two regional indicators, one cluster of two columns.
    let flag = "\u{1f1e9}\u{1f1ea}";
    let spans = [Span::plain(format!("{flag}{flag}{flag}"))];
    let rows: Vec<String> =
        wrap(&spans, 4, WidthMethod::Grapheme).iter().map(|line| line_text(line)).collect();
    assert_eq!(rows, [format!("{flag}{flag}"), flag.to_owned()]);
}
