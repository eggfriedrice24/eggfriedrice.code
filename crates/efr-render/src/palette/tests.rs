use pretty_assertions::assert_eq;

use super::{Palette, Role};
use crate::RenderError;
use crate::options::{ColourMode, RenderOptions};
use crate::style::{Attrs, Colour, Style};

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
            "diff.hunk",
            "code.bg",
            "diff.add.bg",
            "diff.remove.bg"
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
        "text= muted=2 accent=33 heading=1;33 link=4;35 code=32 success=32 warning=33 \
         error=31 quote=3 diff.add=32 diff.remove=31 diff.hunk=36 code.bg= diff.add.bg= \
         diff.remove.bg="
    );
    // Palette entries stay palette entries in truecolor, so they follow the theme.
    assert_eq!(table(&defaults, ColourMode::TrueColor), table(&defaults, ColourMode::Ansi16));
}

#[test]
fn without_colour_roles_keep_their_attributes_and_some_get_one_in_place() {
    assert_eq!(
        table(&Palette::new(), ColourMode::None),
        "text= muted=2 accent=1 heading=1 link=4 code= success= warning=1 error=1 quote=3 \
         diff.add= diff.remove= diff.hunk=2 code.bg= diff.add.bg= diff.remove.bg="
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
    assert_eq!(palette.colour(Role::Link), Some(Colour::Palette(5)));
    // A muted role with a colour is not dim; without colour it is dim again.
    let ansi = RenderOptions::new(80).with_palette(palette.clone());
    assert_eq!(ansi.sgr(Role::Muted).as_deref(), Some("90"));
    assert_eq!(ansi.sgr(Role::Warning).as_deref(), Some("95"));
    assert_eq!(ansi.sgr(Role::Text).as_deref(), Some("97"));
    let none = ansi.with_colour(ColourMode::None);
    assert_eq!(none.sgr(Role::Muted).as_deref(), Some("2"));
    assert_eq!(none.sgr(Role::Text), None);
}

#[test]
fn hex_colours_are_rgb_in_truecolor_and_the_roles_own_slot_in_sixteen() {
    let palette = Palette::new()
        .with(Role::Accent, Colour::Rgb(0xf2, 0xc1, 0x4e))
        .with(Role::Error, Colour::Rgb(0xfa, 0x3c, 0x3c));
    let truecolor =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette.clone());
    assert_eq!(truecolor.sgr(Role::Accent).as_deref(), Some("38;2;242;193;78"));
    assert_eq!(truecolor.sgr(Role::Heading).as_deref(), Some("1;38;2;242;193;78"));
    // NOTE: in 16 colours each role is its own slot (TM-Depths 5 to 8), whatever hex
    // colour the palette gives it.
    let sixteen = truecolor.clone().with_colour(ColourMode::Ansi16);
    let default = RenderOptions::new(80).with_colour(ColourMode::Ansi16);
    assert_eq!(sixteen.sgr(Role::Accent), default.sgr(Role::Accent));
    assert_eq!(sixteen.sgr(Role::Error), default.sgr(Role::Error));
    assert_eq!(sixteen.sgr(Role::Muted), default.sgr(Role::Muted));
    // A slot that the palette names stays.
    let slot = default.clone().with_palette(Palette::new().with(Role::Accent, Colour::Palette(5)));
    assert_eq!(slot.sgr(Role::Accent).as_deref(), Some("35"));
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
    assert_eq!(options.paint(Role::Warning, "allow?"), "\x1b[33mallow?\x1b[0m");
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
    // Without colour, a role keeps what stands in for its colour, and nothing else;
    // the bold of `warning` marks a question, and a gauge must not borrow it.
    let none = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(none.paint(Role::Warning, "allow?"), "\x1b[1mallow?\x1b[0m");
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

#[test]
fn a_background_role_paints_only_an_rgb_colour_in_truecolor() {
    let palette = Palette::new()
        .with(Role::CodeBg, Colour::Rgb(0x12, 0x17, 0x22))
        .with(Role::DiffAddBg, Colour::Palette(2));
    let truecolor =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette.clone());
    assert_eq!(truecolor.fill(Role::CodeBg).as_deref(), Some("48;2;18;23;34"));
    assert_eq!(truecolor.sgr(Role::CodeBg).as_deref(), Some("48;2;18;23;34"));
    assert_eq!(truecolor.paint(Role::CodeBg, "x"), "\x1b[48;2;18;23;34mx\x1b[0m");
    // A palette entry is never reduced to a background, and a role without a colour
    // has none.
    assert_eq!(truecolor.fill(Role::DiffAddBg), None);
    assert_eq!(truecolor.fill(Role::DiffRemoveBg), None);
    // A role that is not a background has no fill.
    assert_eq!(truecolor.fill(Role::Muted), None);
    for mode in [ColourMode::Ansi16, ColourMode::None] {
        let other = truecolor.clone().with_colour(mode);
        assert_eq!(other.fill(Role::CodeBg), None, "{mode:?}");
        assert_eq!(other.sgr(Role::CodeBg), None, "{mode:?}");
    }
    assert_eq!(truecolor.with_terminal(false).fill(Role::CodeBg), None);
    for role in Role::ALL {
        assert_eq!(
            role.is_background(),
            matches!(role, Role::CodeBg | Role::DiffAddBg | Role::DiffRemoveBg),
            "{role}"
        );
        if role.is_background() {
            assert_eq!(Palette::new().colour(role), None, "{role}");
        }
    }
}

#[test]
fn paint_with_adds_attributes_to_the_role_in_every_mode() {
    let ansi = RenderOptions::new(80);
    assert_eq!(ansi.paint_with(Role::Warning, Attrs::BADGE, " ? "), "\x1b[1;7;33m ? \x1b[0m");
    assert_eq!(ansi.paint_with(Role::Text, Attrs::BOLD, "steer"), "\x1b[1msteer\x1b[0m");
    assert_eq!(ansi.paint_with(Role::Text, Attrs::ITALIC, "thought"), "\x1b[3mthought\x1b[0m");
    assert_eq!(ansi.paint_with(Role::Muted, Attrs::NONE, "note"), ansi.paint(Role::Muted, "note"));
    let rgb = Palette::new().with(Role::Error, Colour::Rgb(0xe0, 0x6c, 0x75));
    let truecolor = RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(rgb);
    assert_eq!(
        truecolor.paint_with(Role::Error, Attrs::BOLD, "x"),
        "\x1b[1;38;2;224;108;117mx\x1b[0m"
    );
    // Attributes are not colour: they stay without colour.
    let none = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(none.paint_with(Role::Accent, Attrs::BOLD, "\u{203a}"), "\x1b[1m\u{203a}\x1b[0m");
    assert_eq!(none.paint_with(Role::Text, Attrs::DIM, "x"), "\x1b[2mx\x1b[0m");
    let attrs = Attrs { underline: true, ..Attrs::NONE };
    assert_eq!(none.paint_with(Role::Code, attrs, "x"), "\x1b[4mx\x1b[0m");
    assert_eq!(none.paint_with(Role::Text, Attrs::NONE, "x"), "x");
    let piped = RenderOptions::new(80).with_terminal(false);
    assert_eq!(piped.paint_with(Role::Warning, Attrs::BADGE, " ? "), " ? ");
}
