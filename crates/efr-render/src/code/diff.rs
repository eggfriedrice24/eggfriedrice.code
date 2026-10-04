//! Unified diffs: added lines green, removed lines red, hunk headers cyan, file
//! headers bold, and the diffed file's syntax colours inside the changed lines. The
//! old and new sides keep separate highlighting state, as delta does, so a line
//! removed inside a string does not confuse the colours of the lines added after it.

use crate::highlight::{Highlight, theme_background};
use crate::options::ColourMode;
use crate::style::{CYAN, Colour, GREEN, Line, RED, Span, Style, push_span};

use super::CodeStyle;

/// Mixed into a truecolor theme's background to tint changed lines.
const ADDED_TINT: (u8, u8, u8) = (0x2e, 0xa0, 0x43);
const REMOVED_TINT: (u8, u8, u8) = (0xd7, 0x3a, 0x49);

/// Lines still expected in the current hunk, from its `@@ -a,b +c,d @@` header.
/// `None` when the header gave no usable counts, as model-written diffs often do.
#[derive(Clone, Copy, Debug)]
struct Hunk {
    remaining: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Meta,
    File,
    HunkHeader,
    Added,
    Removed,
    Context,
    Note,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct Diff {
    style: CodeStyle,
    hunk: Option<Hunk>,
    old: Option<Highlight>,
    new: Option<Highlight>,
    /// Backgrounds for added and removed lines, only for a truecolor theme in
    /// truecolor mode; a palette theme has no background colour to blend with.
    tint: Option<(Colour, Colour)>,
}

impl Diff {
    pub(crate) fn new(style: CodeStyle) -> Diff {
        let tint = (style.colour == ColourMode::TrueColor && !style.theme.uses_palette())
            .then(|| theme_background(style.assets.theme(style.theme)))
            .flatten()
            .map(|background| (blend(background, ADDED_TINT), blend(background, REMOVED_TINT)));
        Diff { style, hunk: None, old: None, new: None, tint }
    }

    pub(crate) fn line(&mut self, text: &str) -> Line {
        match self.classify(text) {
            Kind::Meta | Kind::File => vec![Span::new(text, Style::PLAIN.bold())],
            Kind::HunkHeader => hunk_header(text),
            Kind::Added => self.changed(text, true),
            Kind::Removed => self.changed(text, false),
            Kind::Context => self.context(text),
            Kind::Note => vec![Span::new(text, Style::PLAIN.dim())],
            Kind::Other => vec![Span::plain(text)],
        }
    }

    fn classify(&mut self, text: &str) -> Kind {
        if let Some(rest) = text.strip_prefix("diff --git ") {
            self.hunk = None;
            if let Some(path) = rest.rsplit(' ').next() {
                self.set_syntax(path);
            }
            return Kind::Meta;
        }
        if let Some(hunk) = &mut self.hunk {
            let file_header = is_file_header(text);
            if hunk.remaining.is_some() || !file_header {
                let kind = match text.as_bytes().first() {
                    Some(b'+') => Kind::Added,
                    Some(b'-') => Kind::Removed,
                    Some(b' ') | None => Kind::Context,
                    Some(b'\\') => Kind::Note,
                    Some(b'@') if text.starts_with("@@") => {
                        self.hunk = Some(Hunk { remaining: hunk_counts(text) });
                        return Kind::HunkHeader;
                    }
                    Some(_) => {
                        self.hunk = None;
                        return Kind::Other;
                    }
                };
                if let Some((old, new)) = &mut hunk.remaining {
                    if kind != Kind::Added && kind != Kind::Note {
                        *old = old.saturating_sub(1);
                    }
                    if kind != Kind::Removed && kind != Kind::Note {
                        *new = new.saturating_sub(1);
                    }
                    if *old == 0 && *new == 0 {
                        self.hunk = None;
                    }
                }
                return kind;
            }
            self.hunk = None;
        }
        if let Some(path) = text.strip_prefix("--- ").or_else(|| text.strip_prefix("+++ ")) {
            if !path.starts_with("/dev/null") {
                self.set_syntax(path);
            }
            return Kind::File;
        }
        if text.starts_with("@@") {
            self.hunk = Some(Hunk { remaining: hunk_counts(text) });
            return Kind::HunkHeader;
        }
        const META: &[&str] = &[
            "index ",
            "new file mode",
            "deleted file mode",
            "old mode",
            "new mode",
            "similarity index",
            "dissimilarity index",
            "rename from",
            "rename to",
            "copy from",
            "copy to",
            "Binary files",
        ];
        if META.iter().any(|prefix| text.starts_with(prefix)) {
            return Kind::Meta;
        }
        match text.as_bytes().first() {
            Some(b'+') => Kind::Added,
            Some(b'-') => Kind::Removed,
            _ => Kind::Other,
        }
    }

