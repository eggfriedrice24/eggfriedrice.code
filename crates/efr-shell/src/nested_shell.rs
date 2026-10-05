//! Whether a command line starts an interactive shell or a REPL, read from its words.
//!
//! A shell at its prompt (`$ `, `user@host:~$ `) looks like a visible question: the
//! output is quiet and the cursor sits after some text. An answer typed there would run
//! as a command line in that shell, so a run whose command line starts one reports no
//! visible wait (`input.rs`). A REPL's prompt (`>>> `, `postgres=# `) is the same, so
//! here a REPL counts as a shell.
//!
//! The shells are `zsh`, `bash`, `sh`, `dash`, `fish` and `ksh` with neither a command
//! nor a script to run and their input from the terminal, `su` without a command, and
//! `ssh` without a remote command. The REPLs and the shells in a container are in
//! `repl.rs`. A program that runs a command of its own is followed into it: `sudo`
//! (whose `-i` and `-s` start a shell), `doas` (`-s`), `script` (a shell unless `-c`
//! names a command), `env`, `exec`, `nohup`, `nice`, `setsid`, `timeout` and the like.
//! The command text of a shell's or `su`'s `-c`, of `script -c`, of `eval`, of `sudo
//! -i` or `-s`, and of a remote command on a terminal (`ssh -t`) is read as a line of
//! its own.
//!
//! NOTE: this reads the words, not what runs. An alias, a function or a script that
//! starts a shell is not seen, nor is a REPL or a container that this file does not
//! name (`docker run -it`): their prompts still count as visible questions.

mod repl;

/// The programs that are a shell.
const SHELLS: &[&str] = &["zsh", "bash", "sh", "dash", "fish", "ksh"];

/// Programs that run the rest of their words as a command, with their short options
/// that take a value.
const WRAPPERS: &[(&str, &str)] = &[
    ("builtin", ""),
    ("command", ""),
    ("exec", "a"),
    ("nice", "n"),
    ("nocorrect", ""),
    ("noglob", ""),
    ("nohup", ""),
    ("setsid", ""),
    ("stdbuf", "ioe"),
    ("time", "of"),
    ("unbuffer", ""),
];

/// Words that may open a command without being its program.
const RESERVED: &[&str] = &["!", "{", "if", "then", "else", "elif", "while", "until", "do"];

/// How many lines and commands inside a line are read; a deeper one counts as no shell,
/// so a line such as `eval eval eval ...` costs little.
const MAX_DEPTH: usize = 8;

/// True when `line` starts an interactive shell or a REPL, directly or through a
/// program that runs one.
pub(crate) fn starts_shell(line: &str) -> bool {
    line_starts_shell(line, 0)
}

fn line_starts_shell(line: &str, depth: usize) -> bool {
    depth <= MAX_DEPTH && commands(line).iter().any(|command| command.starts_shell(depth))
}

/// One simple command of a line.
#[derive(Debug, Default, PartialEq, Eq)]
struct Command {
    words: Vec<String>,
    /// True when its input comes from a pipe, as after `|`.
    piped: bool,
}

impl Command {
    fn starts_shell(&self, depth: usize) -> bool {
        let mut terminal_input = !self.piped;
        let mut argv: Vec<&str> = Vec::new();
        let mut words = self.words.iter();
        while let Some(word) = words.next() {
            if let Some(target_follows) = redirection(word) {
                terminal_input &= !reads_input(word);
                if target_follows {
                    words.next();
                }
                continue;
            }
            if argv.is_empty() && (RESERVED.contains(&word.as_str()) || is_assignment(word)) {
                continue;
            }
            argv.push(word);
        }
        argv_starts_shell(&argv, terminal_input, depth)
    }
}

