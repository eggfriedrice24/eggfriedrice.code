use pretty_assertions::assert_eq;

use super::{CodeBlock, CodeStyle, Info, source_lines};
use crate::highlight::{ASSETS, Assets};
use crate::options::{ColourMode, RenderOptions};
use crate::style::{GREEN, Span, Style, line_text};

fn style(colour: ColourMode) -> CodeStyle {
    CodeStyle::new(&RenderOptions::new(80).with_colour(colour), &ASSETS)
}

fn fresh(colour: ColourMode) -> (CodeStyle, &'static Assets) {
    let assets: &'static Assets = Box::leak(Box::new(Assets::new()));
    (CodeStyle::new(&RenderOptions::new(80).with_colour(colour), assets), assets)
}

fn label(info: &str) -> Option<String> {
    CodeBlock::new(info, &style(ColourMode::Ansi16)).start().map(|line| line_text(&line))
}

#[test]
fn the_label_is_the_language_once_and_muted() {
    let mut block = CodeBlock::new("rust ignore", &style(ColourMode::Ansi16));
    assert_eq!(block.start(), Some(vec![Span::new("rust", Style::PLAIN.dim())]));
    assert_eq!(block.start(), None);
    assert_eq!(label("rust,no_run").as_deref(), Some("rust"));
    assert_eq!(label("sh").as_deref(), Some("sh"));
}

#[test]
fn a_short_fence_word_names_its_language() {
    assert_eq!(label("ts").as_deref(), Some("typescript"));
    assert_eq!(label("TS").as_deref(), Some("typescript"));
    assert_eq!(label("py app.py").as_deref(), Some("python \u{b7} app.py"));
    assert_eq!(label("Rust").as_deref(), Some("Rust"));
    let ascii = RenderOptions::new(80).with_glyphs(&crate::Glyphs::ASCII);
    let mut block = CodeBlock::new("rs a.rs", &CodeStyle::new(&ascii, &ASSETS));
    assert_eq!(block.start().map(|line| line_text(&line)).as_deref(), Some("rust - a.rs"));
}

#[test]
fn without_colour_the_fences_come_back_dim() {
    // TM-Code 3: the word of the fence as typed, no file.
    let none = style(ColourMode::None);
    let dim = Style::PLAIN.dim();
    for (info, fence) in
        [("rust src/a.rs", "```rust"), ("ts", "```ts"), ("", "```"), ("diff", "```diff")]
    {
        let mut block = CodeBlock::new(info, &none);
        assert_eq!(block.start(), Some(vec![Span::new(fence, dim)]), "{info:?}");
        assert_eq!(block.end(), Some(vec![Span::new("```", dim)]), "{info:?}");
    }
    assert_eq!(CodeBlock::new("rust", &style(ColourMode::Ansi16)).end(), None);
}

#[test]
fn without_colour_a_comment_is_italic() {
    let mut block = CodeBlock::new("rust", &style(ColourMode::None));
    assert_eq!(
        block.line("let x = 1; // one"),
        [Span::plain("let x = 1; "), Span::new("// one", Style::PLAIN.italic())]
    );
}

#[test]
fn a_row_of_code_is_on_code_bg_in_truecolor_only() {
    let background = crate::Colour::Rgb(0x12, 0x17, 0x22);
    let palette = crate::Palette::new().with(crate::Role::CodeBg, background);
    let options = RenderOptions::new(80).with_palette(palette);
    for (colour, fill) in [
        (ColourMode::TrueColor, Some(background)),
        (ColourMode::Ansi16, None),
        (ColourMode::None, None),
    ] {
        let style = CodeStyle::new(&options.clone().with_colour(colour), &ASSETS);
        let mut block = CodeBlock::new("", &style);
        block.line("plain");
        assert_eq!(block.fill(), fill, "{colour:?}");
    }
}

#[test]
fn plain_text_and_diffs_have_no_label() {
    for info in ["", "text", "txt", "plain", "plaintext", "TEXT", "diff", "patch", "udiff"] {
        assert_eq!(label(info), None, "{info:?}");
    }
}

#[test]
fn the_label_is_the_language_and_the_file() {
    assert_eq!(label("rust src/parse.rs").as_deref(), Some("rust \u{b7} src/parse.rs"));
    assert_eq!(
        label("rust title=\"src/my file.rs\"").as_deref(),
        Some("rust \u{b7} src/my file.rs")
    );
    assert_eq!(label("python {title='app.py'}").as_deref(), Some("python \u{b7} app.py"));
    assert_eq!(label("toml file=Cargo.toml").as_deref(), Some("toml \u{b7} Cargo.toml"));
    assert_eq!(label("title=\"main.rs\"").as_deref(), Some("main.rs"));
    // A diff has no label; plain text shows the file alone.
    assert_eq!(label("diff src/lib.rs"), None);
    assert_eq!(label("text notes.txt").as_deref(), Some("notes.txt"));
    // A second word without a slash or a dot is no file name.
    assert_eq!(label("rust ignore").as_deref(), Some("rust"));
    // A key that only ends in `title=` is not the title.
    assert_eq!(label("rust subtitle=x").as_deref(), Some("rust"));
}

