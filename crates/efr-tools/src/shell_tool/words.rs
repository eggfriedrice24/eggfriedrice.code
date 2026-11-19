//! A command line split into simple commands and words, as far as its text shows, for
//! finding the paths a command names.
//!
//! The permission engine splits lines strictly and asks for anything it cannot see
//! through. This split serves the other side: every path it finds becomes a declared
//! requirement, and a declared requirement can only make a decision stricter, so it
//! never fails. It reads quotes and backslashes as zsh does, starts a new command at
//! `;`, `&`, `|`, newlines, parentheses, braces, backquotes and `$(`, so the commands
//! inside a substitution are seen too, and keeps the targets of redirections apart. A
//! `<` or `>` in an arithmetic `(( ... ))` or `$(( ... ))`, or in a `[[ ... ]]`
//! condition, compares two values and names no file.

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
    /// How the command joins the one before it.
    pub(super) joint: Joint,
}

/// How a simple command joins the one before it, so a `cd` before `&&` can tell where
/// the next command runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Joint {
    /// It starts a list of the shell: at the start of the line, after `;`, a newline
    /// or a background `&`, or as the first command of a subshell or a substitution.
    #[default]
    Start,
    /// After `&&`: it runs only when the command before it succeeded.
    And,
    /// After `|` or `|&`: it runs in the same pipeline as the command before it.
    Pipe,
    /// After anything else, such as `||`, the end of a group or a word that the split
    /// cannot place.
    Other,
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
        condition: false,
        joint: Joint::Start,
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

/// Reserved words that may come before the `((` of an arithmetic command or the `[[`
/// of a condition, as in `if (( p > 90 ))`.
const KEYWORDS: &[&str] = &["if", "elif", "while", "until", "then", "else", "do", "!"];

