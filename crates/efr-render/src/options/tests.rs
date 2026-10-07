use std::collections::BTreeSet;

use pretty_assertions::assert_eq;

use super::{ColourMode, RenderOptions, Theme};
use crate::elements::SAMPLE_TMTHEME;
use crate::{CodeTheme, Colour, Palette, RenderError, Role, WidthMethod};

#[test]
fn defaults_are_sixteen_colours_ansi_theme_hyperlinks_and_a_terminal() {
    let options = RenderOptions::new(100);
    assert_eq!(options.width(), 100);
    assert_eq!(options.colour(), ColourMode::Ansi16);
    assert_eq!(options.theme(), Theme::ANSI);
    assert!(options.hyperlinks());
    assert!(options.is_terminal());
}

#[test]
fn zero_width_renders_at_eighty_columns() {
    assert_eq!(RenderOptions::new(0).width(), 80);
}

#[test]
fn builders_set_each_field() {
    let theme = Theme::from_name("nord").unwrap();
    let palette = Palette::new().with(Role::Accent, Colour::Palette(5));
    let code_theme = CodeTheme::from_tmtheme(SAMPLE_TMTHEME).unwrap();
    let options = RenderOptions::new(40)
        .with_colour(ColourMode::None)
        .with_palette(palette.clone())
        .with_theme(theme)
        .with_code_theme(Some(code_theme.clone()))
        .with_hyperlinks(false)
        .with_width_method(WidthMethod::Grapheme)
        .with_terminal(false);
    assert_eq!(options.colour(), ColourMode::None);
    assert_eq!(options.palette(), &palette);
    assert_eq!(options.theme(), theme);
    assert_eq!(options.code_theme(), Some(&code_theme));
    assert!(!options.hyperlinks());
    assert_eq!(options.width_method(), WidthMethod::Grapheme);
    assert!(!options.is_terminal());
    assert_eq!(options.clone().with_code_theme(None).code_theme(), None);
}

#[test]
fn a_new_width_keeps_every_other_option() {
    let options = RenderOptions::new(40)
        .with_colour(ColourMode::TrueColor)
        .with_palette(Palette::new().with(Role::Muted, Colour::Palette(8)))
        .with_width_method(WidthMethod::Grapheme);
    let wider = options.clone().with_width(120);
    assert_eq!(wider.width(), 120);
    assert_eq!(wider.with_width(40), options);
}

#[test]
fn the_defaults_have_the_default_palette_and_count_by_code_point() {
    let options = RenderOptions::new(80);
    assert_eq!(options.palette(), &Palette::new());
    assert_eq!(options.code_theme(), None);
    assert_eq!(options.width_method(), WidthMethod::CodePoint);
}

#[test]
fn theme_names_ignore_case_spaces_and_punctuation() {
    let mocha = Theme::from_name("catppuccin-mocha").unwrap();
    assert_eq!(Theme::from_name("Catppuccin Mocha").unwrap(), mocha);
    assert_eq!(Theme::from_name("CATPPUCCIN_MOCHA").unwrap(), mocha);
    assert_eq!(
        Theme::from_name("Solarized (dark)").unwrap(),
        Theme::from_name("solarized-dark").unwrap()
    );
    assert_eq!(Theme::from_name("DarkNeon").unwrap().name(), "dark-neon");
}

#[test]
fn every_canonical_name_round_trips() {
    for name in Theme::names() {
        let theme: Theme = name.parse().unwrap();
        assert_eq!(theme.name(), name);
        assert_eq!(theme.to_string(), name);
    }
}

#[test]
fn canonical_names_are_unique_and_match_two_face() {
    let names: BTreeSet<&str> = Theme::names().collect();
    assert_eq!(names.len(), Theme::names().count());
    assert_eq!(names.len(), two_face::theme::EmbeddedLazyThemeSet::theme_names().len());
}

#[test]
fn unknown_theme_is_an_error_naming_it() {
    let error = Theme::from_name("no-such-theme").unwrap_err();
    assert!(matches!(&error, RenderError::UnknownTheme { name } if name == "no-such-theme"));
    assert_eq!(error.to_string(), "there is no theme named \"no-such-theme\"");
}

#[test]
fn only_the_palette_themes_use_the_palette() {
    let palette: Vec<&str> =
        Theme::names().filter(|name| Theme::from_name(name).unwrap().uses_palette()).collect();
    assert_eq!(palette, ["ansi", "base16", "base16-256"]);
}
