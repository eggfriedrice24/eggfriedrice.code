//! The lexer behind [`analyze`](super::analyze): characters in, words and operators out,
//! with quotes and escapes resolved as zsh resolves them.

use super::{Construct, is_plain_char};

/// What the lexer produces.
#[derive(Debug)]
pub(super) enum Token {
    Word(Word),
    /// A redirection that was checked and needs nothing more: input from a file, output
    /// to `/dev/null`, or one descriptor copied to another.
    Redirect,
    /// `;` or a newline.
    Separator,
    /// `&&`, `||`, `|` or `|&`.
    Join,
}

/// One word, unquoted.
#[derive(Debug)]
pub(super) struct Word {
    pub(super) text: String,
    /// Where the first `=` is, when everything before it was outside quotes.
    equals: Option<usize>,
    /// True when a pattern character stands outside quotes, so zsh may replace the
    /// word with the file names it matches.
    pub(super) pattern: bool,
}

impl Word {
    /// The variable that the word assigns, when it is `NAME=value` or `NAME+=value`.
    pub(super) fn assigned_name(&self) -> Option<&str> {
        let name = &self.text[..self.equals?];
        let name = name.strip_suffix('+').unwrap_or(name);
        let mut chars = name.chars();
        let first = chars.next()?;
        let valid = (first.is_ascii_alphabetic() || first == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        valid.then_some(name)
    }
}

/// A cursor over the characters of one line.
pub(super) struct Lexer {
    chars: Vec<char>,
    at: usize,
    /// True when an output redirection to a file is a plain redirection, not a
    /// construct: the one-command rule of an unsandboxed exit judges such paths apart.
    file_redirects: bool,
}

impl Lexer {
    pub(super) fn new(line: &str) -> Self {
        Lexer { chars: line.chars().collect(), at: 0, file_redirects: false }
    }

    /// Accepts an output redirection to any file.
    pub(super) fn with_file_redirects(mut self) -> Self {
        self.file_redirects = true;
        self
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.at + offset).copied()
    }

