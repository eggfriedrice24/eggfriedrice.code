//! The output of the renderer against the frames of the terminal design
//! (`efr_test_support::design`). The pattern for every slice of the terminal look:
//! build the options of a depth, render mock text at the frame's width, and compare
//! the rows with the frame, cell by cell. One module for each board: `markdown`
//! (TM-Markdown and the Markdown marks of TM-Legend), `code` (TM-Code, the code block
//! of TM-Legend and TM-Depths), `diffs` (the rows of `diff_lines` against TM-Diffs and
//! the diff rows of TM-Legend).

mod code;
mod diffs;
mod markdown;

use efr_render::{CodeTheme, Colour, ColourMode, Palette, RenderOptions, Role, render};
use efr_test_support::design::{
    CellRow, DESIGN_ROLES, DESIGN_THEME, Depth, Frame, Ink, Look, assert_rows, cells, expect, rgb,
};

/// The options of `depth` at `cols` columns: the palette of the design in truecolor,
/// the default palette in 16 colours and without colour, as a user with and without
/// the palette file of the design system sees efr.
fn options(depth: Depth, cols: u16) -> RenderOptions {
    let options = RenderOptions::new(cols);
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

/// `out` as a terminal `cols` wide shows it: a line wider than the terminal goes on in
/// the next row, and the row that the terminal wraps ends in an erase with the
/// background that its last cell has, which is the background that the terminal
/// shows to its edge. Each visible character is one cell.
fn soft_wrap(out: &str, cols: u16) -> String {
    let cols = usize::from(cols);
    let mut wrapped = String::with_capacity(out.len());
    let mut column = 0;
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                wrapped.push(c);
                match chars.next() {
                    Some('[') => {
                        wrapped.push('[');
                        for c in chars.by_ref() {
                            wrapped.push(c);
                            if ('\x40'..='\x7e').contains(&c) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        wrapped.push(']');
                        while let Some(c) = chars.next() {
                            wrapped.push(c);
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' && chars.peek() == Some(&'\\') {
                                wrapped.extend(chars.next());
                                break;
                            }
                        }
                    }
                    other => wrapped.extend(other),
                }
            }
            '\n' => {
                wrapped.push(c);
                column = 0;
            }
            c => {
                if column == cols {
                    wrapped.push_str("\x1b[K\n");
                    column = 0;
                }
                wrapped.push(c);
                column += 1;
            }
        }
    }
    wrapped
}

fn theme(bytes: &[u8]) -> CodeTheme {
    CodeTheme::from_tmtheme(bytes).unwrap()
}

/// The options of `depth` with the fixture code themes: the truecolor theme in
/// truecolor, its ANSI form in 16 colours.
fn code_options(depth: Depth, cols: u16) -> RenderOptions {
    let bat = std::path::Path::new(DESIGN_THEME).join("bat");
    let truecolor = theme(&std::fs::read(bat.join("eggfriedrice.tmTheme")).unwrap());
    let ansi = theme(&std::fs::read(bat.join("eggfriedrice-ansi.tmTheme")).unwrap());
    options(depth, cols).with_code_theme(Some(truecolor)).with_code_theme_ansi(Some(ansi))
}

/// The cells of `out` as a terminal `cols` wide shows them.
fn screen(out: &str, cols: u16) -> Vec<CellRow> {
    cells(&soft_wrap(out, cols))
}

/// `look` with the text colour of the design as the terminal's own, and a background
/// that is the row's fill as none.
fn plain(look: Look, fill: Option<Ink>) -> Look {
    let text = Ink::Rgb(0xd8, 0xd3, 0xc3);
    Look {
        fg: if look.fg == text { Ink::Default } else { look.fg },
        bg: if Some(look.bg) == fill { Ink::Default } else { look.bg },
        ..look
    }
}

/// How much of a row of a frame a test compares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    /// The text, the fill, and the colours and attributes of each cell.
    All,
    /// All but the text colour of the cells.
    NoTextColour,
}

/// Compares the rows `rows` of `frame` in `depth` with `real`: the text and the fill
/// of each row, and the background and the attributes of each cell; the text colour
/// only where `check` says so for the row. Spaces at the end of a row compare by the
/// fill alone: on a filled row they show nothing more.
fn assert_code_rows(
    frame: &Frame,
    rows: std::ops::Range<usize>,
    real: &[CellRow],
    depth: Depth,
    colour: impl Fn(usize) -> bool,
) {
    let want = expect(frame, depth).unwrap();
    assert_eq!(real.len(), rows.len(), "rows of {} {}", frame.board, frame.number);
    for (at, real) in rows.zip(real) {
        let row = &want.rows[at];
        let check = match () {
            () if colour(at) => Check::All,
            () => Check::NoTextColour,
        };
        assert_eq!(real.text().trim_end(), row.text.trim_end(), "row {at}");
        assert_eq!(real.fill, row.fill, "the fill of row {at}: {}", row.text);
        let wanted: Vec<Look> = row
            .segments
            .iter()
            .flat_map(|segment| segment.text.chars().map(move |_| segment.look))
            .collect();
        for (column, (cell, want)) in real.cells.iter().zip(wanted).enumerate() {
            let (cell, want) = (plain(cell.look, real.fill), plain(want, row.fill));
            let (cell, want) = if check == Check::All || cell_is_space(real, column) {
                (cell, want)
            } else {
                (Look { fg: Ink::Default, ..cell }, Look { fg: Ink::Default, ..want })
            };
            if cell_is_space(real, column) {
                assert_eq!(cell.bg, want.bg, "row {at} column {column}: {}", row.text);
                continue;
            }
            assert_eq!(cell, want, "row {at} column {column}: {}", row.text);
        }
    }
}

fn cell_is_space(row: &CellRow, column: usize) -> bool {
    row.cells[column].text == ' '
}

/// TM-Legend frame 6, prose: the text in `text` and inline code in `code`, green in
/// truecolor and slot 2 in 16 colours. The Legend draws no `NO_COLOR` frame of it; its
/// note says the backticks stay there, which the Markdown board draws (frame 2).
#[test]
fn prose_with_inline_code_is_the_legend() {
    let frame = Frame::load("TM-Legend", 6).unwrap();
    for depth in [Depth::TrueColor, Depth::Ansi16] {
        let out = render("The steer goes into `pending` while\n", &options(depth, frame.cols));
        assert_rows(&frame, .., &out, depth);
    }
}

#[test]
fn every_role_of_the_design_is_a_role_of_the_palette() {
    let names: Vec<&str> = Role::ALL.iter().map(|role| role.name()).collect();
    let design: Vec<&str> = DESIGN_ROLES.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, design);
}
