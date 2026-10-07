use efr_render::{Colour, ColourMode, Palette, Role, Theme, WidthMethod};
use pretty_assertions::assert_eq;

use super::{Background, TermFacts, at_width};
use crate::settings::Settings;
use crate::testing::terminal_facts;

fn with_colorterm(value: &str) -> TermFacts {
    TermFacts { colorterm: Some(value.to_owned()), ..terminal_facts() }
}

#[test]
fn no_color_turns_colour_off_whatever_colorterm_says() {
    let facts = TermFacts { no_color: true, ..with_colorterm("truecolor") };
    assert_eq!(facts.colour(), ColourMode::None);
}

#[test]
fn colorterm_truecolor_or_24bit_selects_truecolor() {
    assert_eq!(with_colorterm("truecolor").colour(), ColourMode::TrueColor);
    assert_eq!(with_colorterm("24bit").colour(), ColourMode::TrueColor);
    assert_eq!(with_colorterm("TrueColor").colour(), ColourMode::TrueColor);
}

#[test]
fn anything_else_is_sixteen_colours() {
    assert_eq!(terminal_facts().colour(), ColourMode::Ansi16);
    assert_eq!(with_colorterm("yes").colour(), ColourMode::Ansi16);
}

#[test]
fn replies_are_formatted_only_on_a_terminal_that_is_not_dumb() {
    assert!(terminal_facts().formats_stdout());
    let piped = TermFacts { stdout_tty: false, ..terminal_facts() };
    assert!(!piped.formats_stdout());
    let dumb = TermFacts { term: Some("dumb".to_owned()), ..terminal_facts() };
    assert!(!dumb.formats_stdout());
}

/// Settings with `theme` and an accent of their own.
fn settings(theme: Theme) -> Settings {
    Settings {
        theme,
        palette: Palette::new().with(Role::Accent, Colour::Palette(5)),
        ..Settings::default()
    }
}

#[test]
fn render_options_carry_every_fact() {
    let theme = Theme::from_name("nord").unwrap();
    let facts = TermFacts { no_color: true, ..terminal_facts() };
    let options = facts.render_options(120, &settings(theme));
    assert_eq!(options.width(), 120);
    assert_eq!(options.colour(), ColourMode::None);
    assert_eq!(options.theme(), theme);
    assert_eq!(options.palette().get(Role::Accent), Some(Colour::Palette(5)));
    assert_eq!(options.width_method(), WidthMethod::CodePoint);
    assert!(options.is_terminal());
    assert!(options.hyperlinks());

    let dumb = TermFacts { term: Some("dumb".to_owned()), ..terminal_facts() };
    assert!(!dumb.render_options(80, &Settings::default()).is_terminal());
}

#[test]
fn ghostty_outside_tmux_counts_widths_by_grapheme_cluster() {
    let facts = |program: Option<&str>, tmux: bool| TermFacts {
        term_program: program.map(str::to_owned),
        tmux,
        ..terminal_facts()
    };
    let table = [
        (Some("ghostty"), false, WidthMethod::Grapheme),
        (Some("Ghostty"), false, WidthMethod::Grapheme),
        (Some("ghostty"), true, WidthMethod::CodePoint),
        (Some("tmux"), true, WidthMethod::CodePoint),
        (Some("kitty"), false, WidthMethod::CodePoint),
        (None, false, WidthMethod::CodePoint),
    ];
    for (program, tmux, method) in table {
        assert_eq!(facts(program, tmux).width_method(), method, "{program:?} tmux={tmux}");
        let options = facts(program, tmux).render_options(80, &Settings::default());
        assert_eq!(options.width_method(), method);
    }
}

#[test]
fn at_width_changes_only_the_width() {
    let theme = Theme::from_name("dracula").unwrap();
    let ghostty =
        TermFacts { term_program: Some("ghostty".to_owned()), ..with_colorterm("truecolor") };
    let options = ghostty.render_options(100, &settings(theme)).with_hyperlinks(false);
    let narrow = at_width(&options, 40);
    assert_eq!(narrow.width(), 40);
    assert_eq!(
        (narrow.colour(), narrow.theme(), narrow.hyperlinks(), narrow.is_terminal()),
        (ColourMode::TrueColor, theme, false, true)
    );
    assert_eq!(narrow.palette(), options.palette());
    assert_eq!(narrow.width_method(), WidthMethod::Grapheme);
    assert_eq!(at_width(&narrow, 100), options);
}

#[test]
fn the_background_is_dark_or_light_in_any_letter_case() {
    assert_eq!(Background::from_name("dark"), Ok(Background::Dark));
    assert_eq!(Background::from_name("Light"), Ok(Background::Light));
    assert_eq!(Background::from_name(" LIGHT "), Ok(Background::Light));
    assert_eq!(Background::from_name("grey"), Err("grey".to_owned()));
    assert_eq!(Background::Light.name(), "light");
}
