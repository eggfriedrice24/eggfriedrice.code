//! A lenient split of a command line for the exit prediction.
//!
//! [`analyze`](crate::command::analyze) fails closed: a line with a substitution, a
//! group or a redirection to a file is one opaque construct. The prediction must still
//! find the exits it can, so this split reads every line: it takes quotes and escapes
//! as zsh does, splits on `;`, `&`, `|`, newlines, parentheses and braces that stand
//! alone, scans the text of every substitution as commands of its own, records the
//! targets of output redirections, and skips the body of a here-document.
//!
//! It may find a program too many, never one too few where it can see one: text
//! analysis can only add a question, and the sandbox holds what it misses.

/// Stands in a word for text that only the shell knows: a substitution or a parameter
/// expansion. A word that holds it names no path the engine can resolve.
pub(super) const OPAQUE: char = '\u{1}';

/// One simple command as the lenient split reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Segment {
    /// The words, quotes removed, assignments and redirections included as written.
    pub(super) words: Vec<String>,
    /// The targets of its output redirections, in order.
    pub(super) redirects: Vec<Redirect>,
    /// True when a `cd`, `pushd` or `popd` comes before it in the line, so where it
    /// runs is unknown.
    pub(super) after_cd: bool,
}

/// The target of an output redirection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Redirect {
    /// The target word, quotes removed.
    pub(super) target: String,
    /// True for `>>`, which appends instead of truncating.
    pub(super) append: bool,
}

/// The simple commands of `line`, in the order the split meets them; the commands of a
/// substitution come before the command that holds it.
pub(super) fn scan(line: &str) -> Vec<Segment> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    scan_into(&chars, &mut out);
    let mut after_cd = false;
    for segment in &mut out {
        segment.after_cd = after_cd;
        let program = segment.words.iter().find(|word| !is_assignment(word));
        if program.is_some_and(|program| matches!(program.as_str(), "cd" | "pushd" | "popd")) {
            after_cd = true;
        }
    }
    out
}

/// True when `word` is `NAME=value` or `NAME+=value`.
pub(super) fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What the next word is, after a redirection operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// The target of an output redirection.
    Output { append: bool },
    /// A file that the command reads, or a here-string: no target.
    Input,
    /// The delimiter of a here-document.
    HereDocument { strip_tabs: bool },
}

/// The simple command being read.
#[derive(Debug, Default)]
struct Building {
    words: Vec<String>,
    redirects: Vec<Redirect>,
    word: String,
    /// True when the word being read has a quote, so an empty word still counts.
    quoted: bool,
    pending: Option<Pending>,
    /// The here-documents whose bodies follow the next newline.
    here_documents: Vec<(String, bool)>,
}

impl Building {
    fn in_word(&self) -> bool {
        !self.word.is_empty() || self.quoted
    }

    fn end_word(&mut self) {
        if !self.in_word() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        self.quoted = false;
        match self.pending.take() {
            Some(Pending::Output { append }) => {
                self.redirects.push(Redirect { target: word, append });
            }
            Some(Pending::Input) => {}
            Some(Pending::HereDocument { strip_tabs }) => {
                self.here_documents.push((word, strip_tabs));
            }
            None if self.words.is_empty() && matches!(word.as_str(), "{" | "}") => {}
            None => self.words.push(word),
        }
    }

    fn end_segment(&mut self, out: &mut Vec<Segment>) {
        self.end_word();
        self.pending = None;
        if self.words.is_empty() && self.redirects.is_empty() {
            return;
        }
        out.push(Segment {
            words: std::mem::take(&mut self.words),
            redirects: std::mem::take(&mut self.redirects),
            after_cd: false,
        });
    }
}