struct Splitter {
    chars: Vec<char>,
    at: usize,
    line: Line,
    command: Command,
    word: Option<Word>,
    slot: Slot,
    /// True inside a `[[ ... ]]` condition, also past its `&&` and `||`.
    condition: bool,
    /// How the command being read joins the one before it.
    joint: Joint,
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
                    if !matches!(c, '&' | '|') {
                        self.condition = false;
                    }
                    if c == '('
                        && let Some(end) = self.arithmetic_command()
                    {
                        // NOTE: an arithmetic command, such as `(( p > 90 ))` or the
                        // head of `for (( i = 0; i < 3; i++ ))`, names no path.
                        self.end_command(Joint::Other);
                        self.at = end + 1;
                        continue;
                    }
                    let subshell = c == '(' && self.word.is_none() && self.command.words.is_empty();
                    let (joint, length) = match (c, self.peek_at(1)) {
                        ('&', Some('&')) => (Joint::And, 2),
                        ('&', Some('|' | '!')) => (Joint::Start, 2),
                        ('|', Some('|')) | (';', Some(';' | '&' | '|')) => (Joint::Other, 2),
                        ('|', Some('&')) => (Joint::Pipe, 2),
                        ('|', _) => (Joint::Pipe, 1),
                        (';' | '&' | '\n', _) => (Joint::Start, 1),
                        _ => (Joint::Other, 1),
                    };
                    if subshell {
                        self.start_list();
                    } else {
                        self.end_command(joint);
                    }
                    self.at += length;
                }
                '<' | '>' if self.condition && self.peek_at(1) != Some('(') => {
                    // NOTE: in `[[ a > b ]]` the operator compares two strings.
                    self.end_word();
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
        self.end_command(Joint::Start);
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
            Slot::Word => {
                // NOTE: `[` is a pattern character, so a `[[` without quotes has its
                // pattern at 0; `"[["` is a program, and its `>` writes.
                if word.text == "[[" && word.pattern == Some(0) && self.leads(&[]) {
                    self.condition = true;
                } else if word.text == "]]" {
                    self.condition = false;
                }
                self.command.words.push(word);
            }
            Slot::Input => self.command.inputs.push(word),
            Slot::Output if word.text != "/dev/null" => self.command.outputs.push(word),
            Slot::Output | Slot::Skip => {}
        }
    }

    /// True when the words of the command so far are only [`KEYWORDS`] or `also`, so a
    /// `((` or `[[` here starts a compound command.
    fn leads(&self, also: &[&str]) -> bool {
        self.command.words.iter().all(|word| {
            KEYWORDS.contains(&word.text.as_str()) || also.contains(&word.text.as_str())
        })
    }

    /// Where the `))` of an arithmetic command at the cursor ends, when a `((` there
    /// starts one ([`arithmetic_end`]).
    fn arithmetic_command(&self) -> Option<usize> {
        let starts = self.word.is_none() && self.slot == Slot::Word && self.leads(&["for"]);
        starts.then(|| arithmetic_end(&self.chars, self.at)).flatten()
    }

    /// Ends the command being read; the next one joins it as `next` says.
    fn end_command(&mut self, next: Joint) {
        self.end_word();
        self.slot = Slot::Word;
        let mut command = std::mem::take(&mut self.command);
        if command.is_empty() {
            // NOTE: no command between two operators, as in `a &&` and a newline
            // before `b`: a blank keeps the operator before it, and any other pair
            // reads as `Other`, so `(cd /x) && b` does not count as after the `cd`.
            self.joint = match (self.joint, next) {
                (Joint::Start, next) => next,
                (joint, Joint::Start) => joint,
                _ => Joint::Other,
            };
            return;
        }
        command.joint = std::mem::replace(&mut self.joint, next);
        self.line.commands.push(command);
    }

    /// Ends the command being read at the start of a subshell or a substitution, whose
    /// first command starts a list of its own.
    fn start_list(&mut self) {
        self.end_command(Joint::Other);
        self.joint = Joint::Start;
    }

    /// A redirection operator that starts with `c` at the cursor.
    fn redirect(&mut self, c: char) {
        self.at += 1;
        match (c, self.peek()) {
            (_, Some('(')) => {
                // `<(...)` and `>(...)`: a command of its own.
                self.line.opaque = true;
                self.condition = false;
                self.start_list();
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
            Some('(') if let Some(end) = arithmetic_end(&self.chars, self.at + 1) => {
                // NOTE: `$(( p > 90 ))` is a number, not a command.
                self.line.opaque = true;
                let text: String =
                    self.chars.get(self.at..=end).unwrap_or_default().iter().collect();
                let word = self.current();
                word.expansion = true;
                word.text.push_str(&text);
                self.at = end + 1;
            }
            Some('(') => {
                self.line.opaque = true;
                self.condition = false;
                self.start_list();
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
                '$' if self.peek_at(1) != Some('(')
                    || arithmetic_end(&self.chars, self.at + 1).is_some() =>
                {
                    self.dollar();
                }
                '$' | '`' => {
                    // The command inside is seen as a command of its own; the rest of
                    // the quoted text is read as words, which may only add paths.
                    self.line.opaque = true;
                    if self.word.as_ref().is_some_and(|word| word.text.is_empty()) {
                        self.word = None;
                    }
                    self.condition = false;
                    if c == '$' {
                        self.start_list();
                    } else {
                        self.end_command(Joint::Other);
                    }
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

/// Where the `))` that ends the arithmetic `((` at `open` ends, as the shell reads
/// `(( p > 90 ))`: the `)` that closes the inner `(` comes right before the one that
/// closes the outer. `None` when the text is a nested subshell, such as
/// `((cd x) && ls)`, and when it holds a quote, a backslash or a substitution, which
/// the split then reads as commands as before.
// NOTE: `efr-permissions/src/exits/scan.rs` has the same function for the exits of the
// `auto` sandbox, since this crate cannot name that one.
fn arithmetic_end(chars: &[char], open: usize) -> Option<usize> {
    if chars.get(open + 1) != Some(&'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut at = open + 2;
    while let Some(&c) = chars.get(at) {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ')' => return (chars.get(at + 1) == Some(&')')).then_some(at + 1),
            '\'' | '"' | '\\' | '`' => return None,
            '$' if chars.get(at + 1) == Some(&'(') => return None,
            _ => {}
        }
        at += 1;
    }
    None
}

#[cfg(test)]
mod tests;
