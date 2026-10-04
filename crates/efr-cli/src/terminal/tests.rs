use efr_render::{ColourMode, Theme};
use pretty_assertions::assert_eq;

use super::{TermFacts, at_width};
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

#[test]
fn render_options_carry_every_fact() {
    let theme = Theme::from_name("nord").unwrap();
    let facts = TermFacts { no_color: true, ..terminal_facts() };
    let options = facts.render_options(120, theme);
    assert_eq!(options.width(), 120);
    assert_eq!(options.colour(), ColourMode::None);
    assert_eq!(options.theme(), theme);
    assert!(options.is_terminal());
    assert!(options.hyperlinks());

    let dumb = TermFacts { term: Some("dumb".to_owned()), ..terminal_facts() };
    assert!(!dumb.render_options(80, Theme::ANSI).is_terminal());
}

#[test]
fn at_width_changes_only_the_width() {
    let theme = Theme::from_name("dracula").unwrap();
    let options = with_colorterm("truecolor").render_options(100, theme).with_hyperlinks(false);
    let narrow = at_width(&options, 40);
    assert_eq!(narrow.width(), 40);
    assert_eq!(
        (narrow.colour(), narrow.theme(), narrow.hyperlinks(), narrow.is_terminal()),
        (ColourMode::TrueColor, theme, false, true)
    );
    assert_eq!(at_width(&narrow, 100), options);
}
