//! Syntax highlighting through syntect, with two-face's grammars and themes (bat's
//! set). Both decompress on first use, because `efr` is a short process per prompt
//! and most replies have no code block.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use syntect::highlighting::{
    Color, FontStyle, HighlightIterator, HighlightState, Highlighter, Theme as SyntectTheme,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use two_face::theme::EmbeddedLazyThemeSet;

use crate::options::Theme;
use crate::style::{Colour, Style};

/// The embedded grammars and themes, loaded on first use.
pub(crate) struct Assets {
    syntaxes: OnceLock<SyntaxSet>,
    themes: OnceLock<EmbeddedLazyThemeSet>,
}

/// The process-wide assets every renderer shares.
pub(crate) static ASSETS: Assets = Assets::new();

/// Fence labels that name a language differently from its grammar.
const ALIASES: &[(&str, &str)] = &[
    ("shell", "bash"),
    ("console", "bash"),
    ("shellsession", "bash"),
    ("sh-session", "bash"),
    ("terminal", "bash"),
    ("zsh", "bash"),
    ("jsonc", "json"),
    ("ndjson", "json"),
    ("dockerfile", "Dockerfile"),
    ("docker", "Dockerfile"),
];

impl Assets {
    pub(crate) const fn new() -> Assets {
        Assets { syntaxes: OnceLock::new(), themes: OnceLock::new() }
    }

    fn syntaxes(&self) -> &SyntaxSet {
        self.syntaxes.get_or_init(two_face::syntax::extra_newlines)
    }

    pub(crate) fn theme(&self, theme: Theme) -> &SyntectTheme {
        self.themes.get_or_init(two_face::theme::extra).get(theme.embedded())
    }

    /// The grammar for a fence label such as `rust`, `rs` or `shell`.
    pub(crate) fn syntax_for_label(&self, label: &str) -> Option<&SyntaxReference> {
        let label = label.to_ascii_lowercase();
        let token = ALIASES
            .iter()
            .find(|(alias, _)| *alias == label)
            .map_or(label.as_str(), |(_, target)| target);
        let syntaxes = self.syntaxes();
        syntaxes.find_syntax_by_token(token).filter(|syntax| !is_plain_text(syntaxes, syntax))
    }

    /// The grammar for a file path in a diff header, by extension and then by the
    /// whole file name (`Makefile`, `Dockerfile`). The file is never opened.
    pub(crate) fn syntax_for_path(&self, path: &str) -> Option<&SyntaxReference> {
        let path = Path::new(path);
        let syntaxes = self.syntaxes();
        let by_extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .and_then(|extension| syntaxes.find_syntax_by_extension(extension));
        by_extension
            .or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| syntaxes.find_syntax_by_extension(name))
            })
            .filter(|syntax| !is_plain_text(syntaxes, syntax))
    }

    #[cfg(test)]
    pub(crate) fn grammars_loaded(&self) -> bool {
        self.syntaxes.get().is_some()
    }
}

impl fmt::Debug for Assets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Assets")
            .field("grammars_loaded", &self.syntaxes.get().is_some())
            .field("themes_loaded", &self.themes.get().is_some())
            .finish()
    }
}

fn is_plain_text(syntaxes: &SyntaxSet, syntax: &SyntaxReference) -> bool {
    std::ptr::eq(syntax, syntaxes.find_syntax_plain_text())
}

/// The state of one highlighted run of lines: a grammar's parse state and a theme's
/// style stack, carried from line to line.
#[derive(Clone, Debug)]
pub(crate) struct Highlight {
    syntaxes: &'static SyntaxSet,
    theme: &'static SyntectTheme,
    parse: ParseState,
    state: HighlightState,
}

impl Highlight {
    pub(crate) fn new(
        assets: &'static Assets,
        syntax: &SyntaxReference,
        theme: Theme,
    ) -> Highlight {
        let theme = assets.theme(theme);
        let highlighter = Highlighter::new(theme);
        Highlight {
            syntaxes: assets.syntaxes(),
            theme,
            parse: ParseState::new(syntax),
            state: HighlightState::new(&highlighter, ScopeStack::new()),
        }
    }

    /// Highlights the next line, given without its newline. `None` when the grammar
    /// fails; the caller shows the rest of the block plain.
    pub(crate) fn line(&mut self, line: &str) -> Option<Vec<(Style, String)>> {
        // The grammars are the "newlines" set, which expect each line to end in one.
        let text = format!("{line}\n");
        let ops = self.parse.parse_line(&text, self.syntaxes).ok()?;
        let highlighter = Highlighter::new(self.theme);
        let mut tokens = Vec::new();
        for (style, piece) in HighlightIterator::new(&mut self.state, &ops, &text, &highlighter) {
            let piece = piece.strip_suffix('\n').unwrap_or(piece);
            if !piece.is_empty() {
                tokens.push((style_of(style), piece.to_owned()));
            }
        }
        Some(tokens)
    }
}

/// A theme style as a [`Style`]. The theme's background is ignored, so code sits on
/// the terminal's own background.
fn style_of(style: syntect::highlighting::Style) -> Style {
    Style {
        fg: theme_colour(style.foreground),
        bold: style.font_style.contains(FontStyle::BOLD),
        italic: style.font_style.contains(FontStyle::ITALIC),
        underline: style.font_style.contains(FontStyle::UNDERLINE),
        ..Style::PLAIN
    }
}

/// bat's encoding: alpha 0 means "palette entry `r`", alpha 1 means "the terminal's
/// default colour", anything else is RGB.
pub(crate) fn theme_colour(colour: Color) -> Option<Colour> {
    match colour.a {
        0 => Some(Colour::Palette(colour.r)),
        1 => None,
        _ => Some(Colour::Rgb(colour.r, colour.g, colour.b)),
    }
}

/// A truecolor theme's background, for tinting diff lines to match it.
pub(crate) fn theme_background(theme: &SyntectTheme) -> Option<(u8, u8, u8)> {
    theme
        .settings
        .background
        .filter(|colour| colour.a > 1)
        .map(|colour| (colour.r, colour.g, colour.b))
}

#[cfg(test)]
mod tests;
