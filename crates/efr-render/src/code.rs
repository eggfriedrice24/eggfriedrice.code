//! Code blocks: a muted label line when it tells the reader something, then one output
//! line per source line with syntax colours, never wrapped. A block renders one line
//! at a time, so the streaming renderer commits each line as soon as it is complete,
//! and a block streamed in pieces renders exactly as the same block rendered at once.
//!
//! The label is the language and the file when the fence names both
//! (`rust · src/parse.rs`), the language alone, or the file alone (`title="x"`,
//! `file=x`). A short table names a language by its common word (`ts` gives
//! `typescript`). Plain text and diffs (`text`, `txt`, `plain`, `plaintext`, `diff`,
//! `patch`, `udiff` or no info) show no label. No frame and no indent: the user copies
//! code with the mouse. In truecolor each row of code is on the `code.bg` role (an
//! added or a removed row of a diff on `diff.add.bg` or `diff.remove.bg`), which the
//! caller paints to the end of the row with `ESC [ K` ([`CodeBlock::fill`]). Without
//! colour the fences come back, dim (` ```rust ` and ` ``` `), so the block still reads
//! as code and a copy is Markdown; a comment stays italic (TM-Code).

mod diff;

use crate::code_theme::CodeTheme;
use crate::glyph::Glyphs;
use crate::highlight::{ASSETS, Assets, Highlight, ThemeRef};
use crate::options::{ColourMode, RenderOptions, Theme};
use crate::palette::Role;
use crate::style::{Colour, Line, Span, Style, expand_tabs, push_span, sanitize};

use diff::Diff;
pub(crate) use diff::Kind as DiffKind;

/// Fence labels that mark a unified diff.
const DIFF_LABELS: &[&str] = &["diff", "patch", "udiff"];

/// Fence labels that say nothing a reader needs: plain text and diffs show no label.
const SILENT_LABELS: &[&str] = &["text", "txt", "plain", "plaintext", "diff", "patch", "udiff"];

/// The fence of a code block, as it comes back without colour.
const FENCE: &str = "```";

