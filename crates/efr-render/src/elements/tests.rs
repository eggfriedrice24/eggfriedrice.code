//! Snapshots per element at 80 and 40 columns, with the `ansi` theme; every element
//! without colour; every role in 16 colours, without colour, with overrides and with a
//! hex palette; and a `.tmTheme` code theme. Escape bytes are written as `\e` so the
//! snapshots stay readable and diffable.

use pretty_assertions::assert_eq;

use super::{ELEMENTS, ROLES, SAMPLE_TMTHEME, element};
use crate::options::{ColourMode, RenderOptions, Theme};
use crate::{CodeTheme, Colour, Palette, Role, render, render_trace};

fn readable(painted: &str) -> String {
    painted.replace('\x1b', "\\e")
}

fn snapshot(name: &str) {
    let markdown = element(name);
    assert!(!markdown.is_empty(), "no element named {name}");
    for width in [80, 40] {
        let options = RenderOptions::new(width).with_theme(Theme::ANSI);
        insta::assert_snapshot!(format!("{name}_w{width}"), readable(&render(markdown, &options)));
    }
}

macro_rules! element_snapshots {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                snapshot(stringify!($name));
            }
        )*

        #[test]
        fn every_element_has_a_snapshot_test() {
            let tested = [$(stringify!($name)),*];
            for (name, _) in ELEMENTS {
                assert!(tested.contains(name), "element {name} has no snapshot test");
            }
        }
    };
}

element_snapshots!(
    headings,
    emphasis,
    paragraphs,
    long_paragraph,
    bullet_list,
    ordered_list,
    task_list,
    quote,
    code_rust,
    code_plain,
    code_in_list,
    diff,
    table,
    table_wide,
    links,
    image,
    rule,
    html,
);

#[test]
fn links_without_hyperlinks() {
    let options = RenderOptions::new(80).with_hyperlinks(false);
    insta::assert_snapshot!(readable(&render(element("links"), &options)));
}

#[test]
fn everything_without_colour() {
    let document: String =
        ELEMENTS.iter().map(|(_, markdown)| *markdown).collect::<Vec<_>>().join("\n");
    for width in [80, 40] {
        let options = RenderOptions::new(width).with_colour(ColourMode::None);
        let name = if width == 80 {
            "everything_without_colour".to_owned()
        } else {
            format!("everything_without_colour_w{width}")
        };
        insta::assert_snapshot!(name, readable(&render(&document, &options)));
    }
}

/// The roles document, then one line per role painted by `RenderOptions::paint`.
fn every_role(options: &RenderOptions) -> String {
    let mut out = render(ROLES, options);
    out.push('\n');
    for role in Role::ALL {
        out.push_str(&options.paint(role, role.name()));
        out.push('\n');
    }
    out.push_str(&render_trace("shell: cargo test", options));
    readable(&out)
}

#[test]
fn every_role_in_sixteen_colours() {
    for width in [80, 40] {
        let options = RenderOptions::new(width);
        insta::assert_snapshot!(
            format!("every_role_in_sixteen_colours_w{width}"),
            every_role(&options)
        );
    }
}

#[test]
fn every_role_without_colour() {
    let options = RenderOptions::new(80).with_colour(ColourMode::None);
    insta::assert_snapshot!(every_role(&options));
}

/// A palette as a config sets it, with ANSI slots only.
fn slot_overrides() -> Palette {
    Palette::new()
        .with(Role::Accent, Colour::Palette(5))
        .with(Role::Muted, Colour::Palette(8))
        .with(Role::Code, Colour::Palette(3))
        .with(Role::Link, Colour::Palette(12))
        .with(Role::Quote, Colour::Palette(7))
        .with(Role::DiffAdd, Colour::Palette(10))
}

#[test]
fn every_role_with_overrides() {
    let options = RenderOptions::new(80).with_palette(slot_overrides());
    insta::assert_snapshot!(every_role(&options));
}

