//! Styled text and how it becomes ANSI: SGR attributes, colours reduced to what the
//! colour mode allows, OSC 8 hyperlinks, and lines that open and close all of their
//! own state, so a caller can drop any line of the live zone without breaking the
//! ones after it.

use std::borrow::Cow;
use std::fmt::Write as _;

use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::options::ColourMode;

/// A colour: an entry of the terminal's palette or 24-bit RGB.
///
/// A palette entry follows the terminal's theme. An RGB colour is written as RGB in
/// [`ColourMode::TrueColor`], as the nearest of the 16 palette entries in
/// [`ColourMode::Ansi16`], and not at all in [`ColourMode::None`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Colour {
    /// A palette entry: 0 to 15 are the 16 named colours (0 black, 1 red, 2 green,
    /// 3 yellow, 4 blue, 5 magenta, 6 cyan, 7 white, then 8 to 15 their bright
    /// forms), 16 to 255 the xterm colour cube and grey ramp.
    Palette(u8),
    /// 24-bit colour: red, green and blue.
    Rgb(u8, u8, u8),
}

pub(crate) const RED: Colour = Colour::Palette(1);
pub(crate) const GREEN: Colour = Colour::Palette(2);
pub(crate) const YELLOW: Colour = Colour::Palette(3);
pub(crate) const BLUE: Colour = Colour::Palette(4);
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

    pub(crate) const fn bold(self) -> Style {
        Style { bold: true, ..self }
    }

    pub(crate) const fn dim(self) -> Style {
        Style { dim: true, ..self }
    }

    pub(crate) const fn italic(self) -> Style {
        Style { italic: true, ..self }
    }

    pub(crate) const fn underline(self) -> Style {
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
    /// The colour of the `text` role: every span without a colour of its own gets it.
    /// `None` leaves such spans in the terminal's default colour.
    text: Option<Colour>,
}

impl Painter {
    pub(crate) fn new(colour: ColourMode, hyperlinks: bool) -> Painter {
        Painter { colour, hyperlinks, text: None }
    }

    /// The painter with `text` as the colour of spans that have none of their own.
    pub(crate) fn with_text(self, text: Option<Colour>) -> Painter {
        Painter { text, ..self }
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

    /// The style with the text colour where it has no colour, and its colours reduced
    /// to what the mode allows.
    fn reduce(&self, style: Style) -> Style {
        let colour = |c: Option<Colour>| reduce_colour(c, self.colour);
        Style { fg: colour(style.fg.or(self.text)), bg: colour(style.bg), ..style }
    }
}

/// `colour` as the mode allows it: none without colour, one of the 16 palette entries
/// in 16-colour mode, unchanged in truecolor.
pub(crate) fn reduce_colour(colour: Option<Colour>, mode: ColourMode) -> Option<Colour> {
    match mode {
        ColourMode::None => None,
        ColourMode::Ansi16 => colour.map(to_ansi16),
        ColourMode::TrueColor => colour,
    }
}

/// `text` between the SGR sequence of `style` and a reset; `text` alone when the style
/// is plain or the text empty. `style` must already be reduced to the colour mode.
pub(crate) fn wrap_sgr(style: Style, text: &str) -> String {
    if style == Style::PLAIN || text.is_empty() {
        return text.to_owned();
    }
    let mut out = String::new();
    write_sgr(&mut out, style);
    out.push_str(text);
    out.push_str(RESET);
    out
}

/// The SGR parameters of `style`, such as `1;33`, without `ESC [` and `m`; `None` for
/// a plain style. `style` must already be reduced to the colour mode.
pub(crate) fn sgr_parameters(style: Style) -> Option<String> {
    let mut out = String::new();
    write_sgr(&mut out, style);
    out.strip_prefix("\x1b[").and_then(|rest| rest.strip_suffix('m')).map(str::to_owned)
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

/// The palette index nearest to an RGB colour. A colour with a clear hue (HSV
/// saturation over a quarter) takes the entry of its hue family (red, yellow, green,
/// cyan, blue or magenta), normal or bright, and any other colour takes a grey (0, 7, 8
/// or 15). By distance alone, the soft colours of a design system are nearest to grey
/// 8, and red, green and blue roles would all look the same. Inside the choice, the
/// "redmean" approximation of perceived distance decides.
fn nearest16((r, g, b): (u8, u8, u8)) -> u8 {
    let distance = |index: u8| {
        let (pr, pg, pb) = XTERM16[usize::from(index)];
        let mean = (i32::from(r) + i32::from(pr)) / 2;
        let dr = i32::from(r) - i32::from(pr);
        let dg = i32::from(g) - i32::from(pg);
        let db = i32::from(b) - i32::from(pb);
        (((512 + mean) * dr * dr) >> 8) + 4 * dg * dg + (((767 - mean) * db * db) >> 8)
    };
    match hue_family((r, g, b)) {
        Some(normal) if distance(normal + 8) < distance(normal) => normal + 8,
        Some(normal) => normal,
        None => [0, 7, 8, 15].into_iter().min_by_key(|index| distance(*index)).unwrap_or(0),
    }
}

/// The normal palette entry (1 to 6) of the hue of an RGB colour, or `None` when the
/// colour has too little hue to have one.
fn hue_family((r, g, b): (u8, u8, u8)) -> Option<u8> {
    /// The entries of the hue families in the order of the colour wheel, from red.
    const FAMILIES: [u8; 6] = [1, 3, 2, 6, 4, 5];
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    let max = r.max(g).max(b);
    let range = max - r.min(g).min(b);
    if range * 4 <= max {
        return None;
    }
    let hue = if max == r {
        60 * (g - b) / range
    } else if max == g {
        120 + 60 * (b - r) / range
    } else {
        240 + 60 * (r - g) / range
    };
    let family = usize::try_from((hue + 360 + 30) / 60 % 6).unwrap_or(0);
    FAMILIES.get(family).copied()
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

#[cfg(test)]
mod tests;
