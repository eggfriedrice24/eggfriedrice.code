use pretty_assertions::assert_eq;

use super::{DiffLineKind, diff_lines};
use crate::diff_rows;
use crate::options::{ColourMode, RenderOptions};
use crate::palette::{Palette, Role};
use crate::style::Colour;

const DIFF: &str = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@ fn main\n fn main() {\n-    old();\n+    new();\n }\n";

#[test]
fn each_line_has_its_kind_and_the_sign_apart() {
    let options = RenderOptions::new(80).with_terminal(false);
    let lines = diff_lines(DIFF, &options);
    let kinds: Vec<DiffLineKind> = lines.iter().map(|line| line.kind).collect();
    assert_eq!(
        kinds,
        [
            DiffLineKind::Other,
            DiffLineKind::Other,
            DiffLineKind::Hunk,
            DiffLineKind::Context,
            DiffLineKind::Removed,
            DiffLineKind::Added,
            DiffLineKind::Context,
        ]
    );
    let signs: Vec<&str> = lines.iter().map(|line| line.sign.as_str()).collect();
    assert_eq!(signs, ["", "", "", " ", "-", "+", " "]);
    assert_eq!(lines[5].rows, ["    new();"]);
    assert!(lines.iter().all(|line| line.fill.is_none()));
}

#[test]
fn the_sign_and_the_rows_paint_what_diff_rows_paints() {
    for mode in [ColourMode::TrueColor, ColourMode::Ansi16, ColourMode::None] {
        let options = RenderOptions::new(80).with_colour(mode);
        let lines = diff_lines(DIFF, &options);
        let rows = diff_rows(DIFF, &options);
        let kinds: Vec<DiffLineKind> = lines.iter().map(|line| line.kind).collect();
        assert_eq!(kinds[2], DiffLineKind::Hunk, "{mode:?}");
        assert_eq!(kinds[4], DiffLineKind::Removed, "{mode:?}");
        for (line, today) in lines.iter().zip(&rows) {
            // The text of the sign and the content is the text of today's row.
            let ours = strip(&format!("{}{}", line.sign, line.rows.concat()));
            assert_eq!(ours.trim_end(), strip(&today.concat()).trim_end(), "{mode:?}");
        }
    }
}

#[test]
fn a_fill_comes_only_from_a_background_role_in_truecolor() {
    let palette = Palette::new()
        .with(Role::DiffAddBg, Colour::Rgb(0x19, 0x2c, 0x17))
        .with(Role::DiffRemoveBg, Colour::Rgb(0x39, 0x1d, 0x1e));
    let truecolor =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette.clone());
    let lines = diff_lines(DIFF, &truecolor);
    assert_eq!(lines[4].fill.as_deref(), Some("48;2;57;29;30"));
    assert_eq!(lines[5].fill.as_deref(), Some("48;2;25;44;23"));
    assert_eq!(lines[3].fill, None);
    let ansi = RenderOptions::new(80).with_colour(ColourMode::Ansi16).with_palette(palette);
    assert!(diff_lines(DIFF, &ansi).iter().all(|line| line.fill.is_none()));
}

#[test]
fn the_sign_is_one_cell_apart_from_the_text_that_shares_its_span() {
    for mode in [ColourMode::Ansi16, ColourMode::None] {
        let lines = diff_lines(DIFF, &RenderOptions::new(80).with_colour(mode));
        assert_eq!(lines[3].sign, " ", "{mode:?}");
        assert_eq!(strip(&lines[3].rows[0]), "fn main() {", "{mode:?}");
        assert_eq!(strip(&lines[5].rows[0]), "    new();", "{mode:?}");
    }
}

#[test]
fn the_sign_and_the_rows_carry_the_fill_and_a_theme_blend_fills_without_the_roles() {
    let mocha = crate::Theme::from_name("catppuccin-mocha").unwrap();
    let options = RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_theme(mocha);
    let lines = diff_lines(DIFF, &options);
    let fill = lines[5].fill.clone().unwrap();
    assert!(lines[5].sign.contains(&fill), "{:?}", lines[5].sign);
    assert!(lines[5].rows[0].contains(&fill), "{:?}", lines[5].rows[0]);
    assert_eq!(lines[3].fill, None);
}

#[test]
fn a_long_line_is_cut_into_rows_of_the_width() {
    let options = RenderOptions::new(10).with_colour(ColourMode::None);
    let lines = diff_lines("@@ -1 +1 @@\n+abcdefghijklmnopqrstuvwxyz\n", &options);
    // Without colour the sign stays bold.
    assert_eq!(strip(&lines[1].sign), "+");
    assert_eq!(lines[1].rows, ["abcdefghij", "klmnopqrst", "uvwxyz"]);
}

/// `text` without its escape sequences.
fn strip(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[test]
fn the_sign_is_one_column_however_the_text_after_it_is_painted() {
    // A line that stays, whose sign is in one span with the spaces and the word after
    // it: the sign is the space alone, and no space of the text goes.
    let diff =
        "--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,1 @@\n             self.state = State::Draining;\n";
    for mode in [ColourMode::TrueColor, ColourMode::Ansi16, ColourMode::None] {
        let options = RenderOptions::new(200).with_colour(mode);
        let line = diff_lines(diff, &options).pop().unwrap();
        let text = crate::width::display_width(&line.rows.concat(), options.width_method());
        assert_eq!(crate::width::display_width(&line.sign, options.width_method()) + text, 42);
    }
}
