//! Command lines split into simple commands, failing closed.
//!
//! A command rule names a program and its arguments, so it can only judge a line whose
//! every part is a plain program with plain arguments. [`analyze`] splits a line on
//! `;`, `&&`, `||`, `|` and newlines, with single quotes, double quotes and backslashes
//! read as zsh reads them, and returns the words of each simple command. Anything whose
//! meaning depends on the shell's state or that could run a second command it does not
//! show is a [`Construct`] instead: a command substitution, a process substitution, a
//! parameter or history expansion, a group, a here-document, an output redirection to
//! anything but `/dev/null`, a background job, an assignment that is not on the short
//! list of harmless ones, or a builtin such as `eval`, `source` or `alias`. Such a line
//! is judged only by the rules for every command line, which ask by default.
//!
//! The lexer is an allowlist. A character it does not know makes the line a construct,
//! which costs an approval; a character wrongly known would let an allowed program
//! smuggle in another command.

mod lexer;

use std::fmt;

use self::lexer::{Lexer, Token, Word};

/// Why a command line cannot be split into simple commands that a command rule judges.
///
/// The line is then judged only by the rules whose resource is `any`, which ask by
/// default.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Construct {
    /// `$(...)` or backquotes: the output of a command the rules do not see becomes part
    /// of the line.
    CommandSubstitution,
    /// `<(...)`, `>(...)` or `=(...)`: a command the rules do not see runs beside the
    /// line.
    ProcessSubstitution,
    /// `$NAME`, `${...}`, `$((...))` or `$'...'`: the word depends on the shell's state.
    Expansion,
    /// `!` outside single quotes: zsh replaces it with an earlier command line.
    HistoryExpansion,
    /// `(`, `)`, `{` or `}`: a subshell, a group, a function definition, a glob
    /// qualifier or a brace expansion.
    Grouping,
    /// A pattern at the start of a word, or in a word that starts with `-`: it may
    /// expand to a file name that the program reads as an option.
    Glob,
    /// `<<` or `<<<`: input written inside the line.
    HereDocument,
    /// An output redirection to anything but `/dev/null`, or a redirection that opens a
    /// file for writing or closes a descriptor.
    Redirection,
    /// `&` at the end of a command, `&|` or `&!`: a job that outlives the call.
    Background,
    /// An assignment without a command, which changes the shell for later commands, or
    /// before a command to a variable that is not on the list of harmless ones, such as
    /// `PATH` or `LD_PRELOAD`.
    Assignment,
    /// A builtin that runs a string as commands or changes how later commands run, such
    /// as `eval`, `exec`, `source`, `.` or `alias`.
    Builtin {
        /// The builtin.
        program: String,
    },
    /// An unclosed quote, a backslash at the end, or a backslash before a newline.
    Unfinished,
    /// A character that the shell may treat specially where it stands, such as `#`, a
    /// `~` that is not the start of `~/`, an `=` at the start of a word, a control
    /// character or a non-ASCII character outside quotes.
    Character {
        /// The character.
        character: char,
    },
    /// An operator without a command on each side, such as `&& ls` or `ls |`, or `;;`.
    Syntax,
    /// Nothing to run.
    Empty,
}

impl fmt::Display for Construct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Construct::CommandSubstitution => f.write_str("a command substitution"),
            Construct::ProcessSubstitution => f.write_str("a process substitution"),
            Construct::Expansion => f.write_str("a parameter or arithmetic expansion"),
            Construct::HistoryExpansion => f.write_str("a history expansion"),
            Construct::Grouping => f.write_str("a group, subshell or brace expansion"),
            Construct::Glob => f.write_str("a pattern that may expand to an option"),
            Construct::HereDocument => f.write_str("a here-document"),
            Construct::Redirection => f.write_str("a redirection to a file"),
            Construct::Background => f.write_str("a background job"),
            Construct::Assignment => f.write_str("a variable assignment"),
            Construct::Builtin { program } => write!(f, "the builtin {program:?}"),
            Construct::Unfinished => f.write_str("an unfinished quote or line"),
            Construct::Character { character } => write!(f, "the character {character:?}"),
            Construct::Syntax => f.write_str("an operator without a command on each side"),
            Construct::Empty => f.write_str("no command"),
        }
    }
}

/// One simple command of a line: the program and its arguments, unquoted, without the
/// harmless assignments and the redirections before or among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SimpleCommand {
    /// The program first, then its arguments. Never empty.
    pub(crate) words: Vec<String>,
}

impl SimpleCommand {
    /// The words joined by spaces, for a reason's text.
    pub(crate) fn text(&self) -> String {
        self.words.join(" ")
    }
}