/// Short fence words and the language that the label names for them.
const LANGUAGE_NAMES: &[(&str, &str)] = &[
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("kt", "kotlin"),
    ("md", "markdown"),
    ("py", "python"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("yml", "yaml"),
];

/// What a code block needs from the options.
#[derive(Clone, Debug)]
pub(crate) struct CodeStyle {
    pub(crate) colour: ColourMode,
    pub(crate) theme: Theme,
    pub(crate) code_theme: Option<CodeTheme>,
    pub(crate) assets: &'static Assets,
    /// The label line: the `muted` role.
    pub(crate) label: Style,
    /// Added and removed lines and hunk headers of a diff: the `diff.*` roles.
    pub(crate) added: Style,
    pub(crate) removed: Style,
    pub(crate) hunk: Style,
    /// The backgrounds of added and removed rows that the palette sets
    /// (`diff.add.bg`, `diff.remove.bg`), in truecolor only.
    pub(crate) added_bg: Option<Colour>,
    pub(crate) removed_bg: Option<Colour>,
    /// The background of every other row of code (`code.bg`), in truecolor only.
    pub(crate) code_bg: Option<Colour>,
    /// The separator between the language and the file of a label.
    pub(crate) glyphs: &'static Glyphs,
}

impl CodeStyle {
    pub(crate) fn new(options: &RenderOptions, assets: &'static Assets) -> CodeStyle {
        // NOTE: in 16 colours a theme in bat's ANSI encoding names the slot of each
        // token kind; the nearest slot of an RGB theme is often another one.
        let code_theme = match (options.colour(), options.code_theme_ansi()) {
            (ColourMode::Ansi16, Some(ansi)) => Some(ansi.clone()),
            _ => options.code_theme().cloned(),
        };
        let palette = options.palette();
        CodeStyle {
            colour: options.colour(),
            theme: options.theme(),
            code_theme,
            assets,
            label: options.role_style(Role::Muted),
            added: options.role_style(Role::DiffAdd),
            removed: options.role_style(Role::DiffRemove),
            hunk: options.role_style(Role::DiffHunk),
            added_bg: palette.background(Role::DiffAddBg, options.colour()),
            removed_bg: palette.background(Role::DiffRemoveBg, options.colour()),
            code_bg: palette.background(Role::CodeBg, options.colour()),
            glyphs: options.glyphs(),
        }
    }

    /// The theme of highlighted code: the user's code theme, else the embedded one.
    /// Without colour, a theme that makes comments italic and nothing else.
    pub(crate) fn theme_ref(&self) -> ThemeRef {
        match &self.code_theme {
            _ if self.colour == ColourMode::None => self.assets.no_colour_theme(),
            Some(theme) => theme.theme_ref(),
            None => self.assets.theme_ref(self.theme),
        }
    }

    /// The embedded theme to load ahead for this style, when it uses one.
    pub(crate) fn embedded_theme(&self) -> Option<Theme> {
        (self.code_theme.is_none() && self.colour != ColourMode::None).then_some(self.theme)
    }

    /// True when the code theme encodes terminal palette entries, so it has no
    /// background of its own to tint diff lines against.
    pub(crate) fn uses_palette(&self) -> bool {
        self.code_theme.is_none() && self.theme.uses_palette()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CodeBlock {
    label: Option<Line>,
    /// The closing fence, without colour; written once at the end of the block.
    end: Option<Line>,
    body: Body,
    started: bool,
    /// The background of a row of code (`code.bg`).
    fill: Option<Colour>,
}

#[derive(Clone, Debug)]
enum Body {
    Plain,
    /// No language: the first line decides whether this is a diff.
    Sniff(Box<CodeStyle>),
    Highlighted(Box<Highlight>),
    Diff(Box<Diff>),
}

/// What a fence's info string says: the language (its first word) and a file name.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Info {
    pub(crate) language: Option<String>,
    pub(crate) file: Option<String>,
}

impl Info {
    pub(crate) fn parse(info: &str) -> Info {
        let language = info
            .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
            .next()
            .filter(|word| !word.is_empty() && !word.contains('='))
            .map(|word| sanitize(word).into_owned());
        let file = attribute(info, "title")
            .or_else(|| attribute(info, "file"))
            .or_else(|| {
                info.split_whitespace()
                    .nth(1)
                    .map(|word| word.trim_matches(|c| matches!(c, '{' | '}' | '"' | '\'')))
                    .filter(|word| !word.contains('=') && word.contains(['/', '.']))
                    .map(str::to_owned)
            })
            .filter(|file| !file.is_empty())
            .map(|file| sanitize(&file).into_owned());
        Info { language, file }
    }

    /// The label to show: the language and the file, apart by `separator`; the
    /// language alone or the file alone; nothing for a diff, and no language for plain
    /// text. The language is the name that [`LANGUAGE_NAMES`] gives its word, else the
    /// word as the fence has it.
    pub(crate) fn label(&self, separator: &str) -> Option<String> {
        let word = self.language.as_deref();
        if word.is_some_and(is_diff) {
            return None;
        }
        let language = word.filter(|word| !is_silent(word)).map(|word| {
            let lower = word.to_ascii_lowercase();
            LANGUAGE_NAMES.iter().find(|(short, _)| *short == lower).map_or(word, |(_, name)| name)
        });
        match (language, self.file.as_deref()) {
            (Some(language), Some(file)) => Some(format!("{language} {separator} {file}")),
            (Some(label), None) | (None, Some(label)) => Some(label.to_owned()),
            (None, None) => None,
        }
    }
}

fn is_diff(language: &str) -> bool {
    DIFF_LABELS.contains(&language.to_ascii_lowercase().as_str())
}

fn is_silent(language: &str) -> bool {
    SILENT_LABELS.contains(&language.to_ascii_lowercase().as_str())
}

/// The value of `name=value`, `name="value"` or `name='value'` in an info string, at
/// the start of a word.
fn attribute(info: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let mut from = 0;
    while let Some(found) = info[from..].find(&key) {
        let at = from + found;
        let starts_word = info[..at]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || c == '{' || c == ',');
        let rest = &info[at + key.len()..];
        if starts_word {
            let value = match rest.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let inner = &rest[1..];
                    inner.split(quote).next().unwrap_or(inner)
                }
                _ => rest
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '}')
                    .next()
                    .unwrap_or(rest),
            };
            return Some(value.to_owned());
        }
        from = at + key.len();
    }
    None
}

