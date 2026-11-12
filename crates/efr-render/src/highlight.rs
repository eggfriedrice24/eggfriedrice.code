//! Syntax highlighting through syntect, with two-face's grammars and themes (bat's
//! set). Both decompress on first use, or when a caller asks for them before
//! ([`crate::Preload`]) on a thread of its own, because `efr` is a short process per
//! prompt and a frame must not wait for the load. A grammar unpacks its contexts and
//! builds its patterns at its first line, 1 to 125 ms by the language.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use std::str::FromStr as _;

use syntect::highlighting::{
    Color, FontStyle, HighlightIterator, HighlightState, Highlighter, ScopeSelectors,
    StyleModifier, Theme as SyntectTheme, ThemeItem,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use two_face::theme::EmbeddedLazyThemeSet;

use crate::options::Theme;
use crate::style::{Colour, Style};

mod refine;

use refine::{Language, refine};

/// The embedded grammars and themes, loaded on first use, and one highlighter for each
/// embedded theme, made the first time a block uses that theme. A highlighter sorts
/// the selectors of its theme, so it is made once and not for each line.
pub(crate) struct Assets {
    syntaxes: OnceLock<SyntaxSet>,
    themes: OnceLock<EmbeddedLazyThemeSet>,
    highlighters: [OnceLock<Highlighter<'static>>; Theme::COUNT],
    /// The theme of code without colour, and its highlighter.
    no_colour: OnceLock<SyntectTheme>,
    no_colour_highlighter: OnceLock<Highlighter<'static>>,
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
        Assets {
            syntaxes: OnceLock::new(),
            themes: OnceLock::new(),
            highlighters: [const { OnceLock::new() }; Theme::COUNT],
            no_colour: OnceLock::new(),
            no_colour_highlighter: OnceLock::new(),
        }
    }

    fn syntaxes(&self) -> &SyntaxSet {
        self.syntaxes.get_or_init(two_face::syntax::extra_newlines)
    }

    pub(crate) fn theme(&self, theme: Theme) -> &SyntectTheme {
        self.themes.get_or_init(two_face::theme::extra).get(theme.embedded())
    }

    /// An embedded theme with the highlighter that all its blocks share.
    pub(crate) fn theme_ref(&'static self, theme: Theme) -> ThemeRef {
        let syntect = self.theme(theme);
        let highlighter =
            self.highlighters[theme.index()].get_or_init(|| Highlighter::new(syntect));
        ThemeRef { theme: syntect, highlighter }
    }

    /// The theme of code without colour (`NO_COLOR`): comments in italic, as the code
    /// table of the design has them, and nothing else, because italic is no colour.
    pub(crate) fn no_colour_theme(&'static self) -> ThemeRef {
        let theme = self.no_colour.get_or_init(no_colour_theme);
        let highlighter = self.no_colour_highlighter.get_or_init(|| Highlighter::new(theme));
        ThemeRef { theme, highlighter }
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

    /// Loads the grammars now, and the embedded `theme` with its highlighter when
    /// there is one, so a later code block finds them loaded.
    pub(crate) fn load(&'static self, theme: Option<Theme>) {
        self.syntaxes();
        if let Some(theme) = theme {
            self.theme_ref(theme);
        }
    }

    #[cfg(test)]
    pub(crate) fn grammars_loaded(&self) -> bool {
        self.syntaxes.get().is_some()
    }

    #[cfg(test)]
    pub(crate) fn highlighter_loaded(&self, theme: Theme) -> bool {
        self.highlighters[theme.index()].get().is_some()
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

/// The theme of a highlighted block, an embedded one or the user's own, with its
/// highlighter. Both stay until the process ends: [`Assets`] keeps the embedded ones,
/// and a [`CodeTheme`](crate::CodeTheme) keeps its own from the parse of its file.
#[derive(Clone, Copy)]
pub(crate) struct ThemeRef {
    pub(crate) theme: &'static SyntectTheme,
    pub(crate) highlighter: &'static Highlighter<'static>,
}

impl ThemeRef {
    pub(crate) fn get(&self) -> &'static SyntectTheme {
        self.theme
    }
}

impl fmt::Debug for ThemeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThemeRef").field("name", &self.theme.name).finish_non_exhaustive()
    }
}

/// The state of one highlighted run of lines: a grammar's parse state and a theme's
/// style stack, carried from line to line.
#[derive(Clone, Debug)]
pub(crate) struct Highlight {
    syntaxes: &'static SyntaxSet,
    theme: ThemeRef,
    syntax: &'static SyntaxReference,
    /// Made at the first line: it unpacks the grammar's contexts, the first part of
    /// the grammar's cost, so the label line of a block does not pay it.
    parse: Option<ParseState>,
    state: HighlightState,
    /// The language whose missing scopes [`refine`] adds, if it knows it.
    language: Option<Language>,
    /// No line is highlighted yet.
    first: bool,
    /// The scope stack of the grammar, without what [`refine`] changes.
    grammar: ScopeStack,
}

impl Highlight {
    pub(crate) fn new(
        assets: &'static Assets,
        syntax: &'static SyntaxReference,
        theme: ThemeRef,
    ) -> Highlight {
        let state = HighlightState::new(theme.highlighter, ScopeStack::new());
        let language = Language::of(syntax.scope);
        Highlight {
            syntaxes: assets.syntaxes(),
            theme,
            syntax,
            parse: None,
            state,
            language,
            first: true,
            grammar: ScopeStack::new(),
        }
    }

    /// Highlights the next line, given without its newline. `None` when the grammar
    /// fails; the caller shows the rest of the block plain.
    pub(crate) fn line(&mut self, line: &str) -> Option<Vec<(Style, String)>> {
        // The grammars are the "newlines" set, which expect each line to end in one.
        let text = format!("{line}\n");
        let syntax = self.syntax;
        let parse = self.parse.get_or_insert_with(|| ParseState::new(syntax));
        let mut ops = parse.parse_line(&text, self.syntaxes).ok()?;
        if let Some(language) = self.language {
            let before = self.grammar.clone();
            for (_, op) in &ops {
                self.grammar.apply(op).ok()?;
            }
            ops = refine(language, &before, self.first, &text, ops);
        }
        self.first = false;
        let highlighter = self.theme.highlighter;
        let mut tokens = Vec::new();
        for (style, piece) in HighlightIterator::new(&mut self.state, &ops, &text, highlighter) {
            let piece = piece.strip_suffix('\n').unwrap_or(piece);
            if !piece.is_empty() {
                tokens.push((style_of(style), piece.to_owned()));
            }
        }
        Some(tokens)
    }
}

/// The theme of [`Assets::no_colour_theme`]: the terminal's own colour (bat's alpha
/// of 1) and one rule, comments in italic.
fn no_colour_theme() -> SyntectTheme {
    let mut theme = SyntectTheme::default();
    theme.settings.foreground = Some(Color { r: 0, g: 0, b: 0, a: 1 });
    if let Ok(scope) = ScopeSelectors::from_str("comment") {
        let style =
            StyleModifier { font_style: Some(FontStyle::ITALIC), ..StyleModifier::default() };
        theme.scopes.push(ThemeItem { scope, style });
    }
    theme
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