fn scan_into(chars: &[char], out: &mut Vec<Segment>) {
    let mut building = Building::default();
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        let next = chars.get(at + 1).copied();
        match c {
            ' ' | '\t' => {
                building.end_word();
                at += 1;
            }
            '\n' => {
                building.end_segment(out);
                at += 1;
                let bodies = std::mem::take(&mut building.here_documents);
                at = skip_here_documents(chars, at, &bodies);
            }
            ';' | '|' | '(' | ')' => {
                building.end_segment(out);
                at += 1;
            }
            '&' if next == Some('>') => {
                building.end_word();
                at += 2;
                let append = chars.get(at) == Some(&'>');
                if append {
                    at += 1;
                }
                if matches!(chars.get(at), Some('|' | '!')) {
                    at += 1;
                }
                building.pending = Some(Pending::Output { append });
            }
            '&' => {
                building.end_segment(out);
                at += 1;
            }
            '<' | '>' if next == Some('(') && !building.in_word() => {
                at = substitution(chars, at + 1, out, &mut building);
            }
            '<' | '>' => {
                if !building.quoted && building.word.chars().all(|c| c.is_ascii_digit()) {
                    // NOTE: digits right before the operator name a descriptor.
                    building.word.clear();
                } else {
                    building.end_word();
                }
                at = redirection(chars, at, &mut building);
            }
            '\'' => {
                building.quoted = true;
                at += 1;
                while let Some(&quoted) = chars.get(at) {
                    at += 1;
                    if quoted == '\'' {
                        break;
                    }
                    building.word.push(quoted);
                }
            }
            '"' => {
                building.quoted = true;
                at = double_quoted(chars, at + 1, out, &mut building);
            }
            '\\' => {
                match next {
                    Some('\n') | None => {}
                    Some(escaped) => {
                        building.quoted = true;
                        building.word.push(escaped);
                    }
                }
                at += 2;
            }
            '$' => at = dollar(chars, at, out, &mut building),
            '`' => at = backquoted(chars, at + 1, out, &mut building),
            '=' if next == Some('(') && !building.in_word() => {
                at = substitution(chars, at + 1, out, &mut building);
            }
            '#' if !building.in_word() => {
                while chars.get(at).is_some_and(|&c| c != '\n') {
                    at += 1;
                }
            }
            c => {
                building.word.push(c);
                at += 1;
            }
        }
    }
    building.end_segment(out);
}

/// A redirection operator at `at`, which is `<` or `>`; returns where it ends.
fn redirection(chars: &[char], mut at: usize, building: &mut Building) -> usize {
    let first = chars.get(at).copied().unwrap_or('>');
    at += 1;
    let next = chars.get(at).copied();
    building.pending = match (first, next) {
        ('<', Some('<')) => {
            at += 1;
            if chars.get(at) == Some(&'<') {
                at += 1;
                Some(Pending::Input)
            } else {
                let strip_tabs = chars.get(at) == Some(&'-');
                if strip_tabs {
                    at += 1;
                }
                Some(Pending::HereDocument { strip_tabs })
            }
        }
        ('<', Some('>')) => {
            at += 1;
            Some(Pending::Output { append: false })
        }
        (_, Some('&')) => {
            at += 1;
            let start = at;
            while chars.get(at).is_some_and(|c| c.is_ascii_digit() || *c == '-') {
                at += 1;
            }
            if at > start || first == '<' {
                // A copy or a close of a descriptor writes no file.
                None
            } else {
                Some(Pending::Output { append: false })
            }
        }
        ('<', _) => Some(Pending::Input),
        (_, Some('>')) => {
            at += 1;
            if matches!(chars.get(at), Some('|' | '!')) {
                at += 1;
            }
            Some(Pending::Output { append: true })
        }
        (_, Some('|' | '!')) => {
            at += 1;
            Some(Pending::Output { append: false })
        }
        _ => Some(Pending::Output { append: false }),
    };
    at
}

