//! What the caller decides about the output. The CLI reads `NO_COLOR`, `TERM`,
//! `COLORTERM`, the config and whether stdout is a terminal, and passes the result
//! here; this crate reads no environment itself.

use std::fmt;
use std::str::FromStr;

use two_face::theme::EmbeddedThemeName;

use crate::code_theme::CodeTheme;
use crate::error::RenderError;
use crate::palette::{Palette, Role};
use crate::style::{Style, sgr_parameters, wrap_sgr};
use crate::width::WidthMethod;

/// The width used when the caller passes 0, which is what a terminal size query
/// returns when it fails.
const FALLBACK_WIDTH: u16 = 80;

/// Indented blocks keep at least this many columns for their text, so a deep list in
/// a narrow terminal still wraps into readable lines.
pub(crate) const MIN_TEXT_WIDTH: usize = 10;

/// How many colours the output may use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColourMode {
    /// No colour, as `NO_COLOR` asks. Bold, italic, dim, underline, strikethrough and
    /// hyperlinks stay, because they are not colour; inline code keeps its backticks
    /// so it stays visible.
    None,
    /// The terminal's 16-colour palette. Theme colours outside the palette are mapped
    /// to the nearest entry.
    #[default]
    Ansi16,
    /// 24-bit colour: a truecolor theme is written as RGB.
    TrueColor,
}

/// A syntax highlighting theme from bat's set, embedded through `two-face`.
///
/// The default is `ansi`, which uses the terminal's own 16 colours, so code follows
/// the Ghostty theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Theme(EmbeddedThemeName);

/// Canonical names in kebab case, in the order `two-face` lists its themes.
const THEMES: &[(&str, EmbeddedThemeName)] = &[
    ("ansi", EmbeddedThemeName::Ansi),
    ("base16", EmbeddedThemeName::Base16),
    ("base16-eighties-dark", EmbeddedThemeName::Base16EightiesDark),
    ("base16-mocha-dark", EmbeddedThemeName::Base16MochaDark),
    ("base16-ocean-dark", EmbeddedThemeName::Base16OceanDark),
    ("base16-ocean-light", EmbeddedThemeName::Base16OceanLight),
    ("base16-256", EmbeddedThemeName::Base16_256),
    ("catppuccin-frappe", EmbeddedThemeName::CatppuccinFrappe),
    ("catppuccin-latte", EmbeddedThemeName::CatppuccinLatte),
    ("catppuccin-macchiato", EmbeddedThemeName::CatppuccinMacchiato),
    ("catppuccin-mocha", EmbeddedThemeName::CatppuccinMocha),
    ("coldark-cold", EmbeddedThemeName::ColdarkCold),
    ("coldark-dark", EmbeddedThemeName::ColdarkDark),
    ("dark-neon", EmbeddedThemeName::DarkNeon),
    ("dracula", EmbeddedThemeName::Dracula),
    ("github", EmbeddedThemeName::Github),
    ("gruvbox-dark", EmbeddedThemeName::GruvboxDark),
    ("gruvbox-light", EmbeddedThemeName::GruvboxLight),
    ("inspired-github", EmbeddedThemeName::InspiredGithub),
    ("1337", EmbeddedThemeName::Leet),
    ("monokai-extended", EmbeddedThemeName::MonokaiExtended),
    ("monokai-extended-bright", EmbeddedThemeName::MonokaiExtendedBright),
    ("monokai-extended-light", EmbeddedThemeName::MonokaiExtendedLight),
    ("monokai-extended-origin", EmbeddedThemeName::MonokaiExtendedOrigin),
    ("nord", EmbeddedThemeName::Nord),
    ("one-half-dark", EmbeddedThemeName::OneHalfDark),
    ("one-half-light", EmbeddedThemeName::OneHalfLight),
    ("solarized-dark", EmbeddedThemeName::SolarizedDark),
    ("solarized-light", EmbeddedThemeName::SolarizedLight),
    ("sublime-snazzy", EmbeddedThemeName::SublimeSnazzy),
    ("two-dark", EmbeddedThemeName::TwoDark),
    ("zenburn", EmbeddedThemeName::Zenburn),
];

impl Theme {
    /// The theme that uses the terminal's 16-colour palette.
    pub const ANSI: Theme = Theme(EmbeddedThemeName::Ansi);

    /// Finds a theme by name. Case, spaces and punctuation do not matter, so
    /// `catppuccin-mocha`, `Catppuccin Mocha` and bat's `Solarized (dark)` all match.
    pub fn from_name(name: &str) -> Result<Theme, RenderError> {
        let wanted = squash(name);
        THEMES
            .iter()
            .find(|(canonical, embedded)| {
                squash(canonical) == wanted || squash(embedded.as_name()) == wanted
            })
            .map(|&(_, embedded)| Theme(embedded))
            .ok_or_else(|| RenderError::UnknownTheme { name: name.to_owned() })
    }

    /// The canonical name, which [`Theme::from_name`] accepts.
    pub fn name(self) -> &'static str {
        THEMES.iter().find(|(_, embedded)| *embedded == self.0).map_or("ansi", |(name, _)| name)
    }

    /// The canonical names of every embedded theme.
    pub fn names() -> impl Iterator<Item = &'static str> {
        THEMES.iter().map(|(name, _)| *name)
    }

    pub(crate) fn embedded(self) -> EmbeddedThemeName {
        self.0
    }

    /// True for the themes that encode terminal palette entries instead of RGB, so
    /// they follow the terminal's colours and have no background of their own.
    pub(crate) fn uses_palette(self) -> bool {
        matches!(
            self.0,
            EmbeddedThemeName::Ansi | EmbeddedThemeName::Base16 | EmbeddedThemeName::Base16_256
        )
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::ANSI
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Theme {
    type Err = RenderError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Theme::from_name(name)
    }
}

