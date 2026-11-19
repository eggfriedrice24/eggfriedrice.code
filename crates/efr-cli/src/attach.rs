//! The `@path` words of a `,` line: the files that go with its prompt, and their
//! uploads (`attachment.put`).
//!
//! A word is `@` at the start of the prompt or after a blank, then a path: absolute,
//! `~/`, `./`, `../`, or relative to the shell's directory. `@"my file.png"` and
//! `@'a b.png'` name a path with blanks, and a backslash before a character stands for
//! that character, as Ghostty escapes a dropped path. `user@host` is never a word. A
//! word whose path names no file is tried once more without the marks of a sentence at
//! its end, so `what is in @shot.png?` names `shot.png`.
//!
//! A word that names a regular file attaches it. Each other word stays text, and the
//! prompt goes. A note says so only for a directory, and for a word that looks like a
//! path: it starts with `/`, `~/`, `./` or `../`, or it has a `/` and its parent dir
//! exists. Prompts about code have many words such as `@types/node` or `@app.route`
//! that name nothing.
//!
//! The prompt text keeps each word as typed, but for a pasted image: its word is the
//! path of a clip file below [`CLIPS_DIR`], which `efr` removes once the prompt went,
//! so the sent text names the attachment as `@paste-1.png`.
//!
//! `upload` reads each file in chunks, uploads it and sends the prompt with the
//! uploads, all on one connection.

mod upload;

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use efr_protocol::fit_name;

use crate::context::Context;

pub(crate) use upload::{Failure, send};

/// The dir of the clip files of pasted images, below the state root of `efr`.
pub(crate) const CLIPS_DIR: &str = "clips";

/// The note when the text of a steer names a file: a steer takes no attachment, so the
/// word stays text (spec 4.7).
pub(crate) const STEER_NOTE: &str = "attachments go with a prompt, not with a steer";

/// The marks of a sentence that can end a word after its path, such as the `?` of
/// `what is in @shot.png?`.
const SENTENCE_MARKS: [char; 6] = ['.', ',', ';', ':', '!', '?'];

/// One `@path` word of a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    /// Where the word stands in the prompt, from its `@` to its end.
    pub(crate) range: Range<usize>,
    /// The path that the word names, with its quotes and escapes undone.
    pub(crate) path: String,
    /// The path stood in quotes, so it has no sentence marks to try without.
    quoted: bool,
}

/// The `@path` words of `text`, in their order.
pub(crate) fn tokens(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find('@') {
        let start = from + found;
        from = start + 1;
        if !text[..start].chars().next_back().is_none_or(char::is_whitespace) {
            continue;
        }
        let rest = &text[from..];
        let (path, len, quoted) = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => match rest[1..].find(quote) {
                Some(end) if end > 0 => (rest[1..=end].to_owned(), end + 2, true),
                _ => continue,
            },
            Some(first) if !first.is_whitespace() => {
                let (path, len) = unescaped(rest);
                (path, len, false)
            }
            _ => continue,
        };
        from += len;
        tokens.push(Token { range: start..from, path, quoted });
    }
    tokens
}

/// The path at the start of `rest` up to the first blank that no backslash escapes,
/// with its escapes undone, and how many bytes of `rest` it takes.
fn unescaped(rest: &str) -> (String, usize) {
    let mut path = String::new();
    let mut chars = rest.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            c if c.is_whitespace() => return (path, at),
            '\\' => match chars.next() {
                Some((_, escaped)) => path.push(escaped),
                None => path.push('\\'),
            },
            c => path.push(c),
        }
    }
    (path, rest.len())
}

/// A file that a word of a prompt attaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    /// Where its word stands in the prompt.
    pub(crate) range: Range<usize>,
    /// Its path as the word typed it, such as `shot.png`, for a note about it.
    pub(crate) typed: String,
    /// The file, an absolute path with no link in it.
    pub(crate) path: PathBuf,
    /// The name of the attachment, cut to the limit of a name.
    pub(crate) name: String,
    /// Its size when efr looked.
    pub(crate) bytes: u64,
    /// A clip file of a pasted image, which goes as a paste and goes away once the
    /// prompt went.
    pub(crate) clip: bool,
}

/// What the `@path` words of a prompt name: the files that go with it, and the notes
/// about the words that stay text.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Words {
    pub(crate) files: Vec<Found>,
    /// Such as `not attached: @src is a directory`.
    pub(crate) notes: Vec<String>,
}

/// Where the paths of the words of a prompt start.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Places<'a> {
    /// The shell's directory, for a relative path.
    pub(crate) pwd: &'a Path,
    /// The home dir, for `~/`.
    pub(crate) home: Option<&'a Path>,
    /// The state root of `efr`, whose [`CLIPS_DIR`] holds the clip files of pasted
    /// images.
    pub(crate) state: &'a Path,
}

impl<'a> Places<'a> {
    /// The places of a prompt of `ctx` that the shell in `pwd` sent.
    pub(crate) fn of(ctx: &'a Context, pwd: &'a Path) -> Places<'a> {
        Places { pwd, home: ctx.home.as_deref(), state: ctx.dirs.state() }
    }
}