/// The simple commands of `line`, split at `;`, `&`, `&&`, `|`, `||`, newlines,
/// parentheses and backquotes, with quotes and backslashes taken away. The inside of a
/// substitution is read as commands of the line, and a comment is dropped.
fn commands(line: &str) -> Vec<Command> {
    let mut split = Split::default();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => split.end_word(),
            '\'' => {
                split.in_word = true;
                split.word.extend(chars.by_ref().take_while(|&c| c != '\''));
            }
            '"' => {
                split.in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            let escaped =
                                chars.next_if(|&next| matches!(next, '"' | '\\' | '$' | '`'));
                            split.word.push(escaped.unwrap_or('\\'));
                        }
                        c => split.word.push(c),
                    }
                }
            }
            '$' if chars.next_if_eq(&'\'').is_some() => {
                split.in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '\'' => break,
                        '\\' => split.word.extend(chars.next()),
                        c => split.word.push(c),
                    }
                }
            }
            '\\' => {
                split.in_word = true;
                split.word.extend(chars.next().filter(|&c| c != '\n'));
            }
            '#' if !split.in_word => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                split.end_command(false);
            }
            ';' | '\n' | '(' | ')' | '`' => split.end_command(false),
            // `2>&1` and `&>file` are redirections, not the end of a command.
            '&' if split.word.ends_with(['<', '>']) || chars.peek() == Some(&'>') => {
                split.in_word = true;
                split.word.push(c);
            }
            '&' => {
                chars.next_if_eq(&'&');
                split.end_command(false);
            }
            '|' if chars.next_if_eq(&'|').is_some() => split.end_command(false),
            '|' => {
                chars.next_if_eq(&'&');
                split.end_command(true);
            }
            c => {
                split.in_word = true;
                split.word.push(c);
            }
        }
    }
    split.end_command(false);
    split.commands
}

/// The state of [`commands`].
#[derive(Debug, Default)]
struct Split {
    commands: Vec<Command>,
    current: Command,
    word: String,
    /// True once the word has begun, so that `''` is a word.
    in_word: bool,
}

impl Split {
    fn end_word(&mut self) {
        if self.in_word {
            self.current.words.push(std::mem::take(&mut self.word));
            self.in_word = false;
        }
    }

    /// Ends the command; the next one reads from a pipe when `piped`.
    fn end_command(&mut self, piped: bool) {
        self.end_word();
        let done = std::mem::replace(&mut self.current, Command { words: Vec::new(), piped });
        if !done.words.is_empty() {
            self.commands.push(done);
        }
    }
}

/// For a redirection such as `>out`, `2>&1` or `<`, whether its target is the next word;
/// `None` for any other word.
fn redirection(word: &str) -> Option<bool> {
    let operator = word.trim_start_matches(|c: char| c.is_ascii_digit());
    let operator = operator.strip_prefix('&').unwrap_or(operator);
    if !operator.starts_with(['<', '>']) {
        return None;
    }
    Some(operator.trim_start_matches(['<', '>', '&', '|']).is_empty())
}

/// True for a redirection of the input: `<file`, `<<EOF`, `<<<text` or `0<file`.
fn reads_input(word: &str) -> bool {
    let operator = word.trim_start_matches(|c: char| c.is_ascii_digit());
    let fd = word.strip_suffix(operator).unwrap_or_default();
    operator.starts_with('<') && (fd.is_empty() || fd == "0")
}

/// True for an assignment such as `LANG=C` before a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let name = name.strip_suffix('+').unwrap_or(name);
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn argv_starts_shell(argv: &[&str], terminal_input: bool, depth: usize) -> bool {
    let Some((first, args)) = argv.split_first() else {
        return false;
    };
    if depth > MAX_DEPTH {
        return false;
    }
    let depth = depth + 1;
    let program = first.rsplit('/').next().unwrap_or(first);
    match program {
        "su" => su_starts_shell(args, depth),
        "ssh" => ssh_starts_shell(args, terminal_input, depth),
        "sudo" => sudo_starts_shell(args, terminal_input, depth),
        "doas" => {
            let parsed = parse(args, "uC", &[]);
            match parsed.operands.as_slice() {
                _ if parsed.has('L') => false,
                [] => parsed.has('s'),
                command => argv_starts_shell(command, terminal_input, depth),
            }
        }
        "script" => script_starts_shell(args, depth),
        "env" => {
            let parsed = parse(args, "uCS", &["unset", "chdir", "split-string"]);
            let command: Vec<&str> =
                parsed.operands.iter().copied().skip_while(|word| is_assignment(word)).collect();
            match parsed.value('S').or(parsed.long_value("split-string")) {
                Some(split) => line_starts_shell(&format!("{split} {}", command.join(" ")), depth),
                None => argv_starts_shell(&command, terminal_input, depth),
            }
        }
        "eval" => line_starts_shell(&args.join(" "), depth),
        "timeout" => {
            let parsed = parse(args, "sk", &["signal", "kill-after"]);
            // The first operand is the duration.
            parsed
                .operands
                .get(1..)
                .is_some_and(|command| argv_starts_shell(command, terminal_input, depth))
        }
        "command" if args.iter().any(|arg| *arg == "-v" || *arg == "-V") => false,
        shell if SHELLS.contains(&shell) => shell_starts_shell(args, terminal_input, depth),
        other => match repl::starts_repl(other, args, terminal_input, depth) {
            Some(starts) => starts,
            None => WRAPPERS.iter().find(|(name, _)| *name == other).is_some_and(|(_, valued)| {
                argv_starts_shell(&parse(args, valued, &[]).operands, terminal_input, depth)
            }),
        },
    }
}

