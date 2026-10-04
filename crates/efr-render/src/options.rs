//! What the caller decides about the output. The CLI reads `NO_COLOR`, `TERM`,
//! `COLORTERM`, the config and whether stdout is a terminal, and passes the result
//! here; this crate reads no environment itself.

use std::fmt;
use std::str::FromStr;

use two_face::theme::EmbeddedThemeName;

use crate::error::RenderError;

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

/// How to render: the width of the terminal, the colours it may use, the theme for
/// code, whether to emit OSC 8 hyperlinks, and whether the output is a terminal.
///
/// The CLI builds this from the environment: `NO_COLOR` selects
/// [`ColourMode::None`]; a pipe, a file or `TERM=dumb` clears
/// [`is_terminal`](RenderOptions::is_terminal), which turns formatting off entirely and
/// passes the markdown through unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderOptions {
    width: u16,
    colour: ColourMode,
    theme: Theme,
    hyperlinks: bool,
    terminal: bool,
}

impl RenderOptions {
    /// Options for a terminal `width` columns wide with the defaults: 16 colours, the
    /// `ansi` theme, hyperlinks on. A width of 0 means unknown and renders at 80.
    pub fn new(width: u16) -> Self {
        RenderOptions {
            width,
            colour: ColourMode::default(),
            theme: Theme::default(),
            hyperlinks: true,
            terminal: true,
        }
    }

    /// Sets how many colours the output may use.
    #[must_use]
    pub fn with_colour(mut self, colour: ColourMode) -> Self {
        self.colour = colour;
        self
    }

    /// Sets the theme for code blocks and diffs.
    #[must_use]
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
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

    /// The theme for code blocks and diffs.
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// Whether links are written as OSC 8 hyperlinks.
    pub fn hyperlinks(&self) -> bool {
        self.hyperlinks
    }

    /// Whether the output is a terminal.
    pub fn is_terminal(&self) -> bool {
        self.terminal
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
