//! A command line split into simple commands and words, as far as its text shows, for
//! finding the paths a command names.
//!
//! The permission engine splits lines strictly and asks for anything it cannot see
//! through. This split serves the other side: every path it finds becomes a declared
//! requirement, and a declared requirement can only make a decision stricter, so it
//! never fails. It reads quotes and backslashes as zsh does, starts a new command at
//! `;`, `&`, `|`, newlines, parentheses, braces, backquotes and `$(`, so the commands
//! inside a substitution are seen too, and keeps the targets of redirections apart.

/// One word, unquoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Word {
    /// The word as the program receives it, as far as the text shows; an expansion
    /// stays as written.
    pub(super) text: String,
    /// True when the word starts with a `~` outside quotes followed by `/` or nothing,
    /// which the shell replaces with the home directory.
    pub(super) tilde: bool,
    /// Where the first pattern character outside quotes is, in bytes of `text`.
    pub(super) pattern: Option<usize>,
    /// True when the word holds an expansion whose value the text does not show.
    pub(super) expansion: bool,
}

impl Word {
    fn new() -> Self {
        Word { text: String::new(), tilde: false, pattern: None, expansion: false }
    }
}

/// One simple command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Command {
    /// The words, assignments first, then the program and its arguments.
    pub(super) words: Vec<Word>,
    /// The files that `<` reads.
    pub(super) inputs: Vec<Word>,
    /// The files that `>`, `>>`, `&>` and their forms write, without `/dev/null`.
    pub(super) outputs: Vec<Word>,
}

impl Command {
    /// The words after the leading assignments: the program first.
    pub(super) fn program_and_args(&self) -> &[Word] {
        let assignments = self.words.iter().take_while(|word| is_assignment(&word.text)).count();
        &self.words[assignments..]
    }

    /// True when the command names nothing: no word with text and no redirection.
    fn is_empty(&self) -> bool {
        self.words.iter().all(|word| word.text.is_empty())
            && self.inputs.is_empty()
            && self.outputs.is_empty()
    }
}

/// A whole line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Line {
    /// The simple commands, in order, including those inside substitutions and groups.
    pub(super) commands: Vec<Command>,
    /// True when the line holds a construct that the words alone do not show: a
    /// substitution, an expansion, a group, a here-document or an unclosed quote.
    pub(super) opaque: bool,
}

/// Where a word being read will go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Word,
    Input,
    Output,
    /// The delimiter of a here-document, or a descriptor to copy: nothing to keep.
    Skip,
}

/// Splits `line`. It never fails: what it cannot read, it reads as text and marks the
/// line opaque.
pub(super) fn split(line: &str) -> Line {
    let mut splitter = Splitter {
        chars: line.chars().collect(),
        at: 0,
        line: Line::default(),
        command: Command::default(),
        word: None,
        slot: Slot::Word,
    };
    splitter.run();
    splitter.line
}

