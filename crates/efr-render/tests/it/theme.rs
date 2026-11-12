//! The theme of the design system (eggfriedrice) against the syntax table of TM-Code.
//! The code blocks of the board, rendered with the code themes that the theme file
//! names, are compared cell by cell with frame 1 (truecolor, `eggfriedrice.tmTheme`)
//! and frame 2 (16 colours, `eggfriedrice-ansi.tmTheme`): the colour and the
//! attributes of each cell of code. The label row, the background of the rows and the
//! fences belong to the code blocks of the renderer, not to the theme, so this file
//! does not compare them. The fenced diff of the board is left out for the same
//! reason: the renderer paints its rows itself.
//!
//! Every cell must be the board's. The board follows the tree-sitter captures of the
//! editor; where syntect's grammars give a token no scope or the scope of another
//! kind (plain Rust variables, fields, macro names, `self`, `usize`, `const ID`, the
//! shebang, shell numbers, `-ne`), the renderer adds the scope of the capture.

use std::path::Path;

use efr_render::{CodeTheme, Colour, ColourMode, Palette, RenderOptions, Role, render};
use efr_test_support::design::{
    CellRow, DESIGN_ROLES, DESIGN_THEME, Depth, Frame, Ink, Look, Row, cells, rgb,
};

fn code_theme(name: &str) -> CodeTheme {
    let path = Path::new(DESIGN_THEME).join("bat").join(name);
    CodeTheme::from_tmtheme(&std::fs::read(path).unwrap()).unwrap()
}

/// The options of a user with the theme file of the design system: its palette and
/// both code themes in truecolor; the default palette and the code theme for 16
/// colours in 16 colours, as `efr_test_support::design` sets each depth.
fn options(depth: Depth, cols: u16) -> RenderOptions {
    let options = RenderOptions::new(cols)
        .with_code_theme(Some(code_theme("eggfriedrice.tmTheme")))
        .with_code_theme_ansi(Some(code_theme("eggfriedrice-ansi.tmTheme")));
    match depth {
        Depth::TrueColor => {
            let mut palette = Palette::new();
            for (name, hex) in DESIGN_ROLES {
                let (Some(role), Some((r, g, b))) = (Role::from_name(name), rgb(hex)) else {
                    panic!("{name} {hex} is no role of efr-render");
                };
                palette.set(role, Colour::Rgb(r, g, b));
            }
            options.with_colour(ColourMode::TrueColor).with_palette(palette)
        }
        Depth::Ansi16 => options.with_colour(ColourMode::Ansi16),
        Depth::NoColor => options.with_colour(ColourMode::None),
    }
}

/// The cells of the rows of a frame, with the rows that the terminal soft-wrapped (a
/// row of exactly `cols` cells and the row after it) joined again into one line.
fn lines(rows: &[Row], cols: u16) -> Vec<Vec<(char, Look)>> {
    let mut out: Vec<Vec<(char, Look)>> = Vec::new();
    let mut wrapped = false;
    for row in rows {
        let cells: Vec<(char, Look)> = row
            .segments
            .iter()
            .flat_map(|segment| segment.text.chars().map(|text| (text, segment.look)))
            .collect();
        let full = cells.len() == usize::from(cols);
        match out.last_mut() {
            Some(last) if wrapped => last.extend(cells),
            _ => out.push(cells),
        }
        wrapped = full;
    }
    out
}

fn text(line: &[(char, Look)]) -> String {
    line.iter().map(|(text, _)| text).collect()
}

/// The code lines of the board: the language of each block, the line in the block and
/// its text, from the fences of the `NO_COLOR` frame (frame 3).
fn code_lines(source: &[String]) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let mut block: Option<(String, usize)> = None;
    for line in source {
        if let Some(lang) = line.strip_prefix("```") {
            block = match block {
                Some(_) => None,
                None => Some((lang.to_owned(), 0)),
            };
            continue;
        }
        if let Some((lang, at)) = block.as_mut() {
            out.push((lang.clone(), *at, line.clone()));
            *at += 1;
        }
    }
    out
}

/// The look of a cell as the theme sets it: the colour of the text, with the board's
/// foreground as the terminal's own, and the attributes. The background is the
/// renderer's.
fn theme_look(look: Look, foreground: Ink) -> Look {
    let fg = if look.fg == foreground { Ink::Default } else { look.fg };
    Look { fg, bg: Ink::Default, ..look }
}

/// The runs of cells of code where `real` differs from `frame`: language, line, token.
fn differences(frame: &Frame, depth: Depth) -> Vec<(String, usize, String)> {
    let nocolor = Frame::load("TM-Code", 3).unwrap();
    let source: Vec<String> =
        lines(&nocolor.rows, nocolor.cols).iter().map(|line| text(line)).collect();
    let markdown = format!("{}\n", source.join("\n"));
    let real: Vec<CellRow> = cells(&render(&markdown, &options(depth, frame.cols)));
    let board = lines(&frame.rows, frame.cols);
    let foreground = if depth == Depth::TrueColor { frame.foreground } else { Ink::Default };
    let mut found = Vec::new();
    let (mut at_real, mut at_board) = (0, 0);
    for (lang, line, code) in code_lines(&source) {
        if lang == "diff" || code.trim().is_empty() {
            continue;
        }
        let Some(r) = real[at_real..].iter().position(|row| row.text().trim_end() == code) else {
            panic!("{lang} line {line} is not in the output: {code}");
        };
        let Some(b) = board[at_board..].iter().position(|row| text(row) == code) else {
            panic!("{lang} line {line} is not in frame {}: {code}", frame.number);
        };
        at_real += r;
        at_board += b;
        let real_cells = &real[at_real].cells;
        let want = &board[at_board];
        let chars: Vec<char> = code.chars().collect();
        let look_at = |column: usize| {
            let w = want.get(column).map(|(_, look)| theme_look(*look, foreground));
            let g = real_cells.get(column).map(|cell| theme_look(cell.look, foreground));
            (w, g)
        };
        let mut column = 0;
        while column < chars.len() {
            let (w, g) = look_at(column);
            if chars.get(column) == Some(&' ') || w == g {
                column += 1;
                continue;
            }
            let start = column;
            while column < chars.len()
                && chars.get(column) != Some(&' ')
                && look_at(column) == (w, g)
            {
                column += 1;
            }
            let token: String = chars.get(start..column).unwrap_or_default().iter().collect();
            found.push((lang.clone(), line, token));
        }
    }
    found
}

fn check(number: u32, depth: Depth) {
    let frame = Frame::load("TM-Code", number).unwrap();
    assert_eq!(frame.depth, depth, "frame {number} of TM-Code");
    let found = differences(&frame, depth);
    assert_eq!(found, [], "the code of TM-Code frame {number} differs from the board");
}

/// TM-Code frame 1: the syntax colours of the table in truecolor.
#[test]
fn the_code_theme_is_the_syntax_table_in_truecolor() {
    check(1, Depth::TrueColor);
}

/// TM-Code frame 2: the slots of the table in 16 colours, from the ANSI code theme.
#[test]
fn the_ansi_code_theme_is_the_syntax_table_in_sixteen_colours() {
    check(2, Depth::Ansi16);
}
