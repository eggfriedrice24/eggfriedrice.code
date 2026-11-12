//! The frames of the terminal design as test fixtures, and a check that compares the
//! real output of efr with them, cell by cell.
//!
//! The boards of the design (`TM-Calls`, `TM-Legend`, ...) draw each element of efr's
//! terminal look as frames: rows of text, each piece with its colour and its
//! attributes. `cargo xtask design-frames` copies them into `fixtures/design/` of this
//! crate, one `TM-<board>.jsonl` for each board and one JSON object for each frame,
//! with each colour resolved for the depth of the frame (see the module of the xtask
//! for the frames it drops and the corrections it makes). A test of a slice of the
//! terminal look:
//!
//! 1. gives efr mock events or mock Markdown (a prose row of a frame keeps the line
//!    breaks of the board, so the Markdown has a line break at the end of each row),
//! 2. renders them at the frame's width ([`Frame::cols`]) with the options of a depth:
//!    in [`Depth::TrueColor`] a palette of [`DESIGN_ROLES`], in [`Depth::Ansi16`] and
//!    [`Depth::NoColor`] the default palette (the 16 colours of the terminal, or none),
//! 3. compares the output with the rows of the frame: [`assert_rows`].
//!
//! ```ignore
//! let frame = Frame::load("TM-Legend", 6).unwrap();
//! for depth in [Depth::TrueColor, Depth::Ansi16] {
//!     let out = efr_render::render("The steer goes into `pending` while\n", &options(depth));
//!     assert_rows(&frame, .., &out, depth);
//! }
//! ```
//!
//! A board draws some frames in all three depths (`TM-Depths`, `TM-Calls` 17 to 19,
//! `TM-Code`, `TM-Diffs`, `TM-Questions`, `TM-Markdown`): [`expect`] takes the board's
//! own frame of a depth. For any other frame it derives the frame of a depth from the
//! truecolor frame through one table ([`expect`] names it). The table knows the roles
//! of efr's own rows and of code rows, nothing more, so a test of an element that a
//! board draws in a depth of its own compares with that frame.
//!
//! This crate does not depend on `efr-render`: a colour here is an [`Ink`], and the
//! roles are [`DESIGN_ROLES`], names and hex.

use std::fmt;
use std::ops::RangeBounds;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::TestSupportError;

/// The fixtures of the design, in this crate.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/design");

/// The theme of the design system (eggfriedrice), as its generator writes it: the theme
/// file `efr/eggfriedrice.toml`, which names the code themes
/// `bat/eggfriedrice.tmTheme` (truecolor) and `bat/eggfriedrice-ansi.tmTheme` (16
/// colours, bat's ANSI encoding) by relative paths. Each file is a copy of the file of
/// the same path under `extras/` of the design system; a change there is copied here.
pub const DESIGN_THEME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/design/theme");

/// Each of the 16 roles of efr's palette (`efr_render::Role`, by its name) with the
/// colour that the design gives it (TM-Legend, the table of the 13 roles, and the three
/// backgrounds of DECISIONS.md). A test in [`Depth::TrueColor`] builds its palette from
/// it, so its output shows the colours of the boards.
pub const DESIGN_ROLES: [(&str, &str); 16] = [
    ("text", "#d8d3c3"),
    ("muted", "#8695b7"),
    ("accent", "#ffc940"),
    ("heading", "#e06c75"),
    ("link", "#c084fc"),
    ("code", "#60e654"),
    ("success", "#60e654"),
    ("warning", "#ffc940"),
    ("error", "#e06c75"),
    ("quote", "#a8a396"),
    ("diff.add", "#60e654"),
    ("diff.remove", "#e06c75"),
    ("diff.hunk", "#78e2d6"),
    ("code.bg", "#121722"),
    ("diff.add.bg", "#192c17"),
    ("diff.remove.bg", "#391d1e"),
];

/// The red, green and blue of a colour written `#rrggbb`, such as a colour of
/// [`DESIGN_ROLES`]; `None` for anything else.
pub fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let digits = hex.strip_prefix('#').filter(|digits| digits.len() == 6)?;
    let part = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some((part(0)?, part(2)?, part(4)?))
}

