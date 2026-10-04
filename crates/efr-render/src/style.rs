//! Styled text and how it becomes ANSI: SGR attributes, colours reduced to what the
//! colour mode allows, OSC 8 hyperlinks, and lines that open and close all of their
//! own state, so a caller can drop any line of the live zone without breaking the
//! ones after it.

use std::borrow::Cow;
use std::fmt::Write as _;

use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::options::ColourMode;

/// A colour as a theme or this crate names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Colour {
    /// A palette entry: 0 to 15 are the 16 named colours, 16 to 255 the xterm colour
    /// cube and grey ramp.
    Palette(u8),
    /// 24-bit colour.
    Rgb(u8, u8, u8),
}

pub(crate) const RED: Colour = Colour::Palette(1);
pub(crate) const GREEN: Colour = Colour::Palette(2);
pub(crate) const YELLOW: Colour = Colour::Palette(3);
pub(crate) const BLUE: Colour = Colour::Palette(4);
pub(crate) const MAGENTA: Colour = Colour::Palette(5);
pub(crate) const CYAN: Colour = Colour::Palette(6);

/// SGR attributes and colours of a piece of text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) fg: Option<Colour>,
    pub(crate) bg: Option<Colour>,
    pub(crate) bold: bool,
    pub(crate) dim: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) strike: bool,
}

impl Style {
    pub(crate) const PLAIN: Style = Style {
        fg: None,
        bg: None,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        strike: false,
    };

    pub(crate) fn fg(colour: Colour) -> Style {
        Style { fg: Some(colour), ..Style::PLAIN }
    }

    pub(crate) fn bold(self) -> Style {
        Style { bold: true, ..self }
    }

    pub(crate) fn dim(self) -> Style {
        Style { dim: true, ..self }
    }

    pub(crate) fn italic(self) -> Style {
        Style { italic: true, ..self }
    }

    pub(crate) fn underline(self) -> Style {
        Style { underline: true, ..self }
    }

    pub(crate) fn strike(self) -> Style {
        Style { strike: true, ..self }
    }

    pub(crate) fn on(self, colour: Colour) -> Style {
        Style { bg: Some(colour), ..self }
    }

    /// `self` with every attribute `over` sets added and `over`'s colours, where it
    /// has them, taking precedence.
    pub(crate) fn patch(self, over: Style) -> Style {
        Style {
            fg: over.fg.or(self.fg),
            bg: over.bg.or(self.bg),
            bold: self.bold || over.bold,
            dim: self.dim || over.dim,
            italic: self.italic || over.italic,
            underline: self.underline || over.underline,
            strike: self.strike || over.strike,
        }
    }

    /// Trailing spaces in this style can be dropped at the end of a line without a
    /// visible change.
    fn space_is_invisible(self) -> bool {
        self.bg.is_none() && !self.underline && !self.strike
    }
}

/// A run of text in one style, optionally a hyperlink to `link` (already encoded for
/// OSC 8, see `link::encode`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) text: String,
    pub(crate) style: Style,
    pub(crate) link: Option<String>,
}

impl Span {
    pub(crate) fn new(text: impl Into<String>, style: Style) -> Span {
        Span { text: text.into(), style, link: None }
    }

    pub(crate) fn plain(text: impl Into<String>) -> Span {
        Span::new(text, Style::PLAIN)
    }

    pub(crate) fn linked(text: impl Into<String>, style: Style, link: Option<String>) -> Span {
        Span { text: text.into(), style, link }
    }

    pub(crate) fn width(&self) -> usize {
        self.text.width()
    }
}

/// One output line before it is painted.
pub(crate) type Line = Vec<Span>;

pub(crate) fn line_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// The plain text of some spans, for comparisons such as "is the link text its URL".
pub(crate) fn line_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// Appends `span` to `line`, merging it into the last span when both look the same.
pub(crate) fn push_span(line: &mut Line, span: Span) {
    if span.text.is_empty() {
        return;
    }
    if let Some(last) = line.last_mut()
        && last.style == span.style
        && last.link == span.link
    {
        last.text.push_str(&span.text);
        return;
    }
    line.push(span);
}

const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";
const RESET: &str = "\x1b[0m";

/// Turns lines of spans into ANSI text for one colour mode.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Painter {
    colour: ColourMode,
    hyperlinks: bool,
}

impl Painter {
    pub(crate) fn new(colour: ColourMode, hyperlinks: bool) -> Painter {
        Painter { colour, hyperlinks }
    }

    /// Writes one line and its newline. The line starts with no SGR state and no open
    /// hyperlink and ends the same way.
    pub(crate) fn paint(&self, out: &mut String, spans: &[Span]) {
        let end = visible_end(spans);
        let mut current = Style::PLAIN;
        let mut link: Option<&str> = None;
        for (index, span) in spans.iter().enumerate().take(end.0) {
            let text = if index + 1 == end.0 { &span.text[..end.1] } else { span.text.as_str() };
            if text.is_empty() {
                continue;
            }
            let target = if self.hyperlinks { span.link.as_deref() } else { None };
            if target != link {
                if link.is_some() {
                    out.push_str(OSC8_CLOSE);
                }
                if let Some(target) = target {
                    let _ = write!(out, "\x1b]8;;{target}\x1b\\");
                }
                link = target;
            }
            let style = self.reduce(span.style);
            if style != current {
                if current != Style::PLAIN {
                    out.push_str(RESET);
                }
                write_sgr(out, style);
                current = style;
            }
            out.push_str(text);
        }
        if current != Style::PLAIN {
            out.push_str(RESET);
        }
        if link.is_some() {
            out.push_str(OSC8_CLOSE);
        }
        out.push('\n');
    }