impl Words {
    /// What the words of `text` name.
    pub(crate) fn find(text: &str, places: &Places<'_>) -> Words {
        let mut words = Words::default();
        let tokens = tokens(text);
        if tokens.is_empty() {
            return words;
        }
        let clips = std::fs::canonicalize(places.state.join(CLIPS_DIR)).ok();
        for token in tokens {
            let (range, path, named) = match Named::of(&token.path, places) {
                Named::Nothing => match without_marks(&token, text) {
                    Some((range, path)) => {
                        let named = Named::of(&path, places);
                        (range, path, named)
                    }
                    None => (token.range, token.path, Named::Nothing),
                },
                named => (token.range, token.path, named),
            };
            let typed = &text[range.clone()];
            match named {
                Named::File(file) => words.files.push(found(range, path, file, clips.as_deref())),
                Named::Dir => words.notes.push(format!("not attached: {typed} is a directory")),
                Named::Nothing if looks_like_path(&path, places) => {
                    words.notes.push(format!("not attached: {typed} (no such file)"));
                }
                Named::Nothing | Named::Other => {}
            }
        }
        words
    }

    /// The text to send for `text`, whose words these are: each word of a clip file
    /// names its attachment, such as `@paste-1.png`, and each other word stays as typed.
    pub(crate) fn sent_text(&self, text: &str) -> String {
        let mut sent = String::with_capacity(text.len());
        let mut from = 0;
        for file in self.files.iter().filter(|file| file.clip) {
            sent.push_str(&text[from..file.range.start]);
            sent.push('@');
            sent.push_str(&file.name);
            from = file.range.end;
        }
        sent.push_str(&text[from..]);
        sent
    }
}

/// A regular file that a path names: its path with no link in it, and its size.
struct File {
    path: PathBuf,
    bytes: u64,
}

/// What the path of a word names.
enum Named {
    File(File),
    Dir,
    /// Something else, such as a FIFO.
    Other,
    Nothing,
}

impl Named {
    fn of(path: &str, places: &Places<'_>) -> Named {
        let Some(absolute) = absolute(path, places) else {
            return Named::Nothing;
        };
        match std::fs::metadata(&absolute) {
            Ok(meta) if meta.is_file() => match std::fs::canonicalize(&absolute) {
                Ok(path) => Named::File(File { path, bytes: meta.len() }),
                Err(_) => Named::Nothing,
            },
            Ok(meta) if meta.is_dir() => Named::Dir,
            Ok(_) => Named::Other,
            Err(_) => Named::Nothing,
        }
    }
}

/// The range and the path of `token`, a word of `text`, without the sentence marks at
/// its end; `None` for a quoted path and a path with no marks at its end.
fn without_marks(token: &Token, text: &str) -> Option<(Range<usize>, String)> {
    let trimmed = token.path.trim_end_matches(SENTENCE_MARKS);
    let marks = &token.path[trimmed.len()..];
    if token.quoted || marks.is_empty() || trimmed.is_empty() {
        return None;
    }
    let range = token.range.start..token.range.end - marks.len();
    text[token.range.clone()].ends_with(marks).then(|| (range, trimmed.to_owned()))
}

/// The attachment of `file`, which the word at `range` names as `typed`: a clip file
/// below `clips` goes as a paste.
fn found(range: Range<usize>, typed: String, file: File, clips: Option<&Path>) -> Found {
    let File { path, bytes } = file;
    let file_name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
    let clip = clips.is_some_and(|clips| path.parent() == Some(clips));
    let name = match clip.then(|| ClipName::parse(&file_name)).flatten() {
        Some(clip) => clip.pasted(),
        None => fit_name(&file_name),
    };
    Found { range, typed, path, name, bytes, clip }
}

/// `path` as an absolute path: `~/` below the home dir, a relative path below the
/// shell's directory; `None` for `~/` with no home dir.
fn absolute(path: &str, places: &Places<'_>) -> Option<PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => Some(places.home?.join(rest)),
        None => Some(places.pwd.join(path)),
    }
}

/// True when a word with `path`, which names nothing, looks like a path, so a note
/// says that it names no file.
fn looks_like_path(path: &str, places: &Places<'_>) -> bool {
    if ["/", "~/", "./", "../"].iter().any(|start| path.starts_with(start)) {
        return true;
    }
    path.contains('/')
        && absolute(path, places)
            .is_some_and(|absolute| absolute.parent().is_some_and(Path::is_dir))
}

/// The name of a clip file: `<shell>.<n>.<extension>`, the paste `n` of the shell with
/// the id `shell`. `efr clip` writes it; the `@` word of the file reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClipName<'a> {
    pub(crate) shell: u32,
    pub(crate) number: u64,
    pub(crate) extension: &'a str,
}

impl<'a> ClipName<'a> {
    /// The clip name `file_name`; `None` for a name of another form.
    pub(crate) fn parse(file_name: &'a str) -> Option<Self> {
        let (shell, rest) = file_name.split_once('.')?;
        let (number, extension) = rest.split_once('.')?;
        let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        if !digits(shell) || !digits(number) || extension.is_empty() {
            return None;
        }
        Some(ClipName { shell: shell.parse().ok()?, number: number.parse().ok()?, extension })
    }

    /// The name of the attachment of the clip: `paste-<n>.<extension>`.
    pub(crate) fn pasted(&self) -> String {
        format!("paste-{}.{}", self.number, self.extension)
    }
}

impl fmt::Display for ClipName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.shell, self.number, self.extension)
    }
}

#[cfg(test)]
mod tests;
