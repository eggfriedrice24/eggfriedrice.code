use pretty_assertions::assert_eq;
use syntect::highlighting::Color;

use super::{ASSETS, Assets, Highlight, theme_background, theme_colour};
use crate::options::Theme;
use crate::style::Colour;

fn fresh_assets() -> &'static Assets {
    Box::leak(Box::new(Assets::new()))
}

#[test]
fn nothing_loads_until_asked() {
    let assets = fresh_assets();
    assert!(!assets.grammars_loaded());
    assert!(assets.syntax_for_label("rust").is_some());
    assert!(assets.grammars_loaded());
}

#[test]
fn labels_find_grammars_by_name_extension_and_alias() {
    let name = |label: &str| ASSETS.syntax_for_label(label).map(|syntax| syntax.name.clone());
    assert_eq!(name("rust").as_deref(), Some("Rust"));
    assert_eq!(name("RS").as_deref(), Some("Rust"));
    assert_eq!(name("py").as_deref(), Some("Python"));
    assert_eq!(name("toml").as_deref(), Some("TOML"));
    assert_eq!(name("typescript").as_deref(), Some("TypeScript"));
    assert_eq!(name("shell").as_deref(), Some("Bourne Again Shell (bash)"));
    assert_eq!(name("zsh").as_deref(), Some("Bourne Again Shell (bash)"));
    assert_eq!(name("docker").as_deref(), Some("Dockerfile"));
}

#[test]
fn unknown_and_plain_text_labels_have_no_grammar() {
    assert!(ASSETS.syntax_for_label("no-such-language").is_none());
    assert!(ASSETS.syntax_for_label("txt").is_none());
}

#[test]
fn paths_find_grammars_without_opening_files() {
    let name = |path: &str| ASSETS.syntax_for_path(path).map(|syntax| syntax.name.clone());
    assert_eq!(name("src/main.rs").as_deref(), Some("Rust"));
    assert_eq!(name("a/b/Makefile").as_deref(), Some("Makefile"));
    assert_eq!(name("/does/not/exist.py").as_deref(), Some("Python"));
    assert_eq!(name("notes"), None);
}

#[test]
fn a_line_highlights_into_tokens_that_rebuild_it() {
    let syntax = ASSETS.syntax_for_label("rust").unwrap();
    let mut highlight = Highlight::new(&ASSETS, syntax, Theme::ANSI);
    let tokens = highlight.line("fn main() {}").unwrap();
    let text: String = tokens.iter().map(|(_, piece)| piece.as_str()).collect();
    assert_eq!(text, "fn main() {}");
    assert!(tokens.iter().any(|(style, _)| style.fg.is_some()));
}

#[test]
fn state_carries_across_lines() {
    let syntax = ASSETS.syntax_for_label("rust").unwrap();
    let mut highlight = Highlight::new(&ASSETS, syntax, Theme::ANSI);
    let comment_style = highlight.line("/* open").unwrap()[0].0;
    let inside = highlight.line("still a comment").unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].0, comment_style);
}

#[test]
fn bat_colour_encoding() {
    assert_eq!(theme_colour(Color { r: 3, g: 0, b: 0, a: 0 }), Some(Colour::Palette(3)));
    assert_eq!(theme_colour(Color { r: 0, g: 0, b: 0, a: 1 }), None);
    assert_eq!(theme_colour(Color { r: 1, g: 2, b: 3, a: 255 }), Some(Colour::Rgb(1, 2, 3)));
}

#[test]
fn only_truecolor_themes_have_a_background_to_tint() {
    assert_eq!(theme_background(ASSETS.theme(Theme::ANSI)), None);
    let mocha = Theme::from_name("catppuccin-mocha").unwrap();
    assert!(theme_background(ASSETS.theme(mocha)).is_some());
}