/// Builtins that run a string as commands, or change how later commands run, so no
/// command rule may judge them: aliases, functions, options, traps, the command hash
/// table and the variables, `PATH` among them.
const BUILTINS: &[&str] = &[
    "eval",
    "exec",
    "source",
    ".",
    "alias",
    "unalias",
    "builtin",
    "fc",
    "r",
    "trap",
    "functions",
    "unfunction",
    "autoload",
    "enable",
    "disable",
    "zmodload",
    "emulate",
    "setopt",
    "unsetopt",
    "set",
    "unset",
    "export",
    "typeset",
    "declare",
    "local",
    "readonly",
    "integer",
    "float",
    "hash",
    "unhash",
    "rehash",
    "coproc",
    "noglob",
    "nocorrect",
    "bindkey",
    "zle",
    "zstyle",
    "compdef",
    "-",
];

/// Programs that run a command as another user. They always need approval: no rule
/// allows them, not even a rule for every command line.
pub(crate) const PRIVILEGED: &[&str] = &["sudo", "sudoedit", "doas", "su", "pkexec", "run0"];

/// Programs that run the program named among their arguments.
const WRAPPERS: &[&str] = &[
    "env",
    "nice",
    "nohup",
    "time",
    "timeout",
    "command",
    "builtin",
    "exec",
    "stdbuf",
    "ionice",
    "chrt",
    "taskset",
    "setsid",
    "flock",
    "xargs",
    "watch",
    "unbuffer",
    "noglob",
    "nocorrect",
    "-",
];

/// Variables that may be set for one command without changing which program runs or
/// what it executes.
const HARMLESS_VARIABLES: &[&str] =
    &["LANG", "LANGUAGE", "TZ", "COLUMNS", "LINES", "NO_COLOR", "TERM", "CLICOLOR"];

/// The simple commands of `line`, or the construct that keeps a command rule from
/// judging it.
pub(crate) fn analyze(line: &str) -> Result<Vec<SimpleCommand>, Construct> {
    let tokens = Lexer::new(line).tokens()?;
    let mut commands = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    // NOTE: an operator that joins two commands needs a command on each side; a
    // separator may stand alone, as at the end of a line or between blank lines.
    let mut joined = false;
    for token in tokens {
        match token {
            Token::Word(word) => words.push(word),
            Token::Redirect => {}
            Token::Separator | Token::Join => {
                let is_join = matches!(token, Token::Join);
                if words.is_empty() {
                    if joined || is_join {
                        return Err(Construct::Syntax);
                    }
                } else {
                    commands.push(simple_command(std::mem::take(&mut words))?);
                }
                joined = is_join;
            }
        }
    }
    if words.is_empty() {
        if joined {
            return Err(Construct::Syntax);
        }
    } else {
        commands.push(simple_command(words)?);
    }
    if commands.is_empty() {
        return Err(Construct::Empty);
    }
    Ok(commands)
}

/// The program of `command` or of a wrapper's command that runs as another user, by
/// base name, so `/usr/bin/sudo` counts.
pub(crate) fn privileged(command: &SimpleCommand) -> Option<&str> {
    let mut words = command.words.iter().map(|word| base_name(word));
    let program = words.next()?;
    if PRIVILEGED.contains(&program) {
        return Some(program);
    }
    if WRAPPERS.contains(&program) {
        return words.find(|word| PRIVILEGED.contains(word));
    }
    None
}

/// True when `command` changes the shell's directory, so the commands after it may run
/// elsewhere.
pub(crate) fn changes_directory(command: &SimpleCommand) -> bool {
    command
        .words
        .first()
        .is_some_and(|program| matches!(base_name(program), "cd" | "pushd" | "popd" | "chdir"))
}

/// A privileged program named anywhere in a line that [`analyze`] could not split, by a
/// coarse scan that would rather find one too many.
pub(crate) fn privileged_anywhere(line: &str) -> Option<&'static str> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')))
        .map(base_name)
        .find_map(|word| PRIVILEGED.iter().copied().find(|program| *program == word))
}

fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// One simple command from its words: assignments first, then the program.
fn simple_command(words: Vec<Word>) -> Result<SimpleCommand, Construct> {
    let mut words = words.into_iter().peekable();
    while let Some(name) = words.peek().and_then(Word::assigned_name) {
        let harmless = name.starts_with("LC_") || HARMLESS_VARIABLES.contains(&name);
        if !harmless {
            return Err(Construct::Assignment);
        }
        words.next();
    }
    let words: Vec<String> = words.map(|word| word.text).collect();
    let Some(program) = words.first() else {
        // Only assignments: they change the shell itself for every later command.
        return Err(Construct::Assignment);
    };
    if BUILTINS.contains(&program.as_str()) {
        return Err(Construct::Builtin { program: program.clone() });
    }
    Ok(SimpleCommand { words })
}

/// Characters that neither bash nor zsh treat specially inside a word outside quotes.
/// The list is an allowlist on purpose: a character that is missing makes a line fall
/// back to the rules for every command line, which is safe; a character wrongly present
/// would let an allowed program smuggle in another command.
pub(crate) fn is_plain_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '-' | '_' | '.' | '/' | ':' | ',' | '+' | '@' | '%' | '=')
}

#[cfg(test)]
mod tests;
