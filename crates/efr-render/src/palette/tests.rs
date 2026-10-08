use pretty_assertions::assert_eq;

use super::{Palette, Role};
use crate::RenderError;
use crate::options::{ColourMode, RenderOptions};
use crate::style::{Colour, Style};

#[test]
fn role_names_round_trip_and_are_unique() {
    let names: Vec<&str> = Role::ALL.iter().map(|role| role.name()).collect();
    assert_eq!(
        names,
        [
            "text",
            "muted",
            "accent",
            "heading",
            "link",
            "code",
            "success",
            "warning",
            "error",
            "quote",
            "diff.add",
            "diff.remove",
            "diff.hunk"
        ]
    );
    for role in Role::ALL {
        assert_eq!(Role::from_name(role.name()), Some(role));
        assert_eq!(role.to_string().parse::<Role>().unwrap(), role);
    }
    let error = "diff_add".parse::<Role>().unwrap_err();
    assert!(matches!(&error, RenderError::UnknownRole { name } if name == "diff_add"));
    assert_eq!(error.to_string(), "there is no colour role named \"diff_add\"");
}

#[test]
fn the_index_of_each_role_is_its_place_in_all() {
    for (index, role) in Role::ALL.into_iter().enumerate() {
        assert_eq!(role.index(), index);
    }
}

