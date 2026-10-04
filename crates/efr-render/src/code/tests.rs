use pretty_assertions::assert_eq;

use super::{CodeBlock, CodeStyle, source_lines};
use crate::highlight::{ASSETS, Assets};
use crate::options::{ColourMode, Theme};
use crate::style::{GREEN, Span, Style, line_text};

fn style(colour: ColourMode) -> CodeStyle {
    CodeStyle { colour, theme: Theme::ANSI, assets: &ASSETS }
}

fn fresh(colour: ColourMode) -> (CodeStyle, &'static Assets) {
    let assets: &'static Assets = Box::leak(Box::new(Assets::new()));
    (CodeStyle { colour, theme: Theme::ANSI, assets }, assets)
}

#[test]
fn the_label_is_the_first_word_of_the_info_string_once() {
    let mut block = CodeBlock::new("rust ignore", style(ColourMode::Ansi16));
    assert_eq!(block.start(), Some(vec![Span::new("rust", Style::PLAIN.dim())]));
    assert_eq!(block.start(), None);
    let mut block = CodeBlock::new("rust,no_run", style(ColourMode::Ansi16));
    assert_eq!(block.start(), Some(vec![Span::new("rust", Style::PLAIN.dim())]));
}

#[test]
fn a_block_without_a_label_has_no_label_line() {
    let mut block = CodeBlock::new("", style(ColourMode::Ansi16));
    assert_eq!(block.start(), None);
    assert_eq!(block.line("plain"), [Span::plain("plain")]);
}

#[test]
fn known_languages_are_highlighted() {
    let mut block = CodeBlock::new("rust", style(ColourMode::Ansi16));
    let line = block.line("let x = 1;");
    assert_eq!(line_text(&line), "let x = 1;");
    assert!(line.iter().any(|span| span.style.fg.is_some()));
}

#[test]
fn unknown_languages_are_plain() {
    let mut block = CodeBlock::new("klingon", style(ColourMode::Ansi16));
    assert_eq!(block.line("qapla'"), [Span::plain("qapla'")]);
}

#[test]
fn tabs_expand_and_control_characters_are_made_visible() {
    let mut block = CodeBlock::new("", style(ColourMode::Ansi16));
    assert_eq!(block.line("\tx\x1b[2J\r"), [Span::plain("    x\u{241b}[2J")]);
}

#[test]
fn diff_labels_make_a_diff() {
    let mut block = CodeBlock::new("diff", style(ColourMode::Ansi16));
    assert_eq!(block.line("+a")[0], Span::new("+", Style::fg(GREEN).bold()));
}

#[test]
fn an_unlabelled_block_starting_with_a_git_diff_header_is_a_diff() {
    let mut block = CodeBlock::new("", style(ColourMode::Ansi16));
    assert_eq!(
        block.line("diff --git a/x b/x"),
        [Span::new("diff --git a/x b/x", Style::PLAIN.bold())]
    );
    assert_eq!(block.line("+a")[0], Span::new("+", Style::fg(GREEN).bold()));

    let mut block = CodeBlock::new("", style(ColourMode::Ansi16));
    block.line("not a diff");
    assert_eq!(block.line("+a"), [Span::plain("+a")]);
}

#[test]
fn grammars_load_only_for_a_labelled_block_with_colour() {
    let (code_style, assets) = fresh(ColourMode::Ansi16);
    let mut block = CodeBlock::new("", code_style);
    block.line("plain text");
    assert!(!assets.grammars_loaded());
    CodeBlock::new("rust", code_style);
    assert!(assets.grammars_loaded());

    let (no_colour, assets) = fresh(ColourMode::None);
    let mut block = CodeBlock::new("rust", no_colour);
    assert_eq!(block.line("fn x() {}"), [Span::plain("fn x() {}")]);
    assert!(!assets.grammars_loaded());
}

#[test]
fn source_lines_keep_an_unterminated_last_line() {
    assert_eq!(source_lines("a\nb\n"), ["a", "b"]);
    assert_eq!(source_lines("a\nb"), ["a", "b"]);
    assert_eq!(source_lines("\n\n"), ["", ""]);
    assert_eq!(source_lines(""), Vec::<&str>::new());
}
