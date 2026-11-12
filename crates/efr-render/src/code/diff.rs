//! Unified diffs: added lines in the `diff.add` role (green), removed lines in
//! `diff.remove` (red), hunk headers whole in `diff.hunk` (cyan), file headers bold.
//! The sign of a changed line is bold in its role. How the rest of a changed line looks
//! goes with the depth (TM-Diffs, TM-Legend):
//!
//! - truecolor: the diffed file's syntax colours inside (the role's colour where no
//!   grammar knows the file), and the row on `diff.add.bg` or `diff.remove.bg`
//!   ([`Diff::fill`]);
//! - 16 colours: the whole line in the role's colour, no syntax and no background;
//! - no colour: a removed line dim, an added line plain.
//!
//! The old and new sides keep separate highlighting state, as delta does, so a line
//! removed inside a string does not confuse the colours of the lines added after it.

use crate::highlight::{Highlight, theme_background};
use crate::options::ColourMode;
use crate::style::{Colour, Line, Span, Style, push_span};

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

/// What a line of a unified diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
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
    /// Backgrounds of the rows of added and removed lines: the palette's
    /// `diff.add.bg` and `diff.remove.bg` when it sets both, else a blend of a truecolor
    /// theme's background in truecolor mode; a palette theme has no background colour
    /// to blend with.
    tint: Option<(Colour, Colour)>,
    /// The kind of the line rendered last.
    last: Kind,
}

impl Diff {
    pub(crate) fn new(style: CodeStyle) -> Diff {
        let blended = (style.colour == ColourMode::TrueColor && !style.uses_palette())
            .then(|| theme_background(style.theme_ref().get()))
            .flatten()
            .map(|background| (blend(background, ADDED_TINT), blend(background, REMOVED_TINT)));
        // NOTE: a palette that sets the background roles wins; one that sets only one
        // of them takes the blend for the other, so the two sides always differ.
        let tint = match (style.added_bg, style.removed_bg, blended) {
            (Some(added), Some(removed), _) => Some((added, removed)),
            (Some(added), None, Some((_, removed))) => Some((added, removed)),
            (None, Some(removed), Some((added, _))) => Some((added, removed)),
            (_, _, blended) => blended,
        };
        Diff { style, hunk: None, old: None, new: None, tint, last: Kind::Other }
    }

    /// The kind of the line that [`line`](Self::line) rendered last.
    pub(crate) fn last_kind(&self) -> Kind {
        self.last
    }

    /// The background of the row of the line that [`line`](Self::line) rendered last:
    /// the tint of an added or a removed line, else none.
    pub(crate) fn fill(&self) -> Option<Colour> {
        match self.last {
            Kind::Added => self.tint.map(|(added, _)| added),
            Kind::Removed => self.tint.map(|(_, removed)| removed),
            _ => None,
        }
    }

    pub(crate) fn line(&mut self, text: &str) -> Line {
        let kind = self.classify(text);
        self.last = kind;
        match kind {
            Kind::Meta | Kind::File => vec![Span::new(text, Style::PLAIN.bold())],
            Kind::HunkHeader => hunk_header(text, self.style.hunk),
            Kind::Added => self.changed(text, true),
            Kind::Removed => self.changed(text, false),
            Kind::Context => self.context(text),
            Kind::Note => vec![Span::new(text, self.style.label)],
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

    /// Starts fresh highlighting for the file a header names. Only in truecolor: in
    /// 16 colours a changed line is in its role's colour alone, and without colour
    /// nothing is coloured.
    fn set_syntax(&mut self, path: &str) {
        if self.style.colour != ColourMode::TrueColor {
            return;
        }
        let path = path.split('\t').next().unwrap_or(path).trim();
        let path = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path);
        let highlight = self
            .style
            .assets
            .syntax_for_path(path)
            .map(|syntax| Highlight::new(self.style.assets, syntax, self.style.theme_ref()));
        self.old.clone_from(&highlight);
        self.new = highlight;
    }

    fn changed(&mut self, text: &str, added: bool) -> Line {
        let role = if added { self.style.added } else { self.style.removed };
        let (sign, content) = text.split_at(1);
        let mut line = vec![Span::new(sign, role.bold())];
        let side = if added { &mut self.new } else { &mut self.old };
        match side.as_mut().and_then(|highlight| highlight.line(content)) {
            Some(tokens) => {
                for (style, piece) in tokens {
                    push_span(&mut line, Span::new(piece, style));
                }
            }
            None if !added && self.style.colour == ColourMode::None => {
                push_span(&mut line, Span::new(content, role.dim()));
            }
            None => push_span(&mut line, Span::new(content, role)),
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

/// A hunk header, whole in `diff.hunk`: the ranges and the function context after
/// them (TM-Code 1).
fn hunk_header(text: &str, style: Style) -> Line {
    vec![Span::new(text, style)]
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