/// True when `text` is `NAME=value` or `NAME+=value`.
pub(super) fn is_assignment(text: &str) -> bool {
    let Some((name, _)) = text.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

struct Splitter {
    chars: Vec<char>,
    at: usize,
    line: Line,
    command: Command,
    word: Option<Word>,
    slot: Slot,
}

impl Splitter {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.at + offset).copied()
    }

    fn run(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' => {
                    self.end_word();
                    self.at += 1;
                }
                '\n' | ';' | '&' | '|' | '(' | ')' | '{' | '}' | '`' => {
                    if c == '&' && self.peek_at(1) == Some('>') {
                        self.end_word();
                        self.at += 2;
                        self.output_operator();
                        continue;
                    }
                    if matches!(c, '(' | ')' | '{' | '}' | '`') {
                        self.line.opaque = true;
                    }
                    self.end_command();
                    self.at += 1;
                }
                '<' | '>' => {
                    let descriptor = self.word.as_ref().is_some_and(|word| {
                        !word.text.is_empty() && word.text.bytes().all(|byte| byte.is_ascii_digit())
                    });
                    if descriptor {
                        self.word = None;
                    } else {
                        self.end_word();
                    }
                    self.redirect(c);
                }
                '\'' => {
                    self.at += 1;
                    self.single_quoted();
                }
                '"' => {
                    self.at += 1;
                    self.double_quoted();
                }
                '\\' => {
                    self.at += 1;
                    if let Some(escaped) = self.peek() {
                        self.at += 1;
                        if escaped != '\n' {
                            self.current().text.push(escaped);
                        }
                    }
                }
                '$' => self.dollar(),
                '~' if self.word.is_none() => {
                    self.current().text.push('~');
                    self.at += 1;
                    let ends = matches!(
                        self.peek(),
                        None | Some('/' | ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | ')')
                    );
                    self.current().tilde = ends;
                }
                '*' | '?' | '[' | '~' | '^' | '#' => {
                    let word = self.current();
                    if word.pattern.is_none() {
                        word.pattern = Some(word.text.len());
                    }
                    word.text.push(c);
                    self.at += 1;
                }
                '!' => {
                    self.line.opaque = true;
                    self.current().text.push('!');
                    self.at += 1;
                }
                c => {
                    self.current().text.push(c);
                    self.at += 1;
                }
            }
        }
        self.end_command();
    }

    /// The word being read, started when there is none.
    fn current(&mut self) -> &mut Word {
        self.word.get_or_insert_with(Word::new)
    }

    fn end_word(&mut self) {
        let Some(word) = self.word.take() else {
            return;
        };
        match std::mem::replace(&mut self.slot, Slot::Word) {
            Slot::Word => self.command.words.push(word),
            Slot::Input => self.command.inputs.push(word),
            Slot::Output if word.text != "/dev/null" => self.command.outputs.push(word),
            Slot::Output | Slot::Skip => {}
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        self.slot = Slot::Word;
        let command = std::mem::take(&mut self.command);
        if !command.is_empty() {
            self.line.commands.push(command);
        }
    }

    /// A redirection operator that starts with `c` at the cursor.
    fn redirect(&mut self, c: char) {
        self.at += 1;
        match (c, self.peek()) {
            (_, Some('(')) => {
                // `<(...)` and `>(...)`: a command of its own.
                self.line.opaque = true;
                self.end_command();
                self.at += 1;
            }
            ('<', Some('<')) => {
                self.line.opaque = true;
                while self.peek() == Some('<') {
                    self.at += 1;
                }
                self.slot = Slot::Skip;
            }
            ('<' | '>', Some('&')) => {
                self.at += 1;
                // A descriptor number copies a descriptor; a word after `>&` is a file.
                self.slot =
                    if c == '>' && !self.peek().is_some_and(|c| c.is_ascii_digit() || c == '-') {
                        Slot::Output
                    } else {
                        Slot::Skip
                    };
            }
            ('<', Some('>')) => {
                self.at += 1;
                self.slot = Slot::Output;
            }
            ('<', _) => self.slot = Slot::Input,
            ('>', _) => self.output_operator(),
            _ => {}
        }
    }

    /// The rest of an output operator after its first `>`: `>`, `|` or `!`.
    fn output_operator(&mut self) {
        while matches!(self.peek(), Some('>' | '|' | '!')) {
            self.at += 1;
        }
        self.slot = Slot::Output;
    }

    /// `$` outside single quotes: `$HOME` at the start of a word is the home directory,
    /// a substitution starts a command of its own, and any other expansion stays in the
    /// word as text.
    fn dollar(&mut self) {
        if self.home_variable() {
            return;
        }
        match self.peek_at(1) {
            Some('(') => {
                self.line.opaque = true;
                self.end_command();
                self.at += 2;
            }
            Some('{') => {
                self.line.opaque = true;
                let word = self.current();
                word.expansion = true;
                let mut depth = 0;
                while let Some(c) = self.peek() {
                    self.at += 1;
                    self.current().text.push(c);
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                self.line.opaque = true;
                let word = self.current();
                word.expansion = true;
                word.text.push('$');
                self.at += 1;
                while let Some(c) = self.peek().filter(|c| c.is_ascii_alphanumeric() || *c == '_') {
                    self.current().text.push(c);
                    self.at += 1;
                }
            }
        }
    }

    /// Reads `$HOME` or `${HOME}` at the start of a word as `~`, whose value the tool
    /// knows; true when it did.
    fn home_variable(&mut self) -> bool {
        if self.word.as_ref().is_some_and(|word| !word.text.is_empty()) {
            return false;
        }
        let rest: String = self.chars[self.at + 1..].iter().take(7).collect();
        let name_char = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let length = if rest.starts_with("{HOME}") {
            7
        } else if rest.starts_with("HOME") && !name_char(rest.chars().nth(4)) {
            5
        } else {
            return false;
        };
        self.at += length;
        self.line.opaque = true;
        let ends = matches!(
            self.peek(),
            None | Some('/' | '"' | ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | ')')
        );
        let word = self.current();
        word.text.push('~');
        word.tilde = ends;
        true
    }

    fn single_quoted(&mut self) {
        // NOTE: `''` is a word of its own, so the word starts before any character.
        self.current();
        while let Some(c) = self.peek() {
            self.at += 1;
            if c == '\'' {
                return;
            }
            self.current().text.push(c);
        }
        self.line.opaque = true;
    }

    fn double_quoted(&mut self) {
        self.current();
        while let Some(c) = self.peek() {
            match c {
                '"' => {
                    self.at += 1;
                    return;
                }
                '\\' => {
                    self.at += 1;
                    match self.peek() {
                        Some(escaped @ ('$' | '`' | '"' | '\\')) => {
                            self.at += 1;
                            self.current().text.push(escaped);
                        }
                        Some('\n') => self.at += 1,
                        _ => self.current().text.push('\\'),
                    }
                }
                '$' if self.peek_at(1) != Some('(') => self.dollar(),
                '$' | '`' => {
                    // The command inside is seen as a command of its own; the rest of
                    // the quoted text is read as words, which may only add paths.
                    self.line.opaque = true;
                    if self.word.as_ref().is_some_and(|word| word.text.is_empty()) {
                        self.word = None;
                    }
                    self.end_command();
                    self.at += if c == '$' { 2 } else { 1 };
                    return;
                }
                c => {
                    self.at += 1;
                    self.current().text.push(c);
                }
            }
        }
        self.line.opaque = true;
    }
}

#[cfg(test)]
mod tests;
