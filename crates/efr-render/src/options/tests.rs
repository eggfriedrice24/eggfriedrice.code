use std::collections::BTreeSet;

use pretty_assertions::assert_eq;

use super::{ColourMode, RenderOptions, Theme};
use crate::RenderError;

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
    let options = RenderOptions::new(40)
        .with_colour(ColourMode::None)
        .with_theme(theme)
        .with_hyperlinks(false)
        .with_terminal(false);
    assert_eq!(options.colour(), ColourMode::None);
    assert_eq!(options.theme(), theme);
    assert!(!options.hyperlinks());
    assert!(!options.is_terminal());
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