/// The colour of a cell's text or background.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Ink {
    /// The terminal's own colour: no SGR colour.
    #[default]
    Default,
    /// 24-bit colour.
    Rgb(u8, u8, u8),
    /// A slot of the terminal's palette, 0 to 255; 0 to 15 are the 16 colours.
    Slot(u8),
}

impl fmt::Display for Ink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ink::Default => f.write_str("default"),
            Ink::Rgb(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
            Ink::Slot(slot) => write!(f, "slot {slot}"),
        }
    }
}

/// How a piece of text looks: its colours and its attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Look {
    /// The colour of the text.
    pub fg: Ink,
    /// The colour behind the text.
    pub bg: Ink,
    /// Bold (SGR 1).
    pub bold: bool,
    /// Italic (SGR 3).
    pub italic: bool,
    /// Underlined (SGR 4).
    pub underline: bool,
    /// Reverse video (SGR 7).
    pub reverse: bool,
    /// Dim (SGR 2).
    pub dim: bool,
    /// Struck through (SGR 9).
    pub strike: bool,
}

impl fmt::Display for Look {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = vec![format!("fg {}", self.fg)];
        if self.bg != Ink::Default {
            parts.push(format!("bg {}", self.bg));
        }
        for (on, name) in [
            (self.bold, "bold"),
            (self.italic, "italic"),
            (self.underline, "underline"),
            (self.reverse, "reverse"),
            (self.dim, "dim"),
            (self.strike, "strike"),
        ] {
            if on {
                parts.push(name.to_owned());
            }
        }
        f.write_str(&parts.join(" "))
    }
}

/// A piece of a row of a frame: its text and how it looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The text.
    pub text: String,
    /// Its colours and attributes.
    pub look: Look,
}

/// A row of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The text of the row: its segments joined.
    pub text: String,
    /// The pieces of the row.
    pub segments: Vec<Segment>,
    /// The background that fills the row to the edge, as efr writes it with `ESC [ K`:
    /// the rows of a code block and of a diff in truecolor.
    pub fill: Option<Ink>,
    /// The row is in the live zone (`L|` in the text export of a board).
    pub live: bool,
}

/// The depth of colour of a frame, and of the output that a test compares with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Depth {
    /// 24-bit colour with the palette of the design ([`DESIGN_ROLES`]).
    TrueColor,
    /// The 16 colours of the terminal: efr's default palette.
    Ansi16,
    /// No colour (`NO_COLOR`): bold, dim, italic and underline only.
    NoColor,
}

impl Depth {
    /// Every depth.
    pub const ALL: [Depth; 3] = [Depth::TrueColor, Depth::Ansi16, Depth::NoColor];

    fn from_name(name: &str) -> Option<Depth> {
        match name {
            "truecolor" => Some(Depth::TrueColor),
            "ansi16" => Some(Depth::Ansi16),
            "nocolor" => Some(Depth::NoColor),
            _ => None,
        }
    }
}

/// A frame of a board of the design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The board, such as `TM-Calls`.
    pub board: String,
    /// The number of the frame in its board, from 1.
    pub number: u32,
    /// The caption that the board gives the frame, such as `16 colours`.
    pub caption: Option<String>,
    /// The width of the terminal of the frame, in columns.
    pub cols: u16,
    /// The depth of colour of the frame.
    pub depth: Depth,
    /// The truecolor frame of the board that this frame draws in another depth.
    pub variant_of: Option<u32>,
    /// The colour of the terminal's own foreground on the board: text in the `text`
    /// role of the design palette has it too, so the two compare equal.
    pub foreground: Ink,
    /// The rows of the frame, from the top.
    pub rows: Vec<Row>,
}

impl Frame {
    /// Frame `number` of `board` (`TM-Calls`, or `Calls`) from the fixtures of this
    /// crate.
    pub fn load(board: &str, number: u32) -> Result<Frame, TestSupportError> {
        let board = if board.starts_with("TM-") { board.to_owned() } else { format!("TM-{board}") };
        board_frames(&board)?
            .into_iter()
            .find(|frame| frame.number == number)
            .ok_or(TestSupportError::NoDesignFrame { board, frame: number })
    }

    /// The text of each row.
    pub fn texts(&self) -> Vec<&str> {
        self.rows.iter().map(|row| row.text.as_str()).collect()
    }

