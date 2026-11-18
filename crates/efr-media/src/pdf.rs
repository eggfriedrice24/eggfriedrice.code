//! The two facts of a PDF that decide if a model can read it as content: a password,
//! and the count of pages.
//!
//! This is no PDF parser. It reads the dictionaries of the file one after the other,
//! skips the body of each stream, and inflates only the object streams, which hold the
//! page tree in a file of pdfTeX and of other new writers.

use miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;

/// The most bytes that the object streams of one PDF inflate to, in all. Over this the
/// count of pages is not known.
pub const PDF_INFLATE_MAX: usize = 64 * 1024 * 1024;

/// The name of the zlib filter of a stream, the one filter that the scan inflates.
pub(crate) const ZLIB_FILTER: &[u8] = b"FlateDecode"; // spellchecker:disable-line

/// What [`pdf_facts`] found in a PDF.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PdfFacts {
    /// The file has an `/Encrypt` entry in a trailer or in a cross-reference stream: it
    /// has a password, or is encrypted with an empty one.
    pub encrypted: bool,
    /// The largest `/Count` of a `/Type /Pages` dictionary: the count of pages of the
    /// root of the page tree, which is never less than the real count of a correct
    /// file. `None` when no such dictionary was found, or an object stream did not
    /// inflate within [`PDF_INFLATE_MAX`].
    pub pages: Option<u32>,
}

/// The facts of the PDF `bytes`. A cut or a broken file gives the facts of the part that
/// could be read, never a panic.
pub fn pdf_facts(bytes: &[u8]) -> PdfFacts {
    let mut scan = Scan { facts: PdfFacts::default(), inflated: 0, unknown: false };
    scan.walk(bytes, true);
    if scan.unknown {
        scan.facts.pages = None;
    }
    scan.facts
}

/// The state of one [`pdf_facts`].
struct Scan {
    facts: PdfFacts,
    /// The bytes that the object streams inflated to so far.
    inflated: usize,
    /// An object stream did not inflate: the count of pages is not known.
    unknown: bool,
}

impl Scan {
    /// Reads each dictionary of `bytes` in order. `outer` is true for the file itself
    /// and false for the content of an object stream, which holds no stream.
    fn walk(&mut self, bytes: &[u8], outer: bool) {
        let mut at = 0;
        while let Some(start) = find(bytes, at, b"<<") {
            let Some((end, dict)) = Dict::parse(bytes, start) else {
                // NOTE: the rest of the file is inside a dictionary that never ends.
                return;
            };
            self.read(bytes, start, &dict);
            at = end;
            if outer && let Some((body, after)) = stream(bytes, end, &dict) {
                if dict.is(b"Type", b"ObjStm") && dict.is(b"Filter", ZLIB_FILTER) {
                    self.inflate(body);
                }
                at = after;
            }
        }
    }

    /// Takes the facts of the dictionary `dict` that starts at `start` of `bytes`.
    fn read(&mut self, bytes: &[u8], start: usize, dict: &Dict<'_>) {
        let trailer = bytes[..start].trim_ascii_end().ends_with(b"trailer");
        if dict.has(b"Encrypt") && (trailer || dict.is(b"Type", b"XRef")) {
            self.facts.encrypted = true;
        }
        if dict.is(b"Type", b"Pages")
            && let Some(count) = dict.number(b"Count")
        {
            self.facts.pages = Some(self.facts.pages.map_or(count, |pages| pages.max(count)));
        }
    }

    /// Inflates the object stream `body` within the limit and reads its dictionaries.
    fn inflate(&mut self, body: &[u8]) {
        let room = PDF_INFLATE_MAX - self.inflated;
        match decompress_to_vec_zlib_with_limit(body, room) {
            Ok(objects) => {
                self.inflated += objects.len();
                self.walk(&objects, false);
            }
            Err(_) => self.unknown = true,
        }
    }
}

/// The body of the stream after the dictionary `dict` that ends at `end`, and the offset
/// after its `endstream`, when a stream follows the dictionary.
fn stream<'a>(bytes: &'a [u8], end: usize, dict: &Dict<'_>) -> Option<(&'a [u8], usize)> {
    let rest = bytes.get(end..)?;
    let keyword = end + (rest.len() - rest.trim_ascii_start().len());
    if !bytes.get(keyword..)?.starts_with(b"stream") {
        return None;
    }
    // NOTE: the keyword ends with CR LF or LF.
    let mut data = keyword + b"stream".len();
    data += match bytes.get(data..data + 2) {
        Some(b"\r\n") => 2,
        Some([b'\n', _] | [b'\r', _]) => 1,
        _ => 0,
    };
    let stated = dict
        .direct_number(b"Length")
        .and_then(|length| usize::try_from(length).ok())
        .and_then(|length| data.checked_add(length))
        .filter(|&stop| {
            bytes
                .get(stop..)
                .is_some_and(|after| after.trim_ascii_start().starts_with(b"endstream"))
        });
    let stop = match stated {
        Some(stop) => stop,
        None => find(bytes, data, b"endstream").unwrap_or(bytes.len()),
    };
    Some((&bytes[data..stop], stop))
}

