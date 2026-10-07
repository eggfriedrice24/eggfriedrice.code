//! Code blocks: a muted label line when it tells the reader something, then one output
//! line per source line with syntax colours, never wrapped. A block renders one line
//! at a time, so the streaming renderer commits each line as soon as it is complete,
//! and a block streamed in pieces renders exactly as the same block rendered at once.
//!
//! The label is the file name when the fence names one (`rust src/parse.rs`,
//! `title="x"`, `file=x`), none for plain text and diffs (`text`, `txt`, `plain`,
//! `plaintext`, `diff`, `patch`, `udiff` or no info), else the language. No frame, no
//! background and no indent: the user copies code with the mouse.

mod diff;

use crate::code_theme::CodeTheme;
use crate::highlight::{Assets, Highlight, ThemeRef};
use crate::options::{ColourMode, RenderOptions, Theme};
use crate::palette::Role;
use crate::style::{Line, Span, Style, expand_tabs, push_span, sanitize};

use diff::Diff;

/// Fence labels that mark a unified diff.
const DIFF_LABELS: &[&str] = &["diff", "patch", "udiff"];

/// Fence labels that say nothing a reader needs: plain text and diffs show no label.
const SILENT_LABELS: &[&str] = &["text", "txt", "plain", "plaintext", "diff", "patch", "udiff"];

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
}

impl CodeStyle {
    pub(crate) fn new(options: &RenderOptions, assets: &'static Assets) -> CodeStyle {
        CodeStyle {
            colour: options.colour(),
            theme: options.theme(),
            code_theme: options.code_theme().cloned(),
            assets,
            label: options.role_style(Role::Muted),
            added: options.role_style(Role::DiffAdd),
            removed: options.role_style(Role::DiffRemove),
            hunk: options.role_style(Role::DiffHunk),
        }
    }

    /// The theme of highlighted code: the user's code theme, else the embedded one.
    pub(crate) fn theme_ref(&self) -> ThemeRef {
        match &self.code_theme {
            Some(theme) => ThemeRef::Custom(theme.clone()),
            None => ThemeRef::Embedded(self.assets.theme(self.theme)),
        }
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
    body: Body,
    started: bool,
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

    /// The label to show: the file name, else the language unless it is plain text or
    /// a diff.
    pub(crate) fn label(&self) -> Option<&str> {
        if let Some(file) = &self.file {
            return Some(file);
        }
        self.language.as_deref().filter(|language| !is_silent(language))
    }
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
        let label = info.label().map(|label| vec![Span::new(label, style.label)]);
        let language = info.language.as_deref();
        let body = match language {
            Some(language) if DIFF_LABELS.contains(&language.to_ascii_lowercase().as_str()) => {
                Body::Diff(Box::new(Diff::new(style.clone())))
            }
            _ if style.colour == ColourMode::None => Body::Plain,
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
        CodeBlock { label, body, started: false }
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