    /// The cells of a row, one for each character, in the order of the row.
    fn cells_of(row: &Row) -> Vec<Cell> {
        row.segments
            .iter()
            .flat_map(|segment| segment.text.chars().map(|text| Cell { text, look: segment.look }))
            .collect()
    }
}

/// One cell of a terminal row: a character and how it looks. Every character is one
/// cell: the glyphs of the design are one cell wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The character.
    pub text: char,
    /// Its colours and attributes.
    pub look: Look,
}

/// A row of real output as a terminal shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellRow {
    /// The cells, from column 0.
    pub cells: Vec<Cell>,
    /// The background that an erase to the end of the row (`ESC [ K`) left there, when
    /// it was not the terminal's own.
    pub fill: Option<Ink>,
}

impl CellRow {
    /// The text of the row.
    pub fn text(&self) -> String {
        self.cells.iter().map(|cell| cell.text).collect()
    }
}

/// The rows of `ansi`, real output of efr, as a terminal shows them: SGR sets the look
/// of the cells that follow, `ESC [ K` erases to the end of the row and leaves its
/// background there ([`CellRow::fill`]), OSC 8 hyperlinks and other escape sequences
/// show nothing, a carriage return goes back to column 0 and a newline ends the row.
/// The cursor never moves up: give it the committed output, or the text of a live zone.
pub fn cells(ansi: &str) -> Vec<CellRow> {
    let mut rows = vec![CellRow::default()];
    let mut look = Look::default();
    let mut column = 0;
    let mut chars = ansi.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut last = None;
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            last = Some(c);
                            break;
                        }
                        params.push(c);
                    }
                    let row = rows.last_mut();
                    match (last, row) {
                        (Some('m'), _) => sgr(&mut look, &params),
                        (Some('K'), Some(row)) => {
                            if params.is_empty() || params == "0" {
                                row.cells.truncate(column);
                                row.fill = (look.bg != Ink::Default).then_some(look.bg);
                            } else if params == "2" {
                                row.cells.clear();
                                row.fill = (look.bg != Ink::Default).then_some(look.bg);
                            }
                        }
                        _ => {}
                    }
                }
                Some(']') => {
                    // NOTE: an OSC ends with BEL or with ST (`ESC \`).
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' => {
                rows.push(CellRow::default());
                column = 0;
            }
            '\r' => column = 0,
            c if c.is_control() => {}
            c => {
                if let Some(row) = rows.last_mut() {
                    let blank = Cell { text: ' ', look: Look::default() };
                    while row.cells.len() < column {
                        row.cells.push(blank);
                    }
                    let cell = Cell { text: c, look };
                    match row.cells.get_mut(column) {
                        Some(old) => *old = cell,
                        None => row.cells.push(cell),
                    }
                }
                column += 1;
            }
        }
    }
    // A newline ends a row; output that ends with one has no row after it.
    if rows.last().is_some_and(|row| row.cells.is_empty() && row.fill.is_none()) {
        rows.pop();
    }
    rows
}

/// Applies the SGR parameters `params` to `look`.
fn sgr(look: &mut Look, params: &str) {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params.split([';', ':']).map(|code| code.parse().unwrap_or(0)).collect()
    };
    let mut at = 0;
    while let Some(&code) = codes.get(at) {
        at += 1;
        match code {
            0 => *look = Look::default(),
            1 => look.bold = true,
            2 => look.dim = true,
            3 => look.italic = true,
            4 => look.underline = true,
            7 => look.reverse = true,
            9 => look.strike = true,
            22 => {
                look.bold = false;
                look.dim = false;
            }
            23 => look.italic = false,
            24 => look.underline = false,
            27 => look.reverse = false,
            29 => look.strike = false,
            30..=37 => look.fg = Ink::Slot(slot(code - 30)),
            39 => look.fg = Ink::Default,
            40..=47 => look.bg = Ink::Slot(slot(code - 40)),
            49 => look.bg = Ink::Default,
            90..=97 => look.fg = Ink::Slot(slot(code - 90 + 8)),
            100..=107 => look.bg = Ink::Slot(slot(code - 100 + 8)),
            38 | 48 => {
                let ink = match codes.get(at) {
                    Some(5) => {
                        let ink = codes.get(at + 1).map(|&n| Ink::Slot(slot(n)));
                        at += 2;
                        ink
                    }
                    Some(2) => {
                        let part = |n: usize| codes.get(at + n).map(|&v| slot(v));
                        let ink = match (part(1), part(2), part(3)) {
                            (Some(r), Some(g), Some(b)) => Some(Ink::Rgb(r, g, b)),
                            _ => None,
                        };
                        at += 4;
                        ink
                    }
                    _ => None,
                };
                if let Some(ink) = ink {
                    if code == 38 { look.fg = ink } else { look.bg = ink }
                }
            }
            _ => {}
        }
    }
}

