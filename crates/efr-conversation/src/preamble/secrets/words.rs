//! The words of a command line, as far as the redaction needs them: the simple commands
//! of the line, and for each word its text without quotes and escapes and where each
//! byte of that text stands in the line.
//!
//! It is not a shell parser. It knows quotes, backslashes and the operators that end a
//! command or start a redirection, and nothing else: no expansion, no comment, no
//! here-document. A here-document's lines read as commands of their own.

use std::ops::Range;

/// One word of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Word {
    /// The text without its quotes and escapes.
    pub(super) text: String,
    /// For each byte of `text`, the byte of the line where it stands.
    at: Vec<usize>,
}

impl Word {
    /// The bytes of the line that hold `text[from..]`. The quotes around them stay
    /// out, so a redaction keeps the quotes of the word.
    pub(super) fn line_range(&self, from: usize) -> Option<Range<usize>> {
        self.line_range_to(from, self.text.len())
    }

    /// The bytes of the line that hold `text[from..to]`.
    pub(super) fn line_range_to(&self, from: usize, to: usize) -> Option<Range<usize>> {
        let start = *self.at.get(from)?;
        let end = self.at.get(to.checked_sub(1)?)?.checked_add(1)?;
        (from < to).then_some(start..end)
    }

    fn push(&mut self, at: usize, c: char) {
        self.text.push(c);
        self.at.extend((0..c.len_utf8()).map(|offset| at + offset));
    }
}

/// The simple commands of `line`, each as its words. The operators that end a command
/// (`;`, `|`, `&`, `(`, `)`, a newline) split the line; a redirection operator ends the
/// word before it and is not a word itself.
pub(super) fn commands(line: &str) -> Vec<Vec<Word>> {
    let mut lexer = Lexer { commands: Vec::new(), command: Vec::new(), word: None };
    let mut chars = line.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '\'' => {
                let word = lexer.word();
                for (at, c) in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(at, c);
                }
            }
            '$' if chars.peek().is_some_and(|(_, next)| *next == '\'') => {
                chars.next();
                let word = lexer.word();
                while let Some((at, c)) = chars.next() {
                    match c {
                        '\'' => break,
                        '\\' => {
                            word.push(at, c);
                            if let Some((at, next)) = chars.next() {
                                word.push(at, next);
                            }
                        }
                        _ => word.push(at, c),
                    }
                }
            }
            '"' => {
                let word = lexer.word();
                while let Some((at, c)) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if chars.peek().is_some_and(|(_, next)| {
                            matches!(next, '$' | '`' | '"' | '\\' | '\n')
                        }) =>
                        {
                            if let Some((at, next)) = chars.next() {
                                word.push(at, next);
                            }
                        }
                        _ => word.push(at, c),
                    }
                }
            }
            '\\' => {
                if let Some((at, next)) = chars.next() {
                    lexer.word().push(at, next);
                }
            }
            '\n' | ';' | '(' | ')' | '|' => lexer.end_command(),
            '&' if chars.peek().is_some_and(|(_, next)| *next == '>') => lexer.end_word(),
            '&' => lexer.end_command(),
            '<' | '>' => {
                lexer.end_word();
                // `>&2`, `<&0`: the `&` belongs to the redirection.
                if chars.peek().is_some_and(|(_, next)| *next == '&') {
                    chars.next();
                }
            }
            c if c.is_whitespace() => lexer.end_word(),
            _ => lexer.word().push(at, c),
        }
    }
    lexer.end_command();
    lexer.commands
}

struct Lexer {
    commands: Vec<Vec<Word>>,
    command: Vec<Word>,
    word: Option<Word>,
}

impl Lexer {
    /// The word that is being read; a new one when none is.
    fn word(&mut self) -> &mut Word {
        self.word.get_or_insert_with(|| Word { text: String::new(), at: Vec::new() })
    }

    fn end_word(&mut self) {
        if let Some(word) = self.word.take() {
            self.command.push(word);
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        if !self.command.is_empty() {
            self.commands.push(std::mem::take(&mut self.command));
        }
    }
}

#[cfg(test)]
mod tests;
