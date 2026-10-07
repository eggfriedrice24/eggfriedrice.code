use pretty_assertions::assert_eq;

use super::{Diff, Kind, hunk_counts};
use crate::code::CodeStyle;
use crate::code_theme::CodeTheme;
use crate::highlight::{ASSETS, Assets};
use crate::options::{ColourMode, RenderOptions, Theme};
use crate::style::{CYAN, Colour, GREEN, RED, Span, Style};

fn style(colour: ColourMode, theme: Theme) -> CodeStyle {
    CodeStyle::new(&RenderOptions::new(80).with_colour(colour).with_theme(theme), &ASSETS)
}

fn kinds(lines: &[&str]) -> Vec<Kind> {
    let mut diff = Diff::new(style(ColourMode::Ansi16, Theme::ANSI));
    lines.iter().map(|line| diff.classify(line)).collect()
}

#[test]
fn a_git_diff_classifies_line_by_line() {
    use Kind::{Added, Context, File, HunkHeader, Meta, Note, Removed};
    assert_eq!(
        kinds(&[
            "diff --git a/src/lib.rs b/src/lib.rs",
            "index 1111111..2222222 100644",
            "--- a/src/lib.rs",
            "+++ b/src/lib.rs",
            "@@ -1,3 +1,3 @@ fn main() {",
            " let a = 1;",
            "-let b = 2;",
            "+let b = 3;",
            "\\ No newline at end of file",
            " let c = 4;",
        ]),
        [Meta, Meta, File, File, HunkHeader, Context, Removed, Added, Note, Context]
    );
}

#[test]
fn counted_hunks_treat_dashes_and_pluses_inside_them_as_changes() {
    use Kind::{Added, File, HunkHeader, Removed};
    assert_eq!(
        kinds(&["@@ -1,1 +1,1 @@", "--- a/looks-like-a-header", "+++ b/also", "--- a/next.rs"]),
        [HunkHeader, Removed, Added, File]
    );
}

#[test]
fn uncounted_hunks_end_at_a_file_header() {
    use Kind::{Added, File, HunkHeader, Removed};
    assert_eq!(
        kinds(&["@@", "-old", "+new", "--- a/next.rs", "+++ b/next.rs", "@@", "+x"]),
        [HunkHeader, Removed, Added, File, File, HunkHeader, Added]
    );
}

#[test]
fn lines_outside_a_hunk_still_show_additions_and_removals() {
    use Kind::{Added, Other, Removed};
    assert_eq!(kinds(&["+added", "-removed", "plain"]), [Added, Removed, Other]);
}

#[test]
fn a_line_that_cannot_be_in_a_hunk_ends_it() {
    use Kind::{Added, HunkHeader, Other};
    assert_eq!(kinds(&["@@", "+a", "prose after", "+b"]), [HunkHeader, Added, Other, Added]);
}

#[test]
fn hunk_counts_default_to_one() {
    assert_eq!(hunk_counts("@@ -1,3 +1,4 @@ fn x()"), Some((3, 4)));
    assert_eq!(hunk_counts("@@ -5 +7 @@"), Some((1, 1)));
    assert_eq!(hunk_counts("@@ -0,0 +1,2 @@"), Some((0, 2)));
    assert_eq!(hunk_counts("@@"), None);
    assert_eq!(hunk_counts("@@ garbage @@"), None);
}

#[test]
fn without_a_known_language_changed_lines_are_wholly_green_or_red() {
    let mut diff = Diff::new(style(ColourMode::Ansi16, Theme::ANSI));
    assert_eq!(
        diff.line("+new"),
        [Span::new("+", Style::fg(GREEN).bold()), Span::new("new", Style::fg(GREEN))]
    );
    assert_eq!(
        diff.line("-old"),
        [Span::new("-", Style::fg(RED).bold()), Span::new("old", Style::fg(RED))]
    );
    assert_eq!(
        diff.line("@@ -1 +1 @@ ctx"),
        [Span::new("@@ -1 +1 @@", Style::fg(CYAN)), Span::plain(" ctx")]
    );
}

#[test]
fn with_a_known_language_changed_lines_carry_syntax_colours() {
    let mut diff = Diff::new(style(ColourMode::Ansi16, Theme::ANSI));
    diff.line("+++ b/src/main.rs");
    let line = diff.line("+fn main() {}");
    assert_eq!(line[0], Span::new("+", Style::fg(GREEN).bold()));
    let text: String = line.iter().map(|span| span.text.as_str()).collect();
    assert_eq!(text, "+fn main() {}");
    assert!(line[1..].iter().any(|span| span.style.fg.is_some() && span.style.fg != Some(GREEN)));
}

#[test]
fn truecolor_themes_tint_changed_lines_and_palette_themes_do_not() {
    let mocha = Theme::from_name("catppuccin-mocha").unwrap();
    let mut tinted = Diff::new(style(ColourMode::TrueColor, mocha));
    let line = tinted.line("+x");
    assert!(line.iter().all(|span| matches!(span.style.bg, Some(Colour::Rgb(..)))));

    let mut plain = Diff::new(style(ColourMode::TrueColor, Theme::ANSI));
    assert!(plain.line("+x").iter().all(|span| span.style.bg.is_none()));
    let mut sixteen = Diff::new(style(ColourMode::Ansi16, mocha));
    assert!(sixteen.line("+x").iter().all(|span| span.style.bg.is_none()));
}

#[test]
fn a_code_theme_from_a_file_tints_changed_lines_in_truecolor() {
    let theme = CodeTheme::from_tmtheme(crate::elements::SAMPLE_TMTHEME).unwrap();
    let options =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_code_theme(Some(theme));
    let mut diff = Diff::new(CodeStyle::new(&options, &ASSETS));
    let line = diff.line("+x");
    // A quarter of the green tint over the theme's background #1c1b19.
    assert!(line.iter().all(|span| span.style.bg == Some(Colour::Rgb(32, 60, 35))));
}

#[test]
fn the_diff_roles_of_the_palette_colour_the_lines() {
    let palette = crate::Palette::new()
        .with(crate::Role::DiffAdd, Colour::Palette(10))
        .with(crate::Role::DiffHunk, Colour::Palette(5));
    let options = RenderOptions::new(80).with_palette(palette);
    let mut diff = Diff::new(CodeStyle::new(&options, &ASSETS));
    assert_eq!(diff.line("+x")[0], Span::new("+", Style::fg(Colour::Palette(10)).bold()));
    assert_eq!(
        diff.line("@@ -1 +1 @@")[0],
        Span::new("@@ -1 +1 @@", Style::fg(Colour::Palette(5)))
    );
    assert_eq!(diff.line("-y")[0], Span::new("-", Style::fg(RED).bold()));
}

#[test]
fn no_colour_loads_no_grammar() {
    let assets: &'static Assets = Box::leak(Box::new(Assets::new()));
    let options = RenderOptions::new(80).with_colour(ColourMode::None);
    let mut diff = Diff::new(CodeStyle::new(&options, assets));
    diff.line("diff --git a/x.rs b/x.rs");
    diff.line("+++ b/x.rs");
    diff.line("+fn x() {}");
    assert!(!assets.grammars_loaded());
}
