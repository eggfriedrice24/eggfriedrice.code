use pretty_assertions::assert_eq;

use super::{Inline, InlineKind};
use crate::style::{BLUE, Span, Style, line_text};

fn texts(lines: &[Vec<Span>]) -> Vec<String> {
    lines.iter().map(|line| line_text(line)).collect()
}

#[test]
fn text_in_one_style_joins_into_one_span() {
    let mut inline = Inline::new(InlineKind::Paragraph);
    inline.push_text("a", Style::PLAIN, None, true);
    inline.push_text("b", Style::PLAIN, None, true);
    inline.push_text("c", Style::PLAIN.bold(), None, true);
    assert_eq!(inline.finish(), [vec![Span::plain("ab"), Span::new("c", Style::PLAIN.bold())]]);
}

#[test]
fn a_url_split_across_text_events_is_still_found() {
    let mut inline = Inline::new(InlineKind::Paragraph);
    inline.push_text("see https://example.com/a", Style::PLAIN, None, true);
    inline.push_text("_b.", Style::PLAIN, None, true);
    let link = Some("https://example.com/a_b".to_owned());
    assert_eq!(
        inline.finish(),
        [vec![
            Span::plain("see "),
            Span::linked("https://example.com/a_b", Style::fg(BLUE).underline(), link),
            Span::plain("."),
        ]]
    );
}

#[test]
fn text_inside_a_link_is_not_linked_again() {
    let mut inline = Inline::new(InlineKind::Paragraph);
    let link = Some("https://a.io".to_owned());
    inline.push_text("https://b.io", Style::PLAIN, link.clone(), false);
    assert_eq!(inline.finish(), [vec![Span::linked("https://b.io", Style::PLAIN, link)]]);
}

#[test]
fn only_paragraphs_break_lines() {
    assert!(Inline::new(InlineKind::Paragraph).breaks_lines());
    assert!(!Inline::new(InlineKind::Cell).breaks_lines());
    assert!(!Inline::new(InlineKind::Heading(Style::PLAIN.bold())).breaks_lines());
}

#[test]
fn breaks_make_lines() {
    let mut inline = Inline::new(InlineKind::Paragraph);
    inline.push_text("one", Style::PLAIN, None, true);
    inline.break_line();
    inline.push_text("two", Style::PLAIN, None, true);
    assert_eq!(texts(&inline.finish()), ["one", "two"]);
}

#[test]
fn text_since_a_position_on_the_same_line() {
    let mut inline = Inline::new(InlineKind::Paragraph);
    inline.push_text("before ", Style::PLAIN, None, true);
    let start = inline.position();
    inline.push_text("link", Style::PLAIN.underline(), None, false);
    assert_eq!(inline.text_since(start).as_deref(), Some("link"));
    inline.break_line();
    assert_eq!(inline.text_since(start), None);
}

#[test]
fn headings_carry_their_base_style() {
    let heading = Style::PLAIN.bold();
    assert_eq!(Inline::new(InlineKind::Heading(heading)).base_style(), heading);
    assert_eq!(Inline::new(InlineKind::Paragraph).base_style(), Style::PLAIN);
}

#[test]
fn control_characters_are_sanitized_when_flushed() {
    let mut inline = Inline::new(InlineKind::Paragraph);
    inline.push_text("a\u{7}b", Style::PLAIN, None, true);
    assert_eq!(texts(&inline.finish()), ["a\u{2407}b"]);
}