impl CodeBlock {
    /// A block for a fence's info string (empty for an indented block). Grammars load
    /// only here, and only for a block with a known language or file in a colour mode.
    pub(crate) fn new(info: &str, style: &CodeStyle) -> CodeBlock {
        let info = Info::parse(info);
        let fences = style.colour == ColourMode::None;
        let label = if fences {
            let word = info.language.as_deref().unwrap_or_default();
            Some(format!("{FENCE}{word}"))
        } else {
            info.label(style.glyphs.separator)
        };
        let label = label.map(|label| vec![Span::new(label, style.label)]);
        let end = fences.then(|| vec![Span::new(FENCE, style.label)]);
        let language = info.language.as_deref();
        let body = match language {
            Some(language) if is_diff(language) => Body::Diff(Box::new(Diff::new(style.clone()))),
            Some(language) if is_silent(language) => Body::Plain,
            None if info.file.is_none() => Body::Sniff(Box::new(style.clone())),
            _ => {
                let assets = style.assets;
                let syntax = language
                    .and_then(|language| assets.syntax_for_label(language))
                    .or_else(|| info.file.as_deref().and_then(|file| assets.syntax_for_path(file)));
                syntax.map_or(Body::Plain, |syntax| {
                    Body::Highlighted(Box::new(Highlight::new(assets, syntax, style.theme_ref())))
                })
            }
        };
        CodeBlock { label, end, body, started: false, fill: style.code_bg }
    }

    /// The label line, the first time it is asked for; `None` afterwards, and for a
    /// block without a label.
    pub(crate) fn start(&mut self) -> Option<Line> {
        if self.started {
            return None;
        }
        self.started = true;
        self.label.clone()
    }

    /// The closing fence, which ends a block without colour; `None` in colour, where
    /// a block has no fences.
    pub(crate) fn end(&self) -> Option<Line> {
        self.end.clone()
    }

    /// The background of the row of the line that [`line`](Self::line) rendered last,
    /// to fill to the end of the row: `code.bg`, or for an added or a removed line of a
    /// diff its own. `None` outside truecolor and without a colour for it.
    pub(crate) fn fill(&self) -> Option<Colour> {
        match &self.body {
            Body::Diff(diff) => diff.fill().or(self.fill),
            _ => self.fill,
        }
    }

    /// The background of the row of the line that [`line`](Self::line) rendered last
    /// when it is an added or a removed line of a diff, without the `code.bg` of the
    /// other rows: for a diff that the caller lays out apart from a code block.
    pub(crate) fn changed_fill(&self) -> Option<Colour> {
        match &self.body {
            Body::Diff(diff) => diff.fill(),
            _ => None,
        }
    }

    /// The kind of the line that [`line`](Self::line) rendered last, for a diff;
    /// `None` for a block that is not a diff.
    pub(crate) fn diff_kind(&self) -> Option<DiffKind> {
        match &self.body {
            Body::Diff(diff) => Some(diff.last_kind()),
            _ => None,
        }
    }

    /// Renders the next source line, given without its newline.
    pub(crate) fn line(&mut self, source: &str) -> Line {
        let source = source.strip_suffix('\r').unwrap_or(source);
        let expanded = expand_tabs(source);
        let text = sanitize(&expanded);
        if let Body::Sniff(style) = &self.body {
            self.body = if text.starts_with("diff --git ") {
                Body::Diff(Box::new(Diff::new((**style).clone())))
            } else {
                Body::Plain
            };
        }
        match &mut self.body {
            Body::Plain | Body::Sniff(_) => vec![Span::plain(text)],
            Body::Diff(diff) => diff.line(&text),
            Body::Highlighted(highlight) => match highlight.line(&text) {
                Some(tokens) => {
                    let mut line = Vec::new();
                    for (style, piece) in tokens {
                        push_span(&mut line, Span::new(piece, style));
                    }
                    line
                }
                None => {
                    self.body = Body::Plain;
                    vec![Span::plain(text)]
                }
            },
        }
    }
}