/// Every role's SGR parameters in one mode, as `name=parameters` lines.
fn table(palette: &Palette, mode: ColourMode) -> String {
    let options = RenderOptions::new(80).with_colour(mode).with_palette(palette.clone());
    Role::ALL
        .iter()
        .map(|role| format!("{role}={}", options.sgr(*role).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_default_palette_uses_the_sixteen_colours_and_yellow_as_accent() {
    let defaults = Palette::new();
    assert_eq!(
        table(&defaults, ColourMode::Ansi16),
        "text= muted=2 accent=33 heading=1;33 link=4;34 code=36 success=32 warning=1;33 \
         error=31 quote=3 diff.add=32 diff.remove=31 diff.hunk=36"
    );
    // Palette entries stay palette entries in truecolor, so they follow the theme.
    assert_eq!(table(&defaults, ColourMode::TrueColor), table(&defaults, ColourMode::Ansi16));
}

#[test]
fn without_colour_roles_keep_their_attributes_and_some_get_one_in_place() {
    assert_eq!(
        table(&Palette::new(), ColourMode::None),
        "text= muted=2 accent=1 heading=1 link=4 code= success= warning=1 error=1 quote=3 \
         diff.add= diff.remove= diff.hunk="
    );
}

#[test]
fn a_set_colour_replaces_the_default_and_keeps_the_attributes() {
    let palette = Palette::new()
        .with(Role::Muted, Colour::Palette(8))
        .with(Role::Warning, Colour::Palette(13))
        .with(Role::Text, Colour::Palette(15));
    assert_eq!(palette.get(Role::Muted), Some(Colour::Palette(8)));
    assert_eq!(palette.get(Role::Link), None);
    assert_eq!(palette.colour(Role::Link), Some(Colour::Palette(4)));
    // A muted role with a colour is not dim; without colour it is dim again.
    let ansi = RenderOptions::new(80).with_palette(palette.clone());
    assert_eq!(ansi.sgr(Role::Muted).as_deref(), Some("90"));
    assert_eq!(ansi.sgr(Role::Warning).as_deref(), Some("1;95"));
    assert_eq!(ansi.sgr(Role::Text).as_deref(), Some("97"));
    let none = ansi.with_colour(ColourMode::None);
    assert_eq!(none.sgr(Role::Muted).as_deref(), Some("2"));
    assert_eq!(none.sgr(Role::Text), None);
}

#[test]
fn hex_colours_are_rgb_in_truecolor_and_the_nearest_entry_in_sixteen() {
    let palette = Palette::new()
        .with(Role::Accent, Colour::Rgb(0xf2, 0xc1, 0x4e))
        .with(Role::Error, Colour::Rgb(0xfa, 0x3c, 0x3c));
    let truecolor =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette.clone());
    assert_eq!(truecolor.sgr(Role::Accent).as_deref(), Some("38;2;242;193;78"));
    assert_eq!(truecolor.sgr(Role::Heading).as_deref(), Some("1;38;2;242;193;78"));
    let sixteen = truecolor.clone().with_colour(ColourMode::Ansi16);
    assert_eq!(sixteen.sgr(Role::Accent).as_deref(), Some("33"));
    assert_eq!(sixteen.sgr(Role::Error).as_deref(), Some("91"));
    let none = truecolor.with_colour(ColourMode::None);
    assert_eq!(none.sgr(Role::Accent).as_deref(), Some("1"));
    assert_eq!(none.sgr(Role::Error).as_deref(), Some("1"));
}

#[test]
fn a_heading_follows_the_accent_until_it_has_its_own_colour() {
    let accent = Palette::new().with(Role::Accent, Colour::Palette(5));
    assert_eq!(accent.colour(Role::Heading), Some(Colour::Palette(5)));
    let own = accent.with(Role::Heading, Colour::Palette(6));
    assert_eq!(own.colour(Role::Heading), Some(Colour::Palette(6)));
    assert_eq!(own.colour(Role::Accent), Some(Colour::Palette(5)));
}

#[test]
fn paint_wraps_text_in_the_role_or_leaves_it() {
    let options = RenderOptions::new(80);
    assert_eq!(options.paint(Role::Warning, "allow?"), "\x1b[1;33mallow?\x1b[0m");
    assert_eq!(options.paint(Role::Muted, "done in 2s"), "\x1b[2mdone in 2s\x1b[0m");
    assert_eq!(options.paint(Role::Text, "plain"), "plain");
    assert_eq!(options.paint(Role::Accent, ""), "");
    let piped = RenderOptions::new(80).with_terminal(false);
    assert_eq!(piped.paint(Role::Warning, "allow?"), "allow?");
    assert_eq!(piped.sgr(Role::Warning), None);
}

#[test]
fn tint_paints_the_colour_of_a_role_without_its_attributes() {
    let options = RenderOptions::new(80);
    assert_eq!(options.tint(Role::Warning, "ctx 60%"), "\x1b[33mctx 60%\x1b[0m");
    assert_eq!(options.tint(Role::Success, "ctx 4%"), "\x1b[32mctx 4%\x1b[0m");
    assert_eq!(options.tint(Role::Error, "ctx 95%"), "\x1b[31mctx 95%\x1b[0m");
    let own = Palette::new().with(Role::Warning, Colour::Rgb(1, 2, 3));
    let truecolor = RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(own);
    assert_eq!(truecolor.tint(Role::Warning, "x"), "\x1b[38;2;1;2;3mx\x1b[0m");
    // Without colour, a role keeps what stands in for its colour, and nothing else.
    let none = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(none.tint(Role::Warning, "ctx 60%"), "ctx 60%");
    assert_eq!(none.tint(Role::Success, "ctx 4%"), "ctx 4%");
    assert_eq!(none.tint(Role::Error, "ctx 95%"), "\x1b[1mctx 95%\x1b[0m");
    assert_eq!(none.tint(Role::Muted, "dim"), "\x1b[2mdim\x1b[0m");
    let piped = RenderOptions::new(80).with_terminal(false);
    assert_eq!(piped.tint(Role::Error, "ctx 95%"), "ctx 95%");
}

#[test]
fn the_style_of_a_role_is_what_the_renderer_paints() {
    let palette = Palette::new().with(Role::Code, Colour::Rgb(1, 2, 3));
    assert_eq!(palette.style(Role::Code, ColourMode::TrueColor), Style::fg(Colour::Rgb(1, 2, 3)));
    assert_eq!(palette.style(Role::Quote, ColourMode::Ansi16), Style::PLAIN.italic());
}