/// A palette as a theme file of the eggfriedrice design system sets it, in hex.
fn hex_palette() -> Palette {
    let hex = |rgb: u32| {
        let [_, r, g, b] = rgb.to_be_bytes();
        Colour::Rgb(r, g, b)
    };
    [
        (Role::Text, 0xe8e2d4),
        (Role::Muted, 0x7a7266),
        (Role::Accent, 0xf2c14e),
        (Role::Heading, 0xf2c14e),
        (Role::Link, 0x7fb4ca),
        (Role::Code, 0x9fc27a),
        (Role::Success, 0x8fbf6a),
        (Role::Warning, 0xe8a33d),
        (Role::Error, 0xe05d4f),
        (Role::Quote, 0xb8b0a0),
        (Role::DiffAdd, 0x8fbf6a),
        (Role::DiffRemove, 0xe05d4f),
        (Role::DiffHunk, 0x7fb4ca),
    ]
    .into_iter()
    .fold(Palette::new(), |palette, (role, rgb)| palette.with(role, hex(rgb)))
}

#[test]
fn a_hex_palette_in_truecolor() {
    let options =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(hex_palette());
    insta::assert_snapshot!(every_role(&options));
}

#[test]
fn a_hex_palette_falls_back_to_the_nearest_of_sixteen() {
    let options = RenderOptions::new(80).with_palette(hex_palette());
    // The roles keep their meaning: an added line never looks like a removed one.
    let roles = [Role::DiffAdd, Role::DiffRemove, Role::Muted, Role::Link];
    let sgr: Vec<_> = roles.iter().map(|role| options.sgr(*role)).collect();
    for (at, one) in sgr.iter().enumerate() {
        assert!(!sgr[at + 1..].contains(one), "two roles share {one:?}: {sgr:?}");
    }
    insta::assert_snapshot!(every_role(&options));
}

#[test]
fn a_hex_palette_without_colour_is_plain() {
    let palette = RenderOptions::new(80).with_colour(ColourMode::None).with_palette(hex_palette());
    let plain = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(every_role(&palette), every_role(&plain));
}

#[test]
fn a_tmtheme_code_theme() {
    let theme = CodeTheme::from_tmtheme(SAMPLE_TMTHEME).unwrap();
    let mut shown = String::new();
    for colour in [ColourMode::TrueColor, ColourMode::Ansi16] {
        let options = RenderOptions::new(80)
            .with_colour(colour)
            .with_theme(Theme::from_name("nord").unwrap())
            .with_code_theme(Some(theme.clone()));
        let code = render(element("code_rust"), &options);
        let diff = render(element("diff"), &options);
        shown.push_str(&format!("{colour:?}:\n{}\n{}\n", readable(&code), readable(&diff)));
    }
    insta::assert_snapshot!(shown);
}

#[test]
fn diff_in_a_truecolor_theme() {
    let theme = Theme::from_name("catppuccin-mocha").unwrap();
    let options = RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_theme(theme);
    insta::assert_snapshot!(readable(&render(element("diff"), &options)));
}

#[test]
fn rust_in_sixteen_colours_from_a_truecolor_theme() {
    let theme = Theme::from_name("gruvbox-dark").unwrap();
    let options = RenderOptions::new(80).with_colour(ColourMode::Ansi16).with_theme(theme);
    insta::assert_snapshot!(readable(&render(element("code_rust"), &options)));
}

/// A fence's language reads as a label above the code, not as a line of the reply: dim
/// with colour or without, and absent from output that is not a terminal, which keeps
/// the markdown as it came.
#[test]
fn a_code_label_is_dim_on_a_terminal_and_plain_markdown_elsewhere() {
    let markdown = "Run this:\n\n```sh\nls -la\n```\n\nIt prints:\n\n```text\ntotal 0\n```\n";
    let mut shown = String::new();
    for colour in [ColourMode::Ansi16, ColourMode::None] {
        let options = RenderOptions::new(80).with_theme(Theme::ANSI).with_colour(colour);
        shown.push_str(&format!("{colour:?}:\n{}\n", readable(&render(markdown, &options))));
    }
    insta::assert_snapshot!(shown);
    let piped = RenderOptions::new(80).with_terminal(false);
    assert_eq!(render(markdown, &piped), markdown);
}
