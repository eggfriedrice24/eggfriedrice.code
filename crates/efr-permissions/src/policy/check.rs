//! Checks of a command's words that a list of words cannot express: a `sed` script
//! that only prints, and git operands that can only be ref names.
//!
//! Each check is an allowlist. It reads the words the way the program reads them as far
//! as it must to prove the command harmless, and anything it does not know, or reads
//! another way than the program might, fails the check, so the command matches no row.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A check of the words after a [`CommandPattern`](super::CommandPattern)'s `args`.
///
/// The enum is deliberately exhaustive, like the other rule types: a new check must be
/// matched everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// `sed` that only prints: `-n` or `--quiet` is given; the only other options are
    /// `-e` and `--expression` with a script, `-E`, `-r`, `-u`, `--regexp-extended`,
    /// `--posix`, `--sandbox` and `--unbuffered`; and each script holds only addresses
    /// (line numbers, `$`, `first~step`, `/regex/` without a bracket expression, and
    /// ranges of them, `addr,+N` and `addr,~N`), `!`, and the commands `p`, `l`, `=`,
    /// `q` and `Q`, separated by `;` or newlines. So no script file (`-f`), no in-place
    /// edit (`-i`), no `w`, `W`, `r`, `R` or `e` command and no `s` command, whose `w`
    /// and `e` flags write a file and run a program.
    SedPrintOnly,
    /// Every operand is a git ref name, such as `main`, `origin`, `v1.2` or
    /// `feature/x`, and no word is `--`: letters, digits, `.`, `_`, `/`, `-`, `~` and
    /// `^`, starting with a letter, a digit or `_`, without `..`, `//` or a trailing
    /// `/` or `.`. So `git checkout -- .`, `git checkout .`, a quoted pathspec such as
    /// `'*'` and a URL such as `https://host/x` fail.
    RefNames,
}

impl Check {
    /// The program whose words the check reads, when it reads the words of one program
    /// only.
    pub(crate) const fn program(self) -> Option<&'static str> {
        match self {
            Check::SedPrintOnly => Some("sed"),
            Check::RefNames => None,
        }
    }

    /// The name of the check in a policy file, such as `sed_print_only`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Check::SedPrintOnly => "sed_print_only",
            Check::RefNames => "ref_names",
        }
    }

    /// True when `words`, the words after a pattern's `args`, pass the check.
    pub(crate) fn accepts(self, words: &[String]) -> bool {
        match self {
            Check::SedPrintOnly => sed_prints_only(words),
            Check::RefNames => words
                .iter()
                .all(|word| word != "--" && (word.starts_with('-') || is_ref_name(word))),
        }
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// True for a word that git can only read as a ref or remote name, never as a path
/// pattern or a URL.
fn is_ref_name(word: &str) -> bool {
    let mut chars = word.chars();
    let starts_well =
        chars.next().is_some_and(|first| first.is_ascii_alphanumeric() || first == '_');
    starts_well
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | '~' | '^'))
        && !word.contains("..")
        && !word.contains("//")
        && !word.ends_with(['/', '.'])
}

/// True when the arguments of `sed` make it print and nothing else.
fn sed_prints_only(args: &[String]) -> bool {
    let mut quiet = false;
    let mut scripts: Vec<&str> = Vec::new();
    let mut operands: Vec<&str> = Vec::new();
    let mut words = args.iter();
    let mut after_options = false;
    while let Some(word) = words.next() {
        if after_options || word == "-" || !word.starts_with('-') {
            operands.push(word);
            continue;
        }
        if word == "--" {
            after_options = true;
            continue;
        }
        if let Some(long) = word.strip_prefix("--") {
            // NOTE: GNU sed takes any unambiguous abbreviation (`--in` is `--in-place`),
            // so only the full names of the options below pass.
            match long.split_once('=') {
                Some(("expression", script)) => scripts.push(script),
                Some(_) => return false,
                None => match long {
                    "quiet" | "silent" => quiet = true,
                    "regexp-extended" | "posix" | "sandbox" | "unbuffered" => {}
                    "expression" => match words.next() {
                        Some(script) => scripts.push(script),
                        None => return false,
                    },
                    _ => return false,
                },
            }
            continue;
        }
        let letters = &word[1..];
        for (at, letter) in letters.char_indices() {
            match letter {
                'n' => quiet = true,
                'E' | 'r' | 'u' => {}
                'e' => {
                    let rest = &letters[at + 1..];
                    let script =
                        if rest.is_empty() { words.next().map(String::as_str) } else { Some(rest) };
                    match script {
                        Some(script) => scripts.push(script),
                        None => return false,
                    }
                    break;
                }
                _ => return false,
            }
        }
    }
    if scripts.is_empty() {
        // Without `-e`, the first operand is the script and the rest are files.
        match operands.first() {
            Some(script) => scripts.push(script),
            None => return false,
        }
    }
    quiet && scripts.iter().all(|script| script_prints_only(script))
}

