use std::time::{Duration, Instant};

use efr_test_support::bench::Report;
use pretty_assertions::assert_eq;
use syntect::highlighting::{
    Color, HighlightIterator, HighlightState, Highlighter, Theme as SyntectTheme,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference};

use super::{ASSETS, Assets, Highlight, style_of, theme_background, theme_colour};
use crate::CodeTheme;
use crate::elements::SAMPLE_TMTHEME;
use crate::options::Theme;
use crate::style::{Colour, Style};

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
    let mut highlight = Highlight::new(&ASSETS, syntax, ASSETS.theme_ref(Theme::ANSI));
    let tokens = highlight.line("fn main() {}").unwrap();
    let text: String = tokens.iter().map(|(_, piece)| piece.as_str()).collect();
    assert_eq!(text, "fn main() {}");
    assert!(tokens.iter().any(|(style, _)| style.fg.is_some()));
}

#[test]
fn state_carries_across_lines() {
    let syntax = ASSETS.syntax_for_label("rust").unwrap();
    let mut highlight = Highlight::new(&ASSETS, syntax, ASSETS.theme_ref(Theme::ANSI));
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

/// Lines of Rust code, as a reply's code block has them.
const RUST: &[&str] = &[
    "/// Reads the config file and checks every key.",
    "pub fn load(path: &Path) -> Result<Config, Error> {",
    "    let text = std::fs::read_to_string(path).map_err(Error::Read)?;",
    "    let config: Config = toml::from_str(&text)?;",
    "    for (key, value) in &config.keys {",
    "        if value.is_empty() { return Err(Error::Empty(key.clone())); }",
    "    }",
    "    let count = config.keys.len() as u64 * 2 + 0x1f; // \"quoted\" text",
    "    Ok(config)",
    "}",
];

/// The highlight of `lines` as it was before the highlighters were shared: a new
/// highlighter of `theme` for the block and one more for each line.
fn highlight_with_fresh_highlighters(
    syntax: &SyntaxReference,
    theme: &SyntectTheme,
    lines: &[&str],
) -> Vec<Vec<(Style, String)>> {
    let mut parse = ParseState::new(syntax);
    let mut state = HighlightState::new(&Highlighter::new(theme), ScopeStack::new());
    let syntaxes = ASSETS.syntaxes();
    lines
        .iter()
        .map(|line| {
            let text = format!("{line}\n");
            let ops = parse.parse_line(&text, syntaxes).unwrap();
            let highlighter = Highlighter::new(theme);
            HighlightIterator::new(&mut state, &ops, &text, &highlighter)
                .map(|(style, piece)| (style_of(style), piece.strip_suffix('\n').unwrap_or(piece)))
                .filter(|(_, piece)| !piece.is_empty())
                .map(|(style, piece)| (style, piece.to_owned()))
                .collect()
        })
        .collect()
}

#[test]
fn the_shared_highlighter_gives_what_a_fresh_one_for_each_line_gives() {
    let code: Vec<&str> = RUST.iter().chain(RUST).copied().collect();
    let syntax = ASSETS.syntax_for_label("rust").unwrap();
    let custom = CodeTheme::from_tmtheme(SAMPLE_TMTHEME).unwrap();
    let mut themes: Vec<_> =
        Theme::names().map(|name| ASSETS.theme_ref(Theme::from_name(name).unwrap())).collect();
    themes.push(custom.theme_ref());
    for theme in themes {
        let mut highlight = Highlight::new(&ASSETS, syntax, theme);
        let shared: Vec<_> = code.iter().map(|line| highlight.line(line).unwrap()).collect();
        let fresh = highlight_with_fresh_highlighters(syntax, theme.get(), &code);
        assert_eq!(shared, fresh, "theme {:?}", theme.get().name);
    }
}

#[test]
fn blocks_of_one_theme_share_one_highlighter() {
    let mocha = Theme::from_name("catppuccin-mocha").unwrap();
    let first = ASSETS.theme_ref(mocha).highlighter;
    assert!(std::ptr::eq(first, ASSETS.theme_ref(mocha).highlighter));
    assert!(!std::ptr::eq(first, ASSETS.theme_ref(Theme::ANSI).highlighter));
    let custom = CodeTheme::from_tmtheme(SAMPLE_TMTHEME).unwrap();
    assert!(std::ptr::eq(custom.theme_ref().highlighter, custom.clone().theme_ref().highlighter));
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_highlight_line() {
    let mut report = Report::new("bench_perf_highlight_line");
    // The first highlight of a process decompresses the grammars and the themes.
    let assets = fresh_assets();
    let start = Instant::now();
    let syntax = assets.syntax_for_label("rust").unwrap();
    let mut highlight = Highlight::new(assets, syntax, assets.theme_ref(Theme::ANSI));
    assert!(highlight.line(RUST[1]).is_some());
    report.time("cold first highlight (syntax set load)", &[start.elapsed()]);

    let syntax = ASSETS.syntax_for_label("rust").unwrap();
    for name in ["ansi", "catppuccin-mocha"] {
        let theme = Theme::from_name(name).unwrap();
        let mut points = Vec::new();
        for lines in [1_000_u64, 10_000] {
            let mut highlight = Highlight::new(&ASSETS, syntax, ASSETS.theme_ref(theme));
            let mut per_line = Vec::new();
            let start = Instant::now();
            for at in 0..lines {
                let line = RUST[at as usize % RUST.len()];
                let one = Instant::now();
                let tokens = highlight.line(line).unwrap();
                per_line.push(one.elapsed());
                assert!(!tokens.is_empty());
            }
            let total: Duration = start.elapsed();
            report.time(&format!("{name} {lines} lines: per line"), &per_line);
            report.time(&format!("{name} {lines} lines: total"), &[total]);
            points.push((lines, total));
        }
        report.slope(&format!("{name} total slope over lines"), &points);
    }
    report.finish();
}