fn slot(value: u16) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// The frame that output in `depth` must match: the board's own frame of that depth
/// when it draws one (the frame itself, or the frame whose `variant_of` names it),
/// else the truecolor frame through this table:
///
/// | Colour of the board | 16 colours | `NO_COLOR` |
/// |---|---|---|
/// | yellow (accent, warning) | slot 3 | bold |
/// | red (error) | slot 1 | bold |
/// | green (success, code) | slot 2 | plain |
/// | purple (link) | slot 5 | plain |
/// | cyan (diff.hunk) | slot 6 | dim |
/// | blue | slot 4 | plain |
/// | comment (muted) on efr's own rows | dim | dim |
/// | comment in a code row | slot 8 | plain |
/// | fg, fg_dark | the terminal's own | the terminal's own |
/// | a fill | none | none |
/// | reverse | reverse | bold |
///
/// A code row is a row with a fill in truecolor; its tokens are plain without colour.
pub fn expect(frame: &Frame, depth: Depth) -> Result<Frame, TestSupportError> {
    if frame.depth == depth {
        return Ok(frame.clone());
    }
    let truecolor = match (frame.depth, frame.variant_of) {
        (Depth::TrueColor, _) => frame.number,
        (_, Some(number)) => number,
        _ => {
            return Err(TestSupportError::NoDesignDepth {
                board: frame.board.clone(),
                frame: frame.number,
            });
        }
    };
    let frames = board_frames(&frame.board)?;
    if let Some(own) = frames.iter().find(|other| {
        other.depth == depth && (other.variant_of == Some(truecolor) || other.number == truecolor)
    }) {
        return Ok(own.clone());
    }
    let base = frames
        .into_iter()
        .find(|other| other.number == truecolor)
        .ok_or(TestSupportError::NoDesignFrame { board: frame.board.clone(), frame: truecolor })?;
    derive(&base, depth)
}

/// The truecolor frame `base` in `depth`, through the table of [`expect`].
fn derive(base: &Frame, depth: Depth) -> Result<Frame, TestSupportError> {
    let mut frame = base.clone();
    frame.depth = depth;
    frame.variant_of = Some(base.number);
    for row in &mut frame.rows {
        let code = row.fill.is_some();
        row.fill = None;
        for segment in &mut row.segments {
            segment.look = derive_look(segment.look, code, depth).ok_or_else(|| {
                TestSupportError::NoDesignColour {
                    board: base.board.clone(),
                    frame: base.number,
                    colour: segment.look.fg.to_string(),
                }
            })?;
        }
    }
    Ok(frame)
}