/// A shell is interactive with neither `-c` nor a script, and with its input from the
/// terminal unless `-i` says so anyway. The text of `-c` is read as a line.
fn shell_starts_shell(args: &[&str], terminal_input: bool, depth: usize) -> bool {
    let parsed = parse(args, "oOC", &["rcfile", "init-file", "init-command"]);
    if parsed.has_long("version") || parsed.has_long("help") {
        return false;
    }
    if parsed.has('c') || parsed.has_long("command") {
        let command = parsed.long_value("command").or(parsed.operands.first().copied());
        return command.is_some_and(|command| line_starts_shell(command, depth));
    }
    parsed.operands.is_empty() && (terminal_input || parsed.has('i'))
}

/// `su` starts the user's shell unless `-c` gives it a command, whose text is read as a
/// line. Options may also follow the user.
fn su_starts_shell(args: &[&str], depth: usize) -> bool {
    let parsed = parse_all(
        args,
        "csgGw",
        &["command", "session-command", "shell", "group", "supp-group", "whitelist-environment"],
    );
    let command =
        parsed.value('c').or(parsed.long_value("command")).or(parsed.long_value("session-command"));
    command.is_none_or(|command| line_starts_shell(command, depth))
}

/// `script` runs the user's shell on a terminal of its own unless `-c` names a command,
/// whose text is read as a line. Options may also follow the typescript file.
fn script_starts_shell(args: &[&str], depth: usize) -> bool {
    let parsed = parse_all(
        args,
        "cEIOBTmo",
        &[
            "command",
            "echo",
            "log-in",
            "log-out",
            "log-io",
            "log-timing",
            "logging-format",
            "output-limit",
        ],
    );
    if parsed.has('V') || parsed.has('h') || parsed.has_long("version") || parsed.has_long("help") {
        return false;
    }
    let command = parsed.value('c').or(parsed.long_value("command"));
    command.is_none_or(|command| line_starts_shell(command, depth))
}

/// `ssh` starts a shell on the remote host when it has no remote command, reads its
/// input from the terminal (or `-t` asks for a terminal anyway) and opens a session at
/// all. A remote command on a terminal (`-t`) is read as a line.
fn ssh_starts_shell(args: &[&str], terminal_input: bool, depth: usize) -> bool {
    const VALUED: &str = "BbcDEeFIiJLlmOopQRSWw";
    let parsed = parse(args, VALUED, &[]);
    let Some((_, after)) = parsed.operands.split_first() else {
        return false;
    };
    // OpenSSH takes options after the destination too, up to the remote command.
    let rest = parse(after, VALUED, &[]);
    let has = |letter| parsed.has(letter) || rest.has(letter);
    // Forwarding only, in the background, without a terminal, or no session at all.
    if ['N', 'W', 'O', 'G', 'V', 'Q', 'f', 'T'].into_iter().any(has) {
        return false;
    }
    if rest.operands.is_empty() {
        return terminal_input || has('t');
    }
    has('t') && line_starts_shell(&rest.operands.join(" "), depth)
}