/// The rest of a double-quoted string from `at`; returns where it ends.
fn double_quoted(
    chars: &[char],
    mut at: usize,
    out: &mut Vec<Segment>,
    building: &mut Building,
) -> usize {
    while let Some(&c) = chars.get(at) {
        match c {
            '"' => return at + 1,
            '\\' => {
                match chars.get(at + 1) {
                    Some(escaped @ ('$' | '`' | '"' | '\\')) => building.word.push(*escaped),
                    Some('\n') | None => {}
                    Some(other) => {
                        building.word.push('\\');
                        building.word.push(*other);
                    }
                }
                at += 2;
            }
            '$' => at = dollar(chars, at, out, building),
            '`' => at = backquoted(chars, at + 1, out, building),
            c => {
                building.word.push(c);
                at += 1;
            }
        }
    }
    at
}

/// A `$` at `at`: a substitution, an expansion or a quote; returns where it ends.
fn dollar(chars: &[char], at: usize, out: &mut Vec<Segment>, building: &mut Building) -> usize {
    match chars.get(at + 1) {
        Some('(') => substitution(chars, at + 1, out, building),
        Some('{') => {
            building.word.push(OPAQUE);
            closing(chars, at + 1, '{', '}') + 1
        }
        Some('\'') => {
            // `$'...'`: a quote with escapes, whose text is plain.
            building.quoted = true;
            let mut end = at + 2;
            while let Some(&c) = chars.get(end) {
                end += 1;
                match c {
                    '\'' => break,
                    '\\' => {
                        if let Some(&escaped) = chars.get(end) {
                            building.word.push(escaped);
                            end += 1;
                        }
                    }
                    c => building.word.push(c),
                }
            }
            end
        }
        _ => {
            building.word.push(OPAQUE);
            at + 1
        }
    }
}

/// A substitution whose `(` is at `open`: its text is scanned as commands of its own,
/// and the word gets [`OPAQUE`]. Returns where it ends.
fn substitution(
    chars: &[char],
    open: usize,
    out: &mut Vec<Segment>,
    building: &mut Building,
) -> usize {
    let close = closing(chars, open, '(', ')');
    let inner = chars.get(open + 1..close.min(chars.len())).unwrap_or_default();
    scan_into(inner, out);
    building.word.push(OPAQUE);
    close + 1
}

/// A backquoted substitution whose text starts at `at`; returns where it ends.
fn backquoted(chars: &[char], at: usize, out: &mut Vec<Segment>, building: &mut Building) -> usize {
    let mut end = at;
    while let Some(&c) = chars.get(end) {
        match c {
            '`' => break,
            '\\' => end += 2,
            _ => end += 1,
        }
    }
    let inner = chars.get(at..end.min(chars.len())).unwrap_or_default();
    scan_into(inner, out);
    building.word.push(OPAQUE);
    end + 1
}

/// The position of the bracket that closes the one at `open`, past nested pairs and
/// quotes, or the end of the line.
fn closing(chars: &[char], open: usize, left: char, right: char) -> usize {
    let mut depth = 0usize;
    let mut at = open;
    while let Some(&c) = chars.get(at) {
        match c {
            '\\' => at += 1,
            '\'' => {
                at += 1;
                while chars.get(at).is_some_and(|&c| c != '\'') {
                    at += 1;
                }
            }
            '"' => {
                at += 1;
                while let Some(&c) = chars.get(at) {
                    match c {
                        '"' => break,
                        '\\' => at += 1,
                        _ => {}
                    }
                    at += 1;
                }
            }
            c if c == left => depth += 1,
            c if c == right => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return at;
                }
            }
            _ => {}
        }
        at += 1;
    }
    chars.len()
}

/// Skips the bodies of `bodies`, the here-documents of the line that just ended, from
/// `at`; returns where the next command starts.
fn skip_here_documents(chars: &[char], mut at: usize, bodies: &[(String, bool)]) -> usize {
    for (delimiter, strip_tabs) in bodies {
        while at < chars.len() {
            let end = chars[at..].iter().position(|&c| c == '\n').map_or(chars.len(), |n| at + n);
            let line: String = chars[at..end].iter().collect();
            let line = if *strip_tabs { line.trim_start_matches('\t') } else { line.as_str() };
            at = (end + 1).min(chars.len());
            if line == delimiter {
                break;
            }
        }
    }
    at
}