/// One look of a truecolor frame in `depth`; `None` for a colour that the table does
/// not know.
fn derive_look(look: Look, code: bool, depth: Depth) -> Option<Look> {
    /// The colours of the design system on the boards: name, red, green, blue.
    const TOKENS: [(&str, (u8, u8, u8)); 8] = [
        ("yellow", (0xff, 0xc9, 0x40)),
        ("red", (0xe0, 0x6c, 0x75)),
        ("green", (0x60, 0xe6, 0x54)),
        ("purple", (0xc0, 0x84, 0xfc)),
        ("cyan", (0x78, 0xe2, 0xd6)),
        ("blue", (0x6c, 0xb8, 0xff)),
        ("comment", (0x86, 0x95, 0xb7)),
        ("fg_dark", (0xa8, 0xa3, 0x96)),
    ];
    let token = match look.fg {
        Ink::Default => "fg",
        Ink::Rgb(r, g, b) => TOKENS.iter().find(|(_, rgb)| *rgb == (r, g, b))?.0,
        Ink::Slot(_) => return None,
    };
    let mut out = Look { fg: Ink::Default, bg: Ink::Default, ..look };
    match depth {
        Depth::TrueColor => return Some(look),
        Depth::Ansi16 => {
            out.fg = match token {
                "yellow" => Ink::Slot(3),
                "red" => Ink::Slot(1),
                "green" => Ink::Slot(2),
                "purple" => Ink::Slot(5),
                "cyan" => Ink::Slot(6),
                "blue" => Ink::Slot(4),
                "comment" if code => Ink::Slot(8),
                _ => Ink::Default,
            };
            out.dim |= token == "comment" && !code;
        }
        Depth::NoColor => {
            if !code {
                out.bold |= matches!(token, "yellow" | "red");
                out.dim |= matches!(token, "cyan" | "comment");
            }
            if out.reverse {
                out.reverse = false;
                out.bold = true;
            }
        }
    }
    Some(out)
}

/// Checks rows `range` of `frame` in `depth` ([`expect`]) against `real`, the output of
/// efr, row by row and cell by cell: the text, the colours and the attributes of each
/// cell, and the fill of each row. A space compares by its text and its background
/// only, so the colour of a blank does not matter, and a cell past the end of a row is
/// a blank. A cell on the background of its row's fill compares as a cell on no
/// background of its own: a row filled to the edge shows the same either way, and efr
/// paints the fill behind each cell of the row too, so a reset inside the row does not
/// drop it. In truecolor the frame's foreground ([`Frame::foreground`]) and the
/// terminal's own compare equal: the board draws both in one colour. Gives the first
/// row that differs, with both sides, as text.
pub fn check_rows(
    frame: &Frame,
    range: impl RangeBounds<usize>,
    real: &str,
    depth: Depth,
) -> Result<(), String> {
    let expected = expect(frame, depth).map_err(|error| error.to_string())?;
    let rows: Vec<&Row> = expected
        .rows
        .iter()
        .enumerate()
        .filter(|(at, _)| range.contains(at))
        .map(|(_, row)| row)
        .collect();
    let actual = cells(real);
    let name = format!("{} frame {} in {depth:?}", expected.board, expected.number);
    if rows.len() != actual.len() {
        return Err(format!(
            "{name}: {} rows expected, {} rows written\nexpected:\n{}\nwritten:\n{}",
            rows.len(),
            actual.len(),
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>().join("\n"),
            actual.iter().map(CellRow::text).collect::<Vec<_>>().join("\n"),
        ));
    }
    let same = |a: Ink, b: Ink| {
        let plain = |ink: Ink| {
            if depth == Depth::TrueColor && ink == expected.foreground { Ink::Default } else { ink }
        };
        plain(a) == plain(b)
    };
    for (index, (row, real)) in rows.iter().zip(&actual).enumerate() {
        let want = Frame::cells_of(row);
        let blank = Cell { text: ' ', look: Look::default() };
        let width = want.len().max(real.cells.len());
        let fill = |ink: Option<Ink>| ink.unwrap_or_default();
        let mut problem = (!same(fill(row.fill), fill(real.fill)))
            .then(|| format!("the fill is {}, the frame has {}", fill(real.fill), fill(row.fill)));
        for column in 0..width {
            if problem.is_some() {
                break;
            }
            let w = on_fill(want.get(column).copied().unwrap_or(blank), row.fill);
            let r = on_fill(real.cells.get(column).copied().unwrap_or(blank), real.fill);
            let space = w.text == ' ' && r.text == ' ';
            let matches = w.text == r.text
                && same(w.look.bg, r.look.bg)
                && (space
                    || same(w.look.fg, r.look.fg)
                        && Look { fg: Ink::Default, bg: Ink::Default, ..w.look }
                            == Look { fg: Ink::Default, bg: Ink::Default, ..r.look });
            if !matches {
                problem = Some(format!(
                    "column {column}: {:?} ({}) written, the frame has {:?} ({})",
                    r.text, r.look, w.text, w.look
                ));
            }
        }
        if let Some(problem) = problem {
            return Err(format!(
                "{name}, row {} of the check: {problem}\nexpected: {}\nwritten:  {}",
                index,
                row.text,
                real.text()
            ));
        }
    }
    Ok(())
}