/// `sudo` with `-i` or `-s` and no command starts a shell; with a command, `-i` and
/// `-s` hand its text to the shell, which reads it as a line.
fn sudo_starts_shell(args: &[&str], terminal_input: bool, depth: usize) -> bool {
    let parsed = parse(
        args,
        "ugCDhprtTUR",
        &[
            "user",
            "group",
            "close-from",
            "chdir",
            "host",
            "prompt",
            "role",
            "type",
            "command-timeout",
            "other-user",
            "chroot",
        ],
    );
    // Editing, listing, validating, the version and removing the credentials run no
    // command.
    let runs_nothing = ['e', 'l', 'v', 'V', 'K'].into_iter().any(|letter| parsed.has(letter))
        || ["edit", "list", "validate", "version", "remove-timestamp"]
            .into_iter()
            .any(|name| parsed.has_long(name));
    if runs_nothing {
        return false;
    }
    let shell =
        parsed.has('i') || parsed.has('s') || parsed.has_long("login") || parsed.has_long("shell");
    match parsed.operands.as_slice() {
        [] => shell,
        command if shell => line_starts_shell(&command.join(" "), depth),
        command => argv_starts_shell(command, terminal_input, depth),
    }
}

/// The options and operands of a program's arguments.
#[derive(Debug, Default)]
struct Parsed<'a> {
    /// Each short option by its letter, with its value if it takes one.
    short: Vec<(char, Option<&'a str>)>,
    /// Each long option by its name without `--`, with its value if it has one.
    long: Vec<(&'a str, Option<&'a str>)>,
    operands: Vec<&'a str>,
    /// True when `--` ended the options.
    ended: bool,
}

impl<'a> Parsed<'a> {
    fn has(&self, letter: char) -> bool {
        self.short.iter().any(|(c, _)| *c == letter)
    }

    fn has_long(&self, name: &str) -> bool {
        self.long.iter().any(|(long, _)| *long == name)
    }

    /// The value of the last short option `letter`.
    fn value(&self, letter: char) -> Option<&'a str> {
        self.short.iter().rev().find(|(c, _)| *c == letter).and_then(|(_, value)| *value)
    }

    /// The value of the last long option `name`.
    fn long_value(&self, name: &str) -> Option<&'a str> {
        self.long.iter().rev().find(|(long, _)| *long == name).and_then(|(_, value)| *value)
    }
}

/// Reads the options of `args` up to the first operand or `--`; the rest are operands.
/// The short options in `short` and the long ones in `long` take a value: attached
/// (`-uroot`, `--user=root`) or as the next word.
fn parse<'a>(args: &[&'a str], short: &str, long: &[&str]) -> Parsed<'a> {
    let mut parsed = Parsed::default();
    let mut words = args.iter().copied().peekable();
    while let Some(arg) = words.next_if(|arg| arg.starts_with('-') && *arg != "-") {
        if arg == "--" {
            parsed.ended = true;
            break;
        }
        if let Some(option) = arg.strip_prefix("--") {
            let (name, value) = match option.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None if long.contains(&option) => (option, words.next()),
                None => (option, None),
            };
            parsed.long.push((name, value));
            continue;
        }
        let letters = &arg[1..];
        for (at, letter) in letters.char_indices() {
            if !short.contains(letter) {
                parsed.short.push((letter, None));
                continue;
            }
            let attached = &letters[at + letter.len_utf8()..];
            let value = if attached.is_empty() { words.next() } else { Some(attached) };
            parsed.short.push((letter, value));
            break;
        }
    }
    parsed.operands = words.collect();
    parsed
}

/// [`parse`] for a program that also takes options after an operand, as GNU programs do.
fn parse_all<'a>(args: &[&'a str], short: &str, long: &[&str]) -> Parsed<'a> {
    let mut all = Parsed::default();
    let mut rest = args.to_vec();
    loop {
        let parsed = parse(&rest, short, long);
        all.short.extend(parsed.short);
        all.long.extend(parsed.long);
        match parsed.operands.split_first() {
            Some((&operand, after)) if !parsed.ended => {
                all.operands.push(operand);
                rest = after.to_vec();
            }
            _ => {
                all.operands.extend(parsed.operands.iter().copied());
                return all;
            }
        }
    }
}

#[cfg(test)]
mod tests;