/// Lower-case letters and digits only, so names compare without their punctuation.
fn squash(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// How to render: the width of the terminal, the colours it may use, the colour of
/// each role, the theme for code, whether to emit OSC 8 hyperlinks, how the terminal
/// counts widths, and whether the output is a terminal.
///
/// The CLI builds this from the environment and the config: `NO_COLOR` selects
/// [`ColourMode::None`]; a pipe, a file or `TERM=dumb` clears
/// [`is_terminal`](RenderOptions::is_terminal), which turns formatting off entirely and
/// passes the markdown through unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderOptions {
    width: u16,
    colour: ColourMode,
    palette: Palette,
    theme: Theme,
    code_theme: Option<CodeTheme>,
    hyperlinks: bool,
    width_method: WidthMethod,
    terminal: bool,
}

impl RenderOptions {
    /// Options for a terminal `width` columns wide with the defaults: 16 colours, the
    /// default palette, the `ansi` theme, hyperlinks on, widths by code point. A width
    /// of 0 means unknown and renders at 80.
    pub fn new(width: u16) -> Self {
        RenderOptions {
            width,
            colour: ColourMode::default(),
            palette: Palette::default(),
            theme: Theme::default(),
            code_theme: None,
            hyperlinks: true,
            width_method: WidthMethod::default(),
            terminal: true,
        }
    }

    /// The same options for a terminal `width` columns wide, after a resize.
    #[must_use]
    pub fn with_width(mut self, width: u16) -> Self {
        self.width = width;
        self
    }

    /// Sets how many colours the output may use.
    #[must_use]
    pub fn with_colour(mut self, colour: ColourMode) -> Self {
        self.colour = colour;
        self
    }

    /// Sets the colour of each role.
    #[must_use]
    pub fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }

    /// Sets the embedded theme for code blocks and diffs. A
    /// [`code theme`](Self::with_code_theme) wins over it.
    #[must_use]
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Sets a code theme from a `.tmTheme` file, or with `None` removes it. While one
    /// is set, code blocks and diffs use it, not the [`theme`](Self::theme).
    #[must_use]
    pub fn with_code_theme(mut self, code_theme: Option<CodeTheme>) -> Self {
        self.code_theme = code_theme;
        self
    }

    /// Sets how the terminal counts the width of text: by code point (most terminals,
    /// tmux) or by grapheme cluster (Ghostty outside tmux). The row count of the live
    /// zone and the cut of a trace line use it.
    #[must_use]
    pub fn with_width_method(mut self, method: WidthMethod) -> Self {
        self.width_method = method;
        self
    }

    /// Turns OSC 8 hyperlinks on or off. With them off, a link shows its target in
    /// parentheses after its text.
    #[must_use]
    pub fn with_hyperlinks(mut self, on: bool) -> Self {
        self.hyperlinks = on;
        self
    }

    /// Says whether the output is a terminal. When it is not, the renderer writes the
    /// markdown unchanged and has no live zone.
    #[must_use]
    pub fn with_terminal(mut self, is_terminal: bool) -> Self {
        self.terminal = is_terminal;
        self
    }

    /// The width in columns that rendering uses: the given width, or 80 when it was 0.
    pub fn width(&self) -> u16 {
        if self.width == 0 { FALLBACK_WIDTH } else { self.width }
    }

    /// How many colours the output may use.
    pub fn colour(&self) -> ColourMode {
        self.colour
    }

    /// The colour of each role.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The embedded theme for code blocks and diffs, used when there is no
    /// [`code_theme`](Self::code_theme).
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// The code theme from a `.tmTheme` file, when there is one.
    pub fn code_theme(&self) -> Option<&CodeTheme> {
        self.code_theme.as_ref()
    }

    /// Whether links are written as OSC 8 hyperlinks.
    pub fn hyperlinks(&self) -> bool {
        self.hyperlinks
    }

    /// How the terminal counts the width of text.
    pub fn width_method(&self) -> WidthMethod {
        self.width_method
    }

    /// Whether the output is a terminal.
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// `text` in the colour and attributes of `role`, followed by a reset, for the
    /// CLI's own lines. `text` stays as it is when the output is not a terminal or the
    /// role is plain here (`text` in the default palette, `code` without colour). The
    /// caller makes `text` safe first: control characters in it reach the terminal.
    pub fn paint(&self, role: Role, text: &str) -> String {
        if !self.terminal {
            return text.to_owned();
        }
        wrap_sgr(self.role_style(role), text)
    }

    /// `text` in the colour of `role` alone, followed by a reset: without the attributes
    /// that the role always has, such as the bold of `warning`. Without a colour, the
    /// attributes that the role has in place of one, such as the bold of `error`. For a
    /// gauge whose colour changes with its level and must not shout. `text` stays as it
    /// is when the output is not a terminal; the caller makes it safe first.
    pub fn tint(&self, role: Role, text: &str) -> String {
        if !self.terminal {
            return text.to_owned();
        }
        wrap_sgr(self.palette.tint(role, self.colour), text)
    }

    /// The SGR parameters of `role`, such as `2` or `1;33`, to write as
    /// `ESC [ <parameters> m` before text in that role; `None` when the role is plain
    /// here or the output is not a terminal.
    pub fn sgr(&self, role: Role) -> Option<String> {
        if !self.terminal {
            return None;
        }
        sgr_parameters(self.role_style(role))
    }

    /// The style of `role` in this colour mode and palette.
    pub(crate) fn role_style(&self, role: Role) -> Style {
        self.palette.style(role, self.colour)
    }

    pub(crate) fn columns(&self) -> usize {
        usize::from(self.width())
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions::new(FALLBACK_WIDTH)
    }
}

#[cfg(test)]
mod tests;