/// `cell` with no background of its own when its background is the fill of its row.
fn on_fill(mut cell: Cell, fill: Option<Ink>) -> Cell {
    if fill == Some(cell.look.bg) {
        cell.look.bg = Ink::Default;
    }
    cell
}

/// [`check_rows`], as an assertion: a test fails with the first row that differs.
#[expect(clippy::panic, reason = "an assertion of a test fails by a panic")]
pub fn assert_rows(frame: &Frame, range: impl RangeBounds<usize>, real: &str, depth: Depth) {
    if let Err(problem) = check_rows(frame, range, real, depth) {
        panic!("the output differs from the design: {problem}");
    }
}

/// Every frame of `board` in the fixtures.
fn board_frames(board: &str) -> Result<Vec<Frame>, TestSupportError> {
    let path = PathBuf::from(FIXTURES).join(format!("{board}.jsonl"));
    let text = std::fs::read_to_string(&path)
        .map_err(|source| TestSupportError::ReadDesign { path: path.clone(), source })?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let written: WrittenFrame = serde_json::from_str(line)
                .map_err(|source| TestSupportError::DesignSyntax { path: path.clone(), source })?;
            written.frame().ok_or(TestSupportError::DesignSyntax {
                path: path.clone(),
                source: <serde_json::Error as serde::de::Error>::custom(
                    "a colour or a depth is not valid",
                ),
            })
        })
        .collect()
}

/// A frame as the fixture writes it.
#[derive(Deserialize)]
struct WrittenFrame {
    board: String,
    frame: u32,
    caption: Option<String>,
    cols: u16,
    depth: String,
    variant_of: Option<u32>,
    fg: String,
    rows: Vec<WrittenRow>,
}

#[derive(Deserialize)]
struct WrittenRow {
    live: bool,
    fill: Option<Value>,
    segs: Vec<WrittenSegment>,
}

#[derive(Deserialize)]
struct WrittenSegment {
    t: String,
    fg: Option<Value>,
    bg: Option<Value>,
    #[serde(default)]
    bold: bool,
    #[serde(default)]
    italic: bool,
    #[serde(default)]
    underline: bool,
    #[serde(default)]
    reverse: bool,
    #[serde(default)]
    dim: bool,
    #[serde(default)]
    strike: bool,
}

/// A colour as the fixture writes it: `#rrggbb`, or a slot as a number.
fn ink(value: Option<&Value>) -> Option<Ink> {
    match value {
        None | Some(Value::Null) => Some(Ink::Default),
        Some(Value::String(hex)) => rgb(hex).map(|(r, g, b)| Ink::Rgb(r, g, b)),
        Some(Value::Number(number)) => {
            number.as_u64().and_then(|slot| u8::try_from(slot).ok()).map(Ink::Slot)
        }
        Some(_) => None,
    }
}

impl WrittenFrame {
    fn frame(self) -> Option<Frame> {
        let mut rows = Vec::new();
        for row in self.rows {
            let mut segments = Vec::new();
            for seg in row.segs {
                let look = Look {
                    fg: ink(seg.fg.as_ref())?,
                    bg: ink(seg.bg.as_ref())?,
                    bold: seg.bold,
                    italic: seg.italic,
                    underline: seg.underline,
                    reverse: seg.reverse,
                    dim: seg.dim,
                    strike: seg.strike,
                };
                segments.push(Segment { text: seg.t, look });
            }
            let fill = match ink(row.fill.as_ref())? {
                Ink::Default => None,
                fill => Some(fill),
            };
            let text = segments.iter().map(|segment| segment.text.as_str()).collect();
            rows.push(Row { text, segments, fill, live: row.live });
        }
        Some(Frame {
            board: self.board,
            number: self.frame,
            caption: self.caption,
            cols: self.cols,
            depth: Depth::from_name(&self.depth)?,
            variant_of: self.variant_of,
            foreground: rgb(&self.fg).map(|(r, g, b)| Ink::Rgb(r, g, b))?,
            rows,
        })
    }
}

#[cfg(test)]
mod tests;