    pub(super) fn tokens(mut self) -> Result<Vec<Token>, Construct> {
        let mut tokens = Vec::new();
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' => self.at += 1,
                '\n' => {
                    self.at += 1;
                    tokens.push(Token::Separator);
                }
                ';' => {
                    self.at += 1;
                    if matches!(self.peek(), Some(';' | '&' | '|')) {
                        return Err(Construct::Syntax);
                    }
                    tokens.push(Token::Separator);
                }
                '&' => match self.peek_at(1) {
                    Some('&') => {
                        self.at += 2;
                        tokens.push(Token::Join);
                    }
                    Some('>') => {
                        self.at += 2;
                        if self.peek() == Some('>') {
                            self.at += 1;
                        }
                        if self.peek() == Some('|') {
                            self.at += 1;
                        }
                        self.output_target()?;
                        tokens.push(Token::Redirect);
                    }
                    _ => return Err(Construct::Background),
                },
                '|' => {
                    self.at += 1;
                    if matches!(self.peek(), Some('|' | '&')) {
                        self.at += 1;
                    }
                    tokens.push(Token::Join);
                }
                '<' | '>' => {
                    self.redirect()?;
                    tokens.push(Token::Redirect);
                }
                '0'..='9' if self.descriptor_then_redirect() => {
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.at += 1;
                    }
                    self.redirect()?;
                    tokens.push(Token::Redirect);
                }
                _ => tokens.push(Token::Word(self.word()?)),
            }
        }
        Ok(tokens)
    }

    /// True when digits at the cursor are a descriptor number, as in `2>/dev/null`.
    fn descriptor_then_redirect(&self) -> bool {
        let digits = self.chars[self.at..].iter().take_while(|c| c.is_ascii_digit()).count();
        matches!(self.peek_at(digits), Some('<' | '>'))
    }

    /// A redirection whose operator starts at the cursor with `<` or `>`.
    fn redirect(&mut self) -> Result<(), Construct> {
        match (self.peek(), self.peek_at(1)) {
            (Some('<'), Some('<')) => Err(Construct::HereDocument),
            (Some('<' | '>'), Some('(')) => Err(Construct::ProcessSubstitution),
            (Some('<'), Some('>')) if !self.file_redirects => Err(Construct::Redirection),
            (Some('<'), Some('>')) => {
                self.at += 2;
                self.output_target()
            }
            (Some('<' | '>'), Some('&')) => {
                self.at += 2;
                self.duplicate()
            }
            (Some('<'), _) => {
                self.at += 1;
                // Reading a file is a read like any other; the tool declares the path.
                self.target().map(|_| ())
            }
            (Some('>'), Some('!')) if !self.file_redirects => Err(Construct::Redirection),
            (Some('>'), Some('!')) => {
                self.at += 2;
                self.output_target()
            }
            (Some('>'), _) => {
                self.at += 1;
                if self.peek() == Some('>') {
                    self.at += 1;
                }
                if self.peek() == Some('|') {
                    self.at += 1;
                }
                self.output_target()
            }
            _ => Err(Construct::Syntax),
        }
    }

    /// After `>&` or `<&`: a descriptor number copies one descriptor to another, which
    /// writes no file. In zsh `>& word` sends both outputs to a file, so any other word
    /// must be `/dev/null`.
    fn duplicate(&mut self) -> Result<(), Construct> {
        if self.peek().is_some_and(|c| c.is_ascii_digit()) {
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.at += 1;
            }
            return match self.peek() {
                None | Some(' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>') => Ok(()),
                Some(_) => Err(Construct::Redirection),
            };
        }
        self.output_target()
    }

    /// The target of an output redirection, which may only be `/dev/null` unless
    /// redirections to files are accepted.
    fn output_target(&mut self) -> Result<(), Construct> {
        let target = self.target()?;
        if self.file_redirects || target.text == "/dev/null" {
            Ok(())
        } else {
            Err(Construct::Redirection)
        }
    }

    /// The word after a redirection operator.
    fn target(&mut self) -> Result<Word, Construct> {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.at += 1;
        }
        match self.peek() {
            None | Some('\n' | ';' | '&' | '|' | '<' | '>') => Err(Construct::Syntax),
            Some(_) => self.word(),
        }
    }

    /// One word at the cursor, quotes and escapes resolved.
    fn word(&mut self) -> Result<Word, Construct> {
        let mut text = String::new();
        let mut equals = None;
        let mut quoted = false;
        let mut pattern = false;
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' => break,
                '\'' => {
                    self.at += 1;
                    quoted = true;
                    self.single_quoted(&mut text)?;
                }
                '"' => {
                    self.at += 1;
                    quoted = true;
                    self.double_quoted(&mut text)?;
                }
                '\\' => {
                    quoted = true;
                    match self.peek_at(1) {
                        None | Some('\n') => return Err(Construct::Unfinished),
                        Some(escaped) if escaped.is_control() => {
                            return Err(Construct::Character { character: escaped });
                        }
                        Some(escaped) => {
                            text.push(escaped);
                            self.at += 2;
                        }
                    }
                }
                '$' => {
                    return Err(if self.peek_at(1) == Some('(') {
                        Construct::CommandSubstitution
                    } else {
                        Construct::Expansion
                    });
                }
                '`' => return Err(Construct::CommandSubstitution),
                '(' | ')' | '{' | '}' => return Err(Construct::Grouping),
                '!' => return Err(Construct::HistoryExpansion),
                '~' if text.is_empty() && !quoted => {
                    // `~` and `~/...` mean the home directory; `~user`, `~+` and `~1`
                    // mean other directories that the rules cannot see.
                    match self.peek_at(1) {
                        None | Some('/' | ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>') => {
                            text.push('~');
                            self.at += 1;
                        }
                        Some(_) => return Err(Construct::Character { character: '~' }),
                    }
                }
                '=' if text.is_empty() && !quoted => {
                    // zsh replaces `=name` with the path of the command `name`, and
                    // `=(...)` with a temporary file of a command's output.
                    return Err(if self.peek_at(1) == Some('(') {
                        Construct::ProcessSubstitution
                    } else {
                        Construct::Character { character: '=' }
                    });
                }
                '=' => {
                    if equals.is_none() && !quoted {
                        equals = Some(text.len());
                    }
                    text.push('=');
                    self.at += 1;
                }
                // NOTE: `~`, `^` and `#` are pattern characters with EXTENDED_GLOB,
                // which the user's startup files may set, and `#` starts a comment
                // with INTERACTIVE_COMMENTS. A pattern is safe only where its every
                // expansion starts with the same plain character, never with `-`.
                '*' | '?' | '[' | ']' | '~' | '^' | '#' => {
                    if text.is_empty() {
                        return Err(if matches!(c, '*' | '?' | '[' | ']') {
                            Construct::Glob
                        } else {
                            Construct::Character { character: c }
                        });
                    }
                    if text.starts_with('-') {
                        return Err(Construct::Glob);
                    }
                    pattern = true;
                    text.push(c);
                    self.at += 1;
                }
                c if is_plain_char(c) => {
                    text.push(c);
                    self.at += 1;
                }
                c => return Err(Construct::Character { character: c }),
            }
        }
        Ok(Word { text, equals, pattern })
    }

    /// The rest of a single-quoted string, up to and past the closing quote.
    fn single_quoted(&mut self, text: &mut String) -> Result<(), Construct> {
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                '\'' => return Ok(()),
                c if is_control(c) => return Err(Construct::Character { character: c }),
                c => text.push(c),
            }
        }
        Err(Construct::Unfinished)
    }

    /// The rest of a double-quoted string, up to and past the closing quote.
    fn double_quoted(&mut self, text: &mut String) -> Result<(), Construct> {
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                '"' => return Ok(()),
                '\\' => match self.peek() {
                    Some(escaped @ ('$' | '`' | '"' | '\\')) => {
                        text.push(escaped);
                        self.at += 1;
                    }
                    Some('\n') | None => return Err(Construct::Unfinished),
                    // Any other backslash stays, and the next character is read as it is.
                    Some(_) => text.push('\\'),
                },
                '$' => {
                    return Err(if self.peek() == Some('(') {
                        Construct::CommandSubstitution
                    } else {
                        Construct::Expansion
                    });
                }
                '`' => return Err(Construct::CommandSubstitution),
                '!' => return Err(Construct::HistoryExpansion),
                c if is_control(c) => return Err(Construct::Character { character: c }),
                c => text.push(c),
            }
        }
        Err(Construct::Unfinished)
    }
}

/// Control characters other than tab and newline, which a terminal may act on.
fn is_control(c: char) -> bool {
    c.is_control() && c != '\t' && c != '\n'
}
