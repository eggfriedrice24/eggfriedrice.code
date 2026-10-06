//! Snapshots per element at 80 and 40 columns, with the `ansi` theme. Escape bytes are
//! written as `\e` so the snapshots stay readable and diffable.

use super::{ELEMENTS, element};
use crate::options::{ColourMode, RenderOptions, Theme};
use crate::render;

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
    let options = RenderOptions::new(80).with_colour(ColourMode::None);
    insta::assert_snapshot!(readable(&render(&document, &options)));
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