#[test]
fn info_strings_split_into_language_and_file() {
    let info = |language: Option<&str>, file: Option<&str>| Info {
        language: language.map(str::to_owned),
        file: file.map(str::to_owned),
    };
    assert_eq!(Info::parse(""), info(None, None));
    assert_eq!(Info::parse("rust"), info(Some("rust"), None));
    assert_eq!(Info::parse("title=\"a.rs\""), info(None, Some("a.rs")));
    assert_eq!(Info::parse("rust a/b"), info(Some("rust"), Some("a/b")));
    assert_eq!(Info::parse("sh\x1b x.sh"), info(Some("sh\u{241b}"), Some("x.sh")));
}

#[test]
fn a_file_name_finds_the_grammar_when_the_language_does_not() {
    let mut block = CodeBlock::new("title=\"main.rs\"", &style(ColourMode::Ansi16));
    let line = block.line("let x = 1;");
    assert!(line.iter().any(|span| span.style.fg.is_some()));
    let mut block = CodeBlock::new("text main.rs", &style(ColourMode::Ansi16));
    assert_eq!(block.line("let x = 1;"), [Span::plain("let x = 1;")]);
}

#[test]
fn a_block_without_a_label_has_no_label_line() {
    let mut block = CodeBlock::new("", &style(ColourMode::Ansi16));
    assert_eq!(block.start(), None);
    assert_eq!(block.line("plain"), [Span::plain("plain")]);
}

#[test]
fn known_languages_are_highlighted() {
    let mut block = CodeBlock::new("rust", &style(ColourMode::Ansi16));
    let line = block.line("let x = 1;");
    assert_eq!(line_text(&line), "let x = 1;");
    assert!(line.iter().any(|span| span.style.fg.is_some()));
}

#[test]
fn unknown_languages_are_plain() {
    let mut block = CodeBlock::new("klingon", &style(ColourMode::Ansi16));
    assert_eq!(block.line("qapla'"), [Span::plain("qapla'")]);
}

#[test]
fn tabs_expand_and_control_characters_are_made_visible() {
    let mut block = CodeBlock::new("", &style(ColourMode::Ansi16));
    assert_eq!(block.line("\tx\x1b[2J\r"), [Span::plain("    x\u{241b}[2J")]);
}

#[test]
fn diff_labels_make_a_diff() {
    let mut block = CodeBlock::new("diff", &style(ColourMode::Ansi16));
    assert_eq!(block.line("+a")[0], Span::new("+", Style::fg(GREEN).bold()));
}

#[test]
fn an_unlabelled_block_starting_with_a_git_diff_header_is_a_diff() {
    let mut block = CodeBlock::new("", &style(ColourMode::Ansi16));
    assert_eq!(
        block.line("diff --git a/x b/x"),
        [Span::new("diff --git a/x b/x", Style::PLAIN.bold())]
    );
    assert_eq!(block.line("+a")[0], Span::new("+", Style::fg(GREEN).bold()));

    let mut block = CodeBlock::new("", &style(ColourMode::Ansi16));
    block.line("not a diff");
    assert_eq!(block.line("+a"), [Span::plain("+a")]);
}

#[test]
fn grammars_load_only_for_a_labelled_block() {
    let (code_style, assets) = fresh(ColourMode::Ansi16);
    let mut block = CodeBlock::new("", &code_style);
    block.line("plain text");
    assert!(!assets.grammars_loaded());
    CodeBlock::new("text", &code_style);
    assert!(!assets.grammars_loaded());
    CodeBlock::new("rust", &code_style);
    assert!(assets.grammars_loaded());

    // Without colour the grammar finds the comments, which stay italic.
    let (no_colour, assets) = fresh(ColourMode::None);
    let mut block = CodeBlock::new("rust", &no_colour);
    assert_eq!(block.line("fn x() {}"), [Span::plain("fn x() {}")]);
    assert!(assets.grammars_loaded());
    assert!(!assets.highlighter_loaded(crate::Theme::ANSI));
}

#[test]
fn source_lines_keep_an_unterminated_last_line() {
    assert_eq!(source_lines("a\nb\n"), ["a", "b"]);
    assert_eq!(source_lines("a\nb"), ["a", "b"]);
    assert_eq!(source_lines("\n\n"), ["", ""]);
    assert_eq!(source_lines(""), Vec::<&str>::new());
}