    /// The style with its colours reduced to what the mode allows.
    fn reduce(&self, style: Style) -> Style {
        let colour = |c: Option<Colour>| match self.colour {
            ColourMode::None => None,
            ColourMode::Ansi16 => c.map(to_ansi16),
            ColourMode::TrueColor => c,
        };
        Style { fg: colour(style.fg), bg: colour(style.bg), ..style }
    }
}

/// Where a line's visible content ends: the number of spans to write and the byte
/// length of the last one, after dropping trailing spaces nobody can see.
fn visible_end(spans: &[Span]) -> (usize, usize) {
    for (index, span) in spans.iter().enumerate().rev() {
        let kept = if span.style.space_is_invisible() {
            span.text.trim_end_matches(' ').len()
        } else {
            span.text.len()
        };
        if kept > 0 {
            return (index + 1, kept);
        }
    }
    (0, 0)
}

fn write_sgr(out: &mut String, style: Style) {
    if style == Style::PLAIN {
        return;
    }
    let mut codes: Vec<String> = Vec::new();
    for (on, code) in [
        (style.bold, "1"),
        (style.dim, "2"),
        (style.italic, "3"),
        (style.underline, "4"),
        (style.strike, "9"),
    ] {
        if on {
            codes.push(code.to_owned());
        }
    }
    if let Some(fg) = style.fg {
        codes.push(colour_code(fg, false));
    }
    if let Some(bg) = style.bg {
        codes.push(colour_code(bg, true));
    }
    let _ = write!(out, "\x1b[{}m", codes.join(";"));
}

fn colour_code(colour: Colour, background: bool) -> String {
    let base = if background { 40 } else { 30 };
    match colour {
        Colour::Palette(index @ 0..=7) => (base + u16::from(index)).to_string(),
        Colour::Palette(index @ 8..=15) => (base + 60 + u16::from(index - 8)).to_string(),
        Colour::Palette(index) => format!("{};5;{index}", base + 8),
        Colour::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
    }
}

/// The xterm defaults for the 16 named colours, used only to pick the nearest entry
/// for a colour outside them; the terminal shows its own palette.
const XTERM16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

pub(crate) fn to_ansi16(colour: Colour) -> Colour {
    match colour {
        Colour::Palette(index) if index < 16 => colour,
        other => Colour::Palette(nearest16(rgb_of(other))),
    }
}

fn rgb_of(colour: Colour) -> (u8, u8, u8) {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match colour {
        Colour::Rgb(r, g, b) => (r, g, b),
        Colour::Palette(index @ 0..=15) => XTERM16[usize::from(index)],
        Colour::Palette(index @ 16..=231) => {
            let cube = index - 16;
            (
                LEVELS[usize::from(cube / 36)],
                LEVELS[usize::from(cube / 6 % 6)],
                LEVELS[usize::from(cube % 6)],
            )
        }
        Colour::Palette(index) => {
            let grey = 8 + 10 * (index - 232);
            (grey, grey, grey)
        }
    }
}

/// The palette index nearest to an RGB colour, by the "redmean" approximation of
/// perceived distance.
fn nearest16((r, g, b): (u8, u8, u8)) -> u8 {
    let distance = |(pr, pg, pb): (u8, u8, u8)| {
        let mean = (i32::from(r) + i32::from(pr)) / 2;
        let dr = i32::from(r) - i32::from(pr);
        let dg = i32::from(g) - i32::from(pg);
        let db = i32::from(b) - i32::from(pb);
        (((512 + mean) * dr * dr) >> 8) + 4 * dg * dg + (((767 - mean) * db * db) >> 8)
    };
    let mut best = 0_u8;
    let mut best_distance = i32::MAX;
    for (index, candidate) in (0_u8..).zip(XTERM16) {
        let d = distance(candidate);
        if d < best_distance {
            best = index;
            best_distance = d;
        }
    }
    best
}

/// Replaces control characters with visible stand-ins. Markdown from a model can
/// carry text from files and web pages; an escape sequence in it must not move the
/// cursor, set the title or write the clipboard. A tab becomes one space.
pub(crate) fn sanitize(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|c| match c {
                '\t' => ' ',
                '\u{0}'..='\u{1f}' => char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'),
                '\u{7f}' => '\u{2421}',
                c if c.is_control() => '\u{fffd}',
                c => c,
            })
            .collect(),
    )
}

/// Expands tabs to the next multiple of four columns, for code, where alignment
/// matters and the terminal's tab stops would not know about a line's prefix.
pub(crate) fn expand_tabs(text: &str) -> Cow<'_, str> {
    if !text.contains('\t') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut column = 0;
    for c in text.chars() {
        if c == '\t' {
            let spaces = 4 - column % 4;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(c);
            column += c.width().unwrap_or(0);
        }
    }
    Cow::Owned(out)
}

/// The number of columns painted text takes, skipping CSI and OSC sequences.
pub(crate) fn display_width(painted: &str) -> usize {
    let mut width = 0;
    let mut chars = painted.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            width += c.width().unwrap_or(0);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    width
}

#[cfg(test)]
mod tests;