/// The entries of one dictionary as tokens at its own level: names, numbers and the
/// other words. A nested dictionary, an array or a string is one token.
struct Dict<'a> {
    tokens: Vec<Token<'a>>,
}

/// One token of a [`Dict`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// A name with no `/`, such as `Type`.
    Name(&'a [u8]),
    /// A number or a keyword, such as `12` or `R`.
    Word(&'a [u8]),
    /// A nested dictionary, an array or a string.
    Nested,
}

impl<'a> Dict<'a> {
    /// The dictionary that starts with the `<<` at `start` of `bytes`, and the offset
    /// after its `>>`. `None` when it does not end in the file.
    fn parse(bytes: &'a [u8], start: usize) -> Option<(usize, Dict<'a>)> {
        let mut tokens = Vec::new();
        let mut depth = 0_usize;
        let mut at = start;
        loop {
            let byte = *bytes.get(at)?;
            let next = bytes.get(at + 1).copied();
            match byte {
                b'<' if next == Some(b'<') => {
                    depth += 1;
                    if depth == 2 {
                        tokens.push(Token::Nested);
                    }
                    at += 2;
                }
                b'>' if next == Some(b'>') => {
                    depth -= 1;
                    at += 2;
                    if depth == 0 {
                        return Some((at, Dict { tokens }));
                    }
                }
                b'[' | b']' => {
                    if byte == b'[' {
                        depth += 1;
                        if depth == 2 {
                            tokens.push(Token::Nested);
                        }
                    } else {
                        depth = depth.saturating_sub(1).max(1);
                    }
                    at += 1;
                }
                b'(' => {
                    at = string_end(bytes, at)?;
                    if depth == 1 {
                        tokens.push(Token::Nested);
                    }
                }
                b'<' => {
                    at = find(bytes, at, b">")? + 1;
                    if depth == 1 {
                        tokens.push(Token::Nested);
                    }
                }
                b'%' => {
                    at = bytes[at..]
                        .iter()
                        .position(|&b| b == b'\n' || b == b'\r')
                        .map_or(bytes.len(), |end| at + end)
                }
                _ if is_space(byte) || byte == b')' || byte == b'>' => at += 1,
                _ => {
                    let from = at + usize::from(byte == b'/');
                    let end = bytes[from..]
                        .iter()
                        .position(|&b| is_space(b) || is_delimiter(b))
                        .map_or(bytes.len(), |end| from + end);
                    if depth == 1 {
                        let word = &bytes[from..end];
                        tokens.push(if byte == b'/' {
                            Token::Name(word)
                        } else {
                            Token::Word(word)
                        });
                    }
                    at = end.max(at + 1);
                }
            }
        }
    }

    /// True when the dictionary has the key `key`.
    fn has(&self, key: &[u8]) -> bool {
        self.value(key).is_some()
    }

    /// True when the value of `key` is the name `name`.
    fn is(&self, key: &[u8], name: &[u8]) -> bool {
        self.value(key) == Some(Token::Name(name))
    }

    /// The whole number that is the value of `key`.
    fn number(&self, key: &[u8]) -> Option<u32> {
        match self.value(key)? {
            Token::Word(word) => std::str::from_utf8(word).ok()?.parse().ok(),
            _ => None,
        }
    }

    /// The whole number that is the value of `key`, when it is not a reference such as
    /// `12 0 R` to an object that holds the value.
    fn direct_number(&self, key: &[u8]) -> Option<u32> {
        let at = self.key(key)?;
        let reference = matches!(self.tokens.get(at + 3), Some(Token::Word(b"R")));
        if reference { None } else { self.number(key) }
    }

    /// The token after the key `key`.
    fn value(&self, key: &[u8]) -> Option<Token<'a>> {
        self.tokens.get(self.key(key)? + 1).copied()
    }

    /// The index of the first name `key` among the tokens. A value that is the same name
    /// as a key is rare enough to read as that key.
    fn key(&self, key: &[u8]) -> Option<usize> {
        self.tokens.iter().position(|token| *token == Token::Name(key))
    }
}

/// The offset after the literal string that starts with the `(` at `start`: strings can
/// nest their parentheses, and `\` escapes the next byte.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut at = start;
    loop {
        match *bytes.get(at)? {
            b'\\' => at += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            _ => {}
        }
        at += 1;
    }
}

/// The offset of the first `needle` in `bytes` at or after `from`.
fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes.get(from..)?.windows(needle.len()).position(|window| window == needle).map(|at| from + at)
}

/// The white-space bytes of PDF.
fn is_space(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

/// The delimiter bytes of PDF.
fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

#[cfg(test)]
mod tests;