/// Loads the syntax data of code blocks ahead, for a caller that runs it on a thread
/// of its own, so that a frame does not wait for the load. [`Preload::load`] loads the
/// grammars and the theme with its highlighter (1 to 2 ms). [`Preload::text`] reads
/// the text of a reply as it comes, and readies the grammar of each fenced block that
/// opens in it, which the first line of the block otherwise pays: 1 to 125 ms the
/// first time, by the language. The block's label line does not need the grammar, so
/// the work starts while the label shows. Without a terminal, nothing loads; without
/// colour, the grammars still load, for the italic of comments.
#[derive(Debug)]
pub struct Preload {
    style: Option<CodeStyle>,
    /// The line in progress of the text.
    line: String,
    /// The info strings whose grammars are ready.
    ready: Vec<String>,
}

/// A line in progress longer than this is no fence's opening line: it is dropped.
const PRELOAD_LINE_MAX: usize = 1024;

impl Preload {
    /// A preload for the code blocks of `options`.
    pub fn new(options: &RenderOptions) -> Preload {
        Preload::with_assets(options, &ASSETS)
    }

    pub(crate) fn with_assets(options: &RenderOptions, assets: &'static Assets) -> Preload {
        let style = options.is_terminal().then(|| CodeStyle::new(options, assets));
        Preload { style, line: String::new(), ready: Vec::new() }
    }

    /// Loads the grammars, and the embedded theme with its highlighter unless a code
    /// theme of the user's own replaces it. A second call costs nothing.
    pub fn load(&self) {
        let Some(style) = &self.style else { return };
        style.assets.load(style.embedded_theme());
    }

    /// The info strings whose grammars this preload readied.
    #[cfg(test)]
    pub(crate) fn ready(&self) -> Vec<&str> {
        self.ready.iter().map(String::as_str).collect()
    }

    /// Takes the next piece of a reply's text, and readies the grammar of each fenced
    /// code block whose opening line ends in it.
    pub fn text(&mut self, piece: &str) {
        let Some(style) = &self.style else { return };
        for part in piece.split_inclusive('\n') {
            if self.line.len() + part.len() <= PRELOAD_LINE_MAX {
                self.line.push_str(part);
            } else {
                self.line.clear();
            }
            if !part.ends_with('\n') {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            if let Some(info) = fence_info(&line)
                && !self.ready.iter().any(|ready| ready == info)
            {
                // NOTE: an empty line unpacks the grammar and builds the patterns of
                // its first context.
                CodeBlock::new(info, style).line("");
                self.ready.push(info.to_owned());
            }
        }
    }
}

/// The info string of `line` when it opens a fenced code block, at the top level or
/// after quote and list markers; `None` for a fence without one. A guess is enough:
/// a wrong one costs a grammar load, never a wrong rendering.
pub(crate) fn fence_info(line: &str) -> Option<&str> {
    let mut rest = line.trim_end_matches(['\n', '\r']);
    loop {
        let trimmed = rest.trim_start_matches([' ', '\t', '>']);
        let bullet = trimmed.strip_prefix(['-', '*', '+']);
        let digits = trimmed.trim_start_matches(|c: char| c.is_ascii_digit());
        let number = (digits.len() < trimmed.len()).then(|| digits.strip_prefix(['.', ')']));
        match bullet.or(number.flatten()).filter(|after| after.starts_with([' ', '\t'])) {
            Some(after) => rest = after,
            None => {
                rest = trimmed;
                break;
            }
        }
    }
    let fence = rest.strip_prefix("```").or_else(|| rest.strip_prefix("~~~"))?;
    let info = fence.trim_start_matches(['`', '~']).trim();
    (!info.is_empty()).then_some(info)
}

/// The lines of a code block's text, without their newlines. A final line without a
/// newline (an unclosed fence at the end of a reply) is still a line.
pub(crate) fn source_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests;