    /// Starts fresh highlighting for the file a header names. Grammars are not
    /// loaded when the output has no colour.
    fn set_syntax(&mut self, path: &str) {
        if self.style.colour == ColourMode::None {
            return;
        }
        let path = path.split('\t').next().unwrap_or(path).trim();
        let path = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path);
        let highlight = self
            .style
            .assets
            .syntax_for_path(path)
            .map(|syntax| Highlight::new(self.style.assets, syntax, self.style.theme));
        self.old.clone_from(&highlight);
        self.new = highlight;
    }

    fn changed(&mut self, text: &str, added: bool) -> Line {
        let (sign_colour, tint) = if added {
            (GREEN, self.tint.map(|(added, _)| added))
        } else {
            (RED, self.tint.map(|(_, removed)| removed))
        };
        let base = tint.map_or(Style::PLAIN, |tint| Style::PLAIN.on(tint));
        let (sign, content) = text.split_at(1);
        let mut line = vec![Span::new(sign, base.patch(Style::fg(sign_colour).bold()))];
        let side = if added { &mut self.new } else { &mut self.old };
        match side.as_mut().and_then(|highlight| highlight.line(content)) {
            Some(tokens) => {
                for (style, piece) in tokens {
                    push_span(&mut line, Span::new(piece, base.patch(style)));
                }
            }
            None => push_span(&mut line, Span::new(content, base.patch(Style::fg(sign_colour)))),
        }
        line
    }

    fn context(&mut self, text: &str) -> Line {
        let (sign, content) = if text.is_empty() { ("", "") } else { text.split_at(1) };
        if let Some(old) = &mut self.old {
            let _ = old.line(content);
        }
        let mut line = vec![Span::plain(sign)];
        match self.new.as_mut().and_then(|highlight| highlight.line(content)) {
            Some(tokens) => {
                for (style, piece) in tokens {
                    push_span(&mut line, Span::new(piece, style));
                }
            }
            None => push_span(&mut line, Span::plain(content)),
        }
        line
    }
}

fn is_file_header(text: &str) -> bool {
    ["--- a/", "--- /dev/null", "+++ b/", "+++ /dev/null"].iter().any(|p| text.starts_with(p))
}

/// The old and new line counts of `@@ -a,b +c,d @@`; a missing count means 1.
fn hunk_counts(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.trim_start_matches('@').split_whitespace();
    let count = |part: Option<&str>, sign: char| -> Option<u32> {
        let range = part?.strip_prefix(sign)?;
        match range.split_once(',') {
            Some((start, count)) => {
                start.parse::<u32>().ok()?;
                count.parse().ok()
            }
            None => range.parse::<u32>().ok().map(|_| 1),
        }
    };
    let old = count(parts.next(), '-')?;
    let new = count(parts.next(), '+')?;
    Some((old, new))
}

fn hunk_header(text: &str) -> Line {
    let end = text[2..].find("@@").map_or(text.len(), |at| at + 4);
    let (header, rest) = text.split_at(end);
    vec![Span::new(header, Style::fg(CYAN)), Span::plain(rest)]
}

/// Three parts background to one part tint.
fn blend(background: (u8, u8, u8), tint: (u8, u8, u8)) -> Colour {
    let mix = |b: u8, t: u8| {
        let mixed = (u16::from(b) * 3 + u16::from(t)) / 4;
        u8::try_from(mixed).unwrap_or(u8::MAX)
    };
    Colour::Rgb(mix(background.0, tint.0), mix(background.1, tint.1), mix(background.2, tint.2))
}

#[cfg(test)]
mod tests;