/// True when the sed `script` holds only addresses and the commands `p`, `l`, `=`, `q`
/// and `Q`.
fn script_prints_only(script: &str) -> bool {
    let mut cursor = Cursor { chars: script.chars().collect(), at: 0 };
    loop {
        cursor.skip(|c| matches!(c, ' ' | '\t' | ';' | '\n'));
        if cursor.done() {
            return true;
        }
        match cursor.address() {
            Some(true) => {
                cursor.skip_blanks();
                if cursor.eat(',') {
                    cursor.skip_blanks();
                    let second = match cursor.address() {
                        Some(true) => true,
                        Some(false) => cursor.relative(),
                        None => false,
                    };
                    if !second {
                        return false;
                    }
                }
            }
            Some(false) => {}
            None => return false,
        }
        cursor.skip_blanks();
        if cursor.eat('!') {
            cursor.skip_blanks();
        }
        match cursor.next() {
            Some('p' | '=') => {}
            Some('l' | 'q' | 'Q') => {
                cursor.skip_blanks();
                cursor.skip(|c| c.is_ascii_digit());
            }
            _ => return false,
        }
        cursor.skip_blanks();
        if !(cursor.done() || cursor.eat(';') || cursor.eat('\n')) {
            return false;
        }
    }
}

/// A position in a sed script.
struct Cursor {
    chars: Vec<char>,
    at: usize,
}

impl Cursor {
    fn done(&self) -> bool {
        self.at >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let next = self.peek();
        self.at += usize::from(next.is_some());
        next
    }

    fn eat(&mut self, wanted: char) -> bool {
        let found = self.peek() == Some(wanted);
        self.at += usize::from(found);
        found
    }

    fn skip(&mut self, while_true: impl Fn(char) -> bool) -> usize {
        let start = self.at;
        while self.peek().is_some_and(&while_true) {
            self.at += 1;
        }
        self.at - start
    }

    fn skip_blanks(&mut self) {
        self.skip(|c| matches!(c, ' ' | '\t'));
    }

    /// One address: a line number, `first~step`, `$` or `/regex/` with the flags `I`
    /// and `M`. `Some(false)`, with nothing read, when there is none; `None` for an
    /// address this check does not accept, such as a regex that is not closed on its
    /// line or holds a bracket expression.
    fn address(&mut self) -> Option<bool> {
        match self.peek() {
            Some(c) if c.is_ascii_digit() => {
                self.skip(|c| c.is_ascii_digit());
                if self.eat('~') && self.skip(|c| c.is_ascii_digit()) == 0 {
                    return None;
                }
                Some(true)
            }
            Some('$') => {
                self.at += 1;
                Some(true)
            }
            Some('/') => {
                self.at += 1;
                if !self.regex() {
                    return None;
                }
                self.skip(|c| matches!(c, 'I' | 'M'));
                Some(true)
            }
            _ => Some(false),
        }
    }

    /// The rest of a `/regex/` after its first `/`, through the closing `/`.
    ///
    /// NOTE: GNU sed ends the regex at the first `/` without a backslash, while BSD sed
    /// skips a `/` inside a bracket expression. A script read one way here and another
    /// way by sed could hide a `w` command in what this reads as a regex, so a bracket
    /// expression fails the check.
    fn regex(&mut self) -> bool {
        while let Some(c) = self.next() {
            match c {
                '/' => return true,
                '\\' => match self.next() {
                    Some('\n') | None => return false,
                    Some(_) => {}
                },
                '[' | '\n' => return false,
                _ => {}
            }
        }
        false
    }

    /// The second address of a range in the forms `+N` and `~N`.
    fn relative(&mut self) -> bool {
        if self.eat('+') || self.eat('~') {
            return self.skip(|c| c.is_ascii_digit()) > 0;
        }
        false
    }
}

#[cfg(test)]
mod tests;
