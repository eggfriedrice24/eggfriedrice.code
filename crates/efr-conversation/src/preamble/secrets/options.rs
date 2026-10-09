//! The value of an option that takes a password, for the programs known to take one
//! there: `mysql -psecret`, `sshpass -p secret`, `docker login -p secret`,
//! `curl -u user:secret`.
//!
//! The list names each program, because the same option means something else
//! elsewhere: `docker run -p` publishes a port, `mysql -p secret` asks for the password
//! and opens the database `secret`, and `psql --password` only asks. The assignment
//! form `--password=secret` is redacted for every program (`super::redact`).

use std::ops::Range;

use super::names_another_value;
use super::words::Word;

/// Where an option takes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Takes {
    /// Only in the same word: `-psecret`, `--password=secret`.
    Attached,
    /// Only in the next word: `-a secret`.
    Next,
    /// In the same word or the next one.
    Both,
}

/// Which part of the value is the secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// All of it.
    Whole,
    /// What follows the first `:`, as in `user:password`; nothing without one.
    AfterColon,
    /// What follows `pass:`, as in OpenSSL's `-passin pass:secret`; nothing in another
    /// form (`env:NAME`, `file:path`).
    AfterPass,
}

/// An option that takes a password.
#[derive(Debug)]
struct PasswordOption {
    name: &'static str,
    takes: Takes,
    part: Part,
}

const fn option(name: &'static str, takes: Takes) -> PasswordOption {
    PasswordOption { name, takes, part: Part::Whole }
}

/// Programs that take a password in an option.
#[derive(Debug)]
struct Program {
    names: &'static [&'static str],
    /// A word that must come after the program, such as `login` for `docker login`: one
    /// of the first [`SUBCOMMAND_WORDS`] words that are not options (`docker --config d
    /// login`, `helm registry login`). The password options count only after it.
    subcommand: Option<&'static str>,
    options: &'static [PasswordOption],
}

/// The MySQL and MariaDB clients. Their password must be attached: `-p` alone asks
/// for it, and the next word is something else.
const MYSQL: &[&str] = &[
    "mysql",
    "mysqldump",
    "mysqladmin",
    "mysqlimport",
    "mysqlshow",
    "mysqlcheck",
    "mysqlpump",
    "mysqlslap",
    "mysqlbinlog",
    "mysqlsh",
    "mariadb",
    "mariadb-dump",
    "mariadb-admin",
    "mariadb-import",
    "mariadb-show",
    "mariadb-check",
    "mariadb-slap",
    "mariadb-binlog",
];

const MONGO: &[&str] = &[
    "mongo",
    "mongosh",
    "mongodump",
    "mongorestore",
    "mongoexport",
    "mongoimport",
    "mongostat",
    "mongotop",
    "mongofiles",
];

const LDAP: &[&str] = &[
    "ldapsearch",
    "ldapmodify",
    "ldapadd",
    "ldapdelete",
    "ldapwhoami",
    "ldappasswd",
    "ldapcompare",
    "ldapmodrdn",
    "ldapexop",
];

/// How many of the first words after a program, options left out, can be its
/// subcommand.
const SUBCOMMAND_WORDS: usize = 2;

/// The container tools whose `login` takes a password.
const REGISTRY_LOGIN: &[&str] =
    &["docker", "podman", "nerdctl", "buildah", "skopeo", "oras", "helm"];

const PROGRAMS: &[Program] = &[
    Program {
        names: MYSQL,
        subcommand: None,
        options: &[option("-p", Takes::Attached), option("--password", Takes::Attached)],
    },
    Program { names: &["sshpass"], subcommand: None, options: &[option("-p", Takes::Both)] },
    Program {
        names: &["redis-cli", "valkey-cli"],
        subcommand: None,
        options: &[option("-a", Takes::Next), option("--pass", Takes::Both)],
    },
    Program {
        names: MONGO,
        subcommand: None,
        options: &[option("-p", Takes::Both), option("--password", Takes::Both)],
    },
    Program {
        names: REGISTRY_LOGIN,
        subcommand: Some("login"),
        options: &[option("-p", Takes::Both), option("--password", Takes::Both)],
    },
    Program { names: &["helm"], subcommand: None, options: &[option("--password", Takes::Both)] },
    Program {
        names: &["zip", "unzip", "zipcloak"],
        subcommand: None,
        options: &[option("-P", Takes::Both), option("--password", Takes::Both)],
    },
    Program {
        names: &["7z", "7za", "7zr", "7zz"],
        subcommand: None,
        options: &[option("-p", Takes::Attached)],
    },
    Program { names: &["sqlcmd"], subcommand: None, options: &[option("-P", Takes::Both)] },
    Program { names: LDAP, subcommand: None, options: &[option("-w", Takes::Both)] },
    Program {
        names: &["wget"],
        subcommand: None,
        options: &[
            option("--password", Takes::Both),
            option("--http-password", Takes::Both),
            option("--ftp-password", Takes::Both),
            option("--proxy-password", Takes::Both),
        ],
    },
    Program {
        names: &["curl"],
        subcommand: None,
        options: &[
            PasswordOption { name: "-u", takes: Takes::Both, part: Part::AfterColon },
            PasswordOption { name: "--user", takes: Takes::Both, part: Part::AfterColon },
            PasswordOption { name: "-U", takes: Takes::Both, part: Part::AfterColon },
            PasswordOption { name: "--proxy-user", takes: Takes::Both, part: Part::AfterColon },
        ],
    },
    Program {
        names: &["openssl"],
        subcommand: None,
        options: &[
            PasswordOption { name: "-passin", takes: Takes::Next, part: Part::AfterPass },
            PasswordOption { name: "-passout", takes: Takes::Next, part: Part::AfterPass },
            PasswordOption { name: "-pass", takes: Takes::Next, part: Part::AfterPass },
            option("-k", Takes::Next),
        ],
    },
    Program {
        names: &["gpg", "gpg2"],
        subcommand: None,
        options: &[option("--passphrase", Takes::Both)],
    },
    Program { names: &["svn"], subcommand: None, options: &[option("--password", Takes::Both)] },
    Program {
        names: &["kubectl", "oc"],
        subcommand: None,
        options: &[option("--password", Takes::Both), option("--token", Takes::Both)],
    },
];

/// A command that runs the command in its arguments.
#[derive(Debug)]
struct Wrapper {
    name: &'static str,
    /// Its options that take the next word as their value.
    valued: &'static [&'static str],
    /// How many words that are not options come before the command (`timeout 5 cmd`).
    operands: usize,
}

const WRAPPERS: &[Wrapper] = &[
    Wrapper {
        name: "sudo",
        valued: &[
            "-u",
            "-g",
            "-h",
            "-p",
            "-C",
            "-D",
            "-R",
            "-T",
            "-U",
            "-r",
            "-t",
            "--user",
            "--group",
            "--host",
            "--prompt",
            "--close-from",
            "--chdir",
            "--chroot",
            "--command-timeout",
            "--other-user",
            "--role",
            "--type",
        ],
        operands: 0,
    },
    Wrapper { name: "doas", valued: &["-u", "-C"], operands: 0 },
    Wrapper { name: "env", valued: &["-u", "--unset", "-C", "--chdir"], operands: 0 },
    Wrapper { name: "command", valued: &[], operands: 0 },
    Wrapper { name: "exec", valued: &["-a"], operands: 0 },
    Wrapper { name: "time", valued: &["-f", "--format", "-o", "--output"], operands: 0 },
    Wrapper { name: "nohup", valued: &[], operands: 0 },
    Wrapper { name: "nice", valued: &["-n", "--adjustment"], operands: 0 },
    Wrapper { name: "timeout", valued: &["-k", "--kill-after", "-s", "--signal"], operands: 1 },
];

/// The bytes of the line that hold the value of each password option in `commands`.
/// A value that names another value (`$PASSWORD`) stays, and so does a next word that
/// starts with `-`, which is the next option: the program then asks for the password.
pub(super) fn values(commands: &[Vec<Word>]) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    for command in commands {
        let Some((program, args)) = program(command) else {
            continue;
        };
        // NOTE: each option with the index of the first word where it counts.
        let options: Vec<(&PasswordOption, usize)> = PROGRAMS
            .iter()
            .filter(|known| known.names.contains(&program))
            .filter_map(|known| {
                let from = match known.subcommand {
                    None => 0,
                    Some(sub) => subcommand_at(args, sub)? + 1,
                };
                Some(known.options.iter().map(move |option| (option, from)))
            })
            .flatten()
            .collect();
        let mut index = 0;
        while index < args.len() {
            let word = &args[index];
            if word.text == "--" {
                break;
            }
            let counting = options.iter().filter(|(_, from)| index >= *from);
            for (option, _) in counting {
                if let Some(at) = attached(&word.text, option) {
                    found.extend(secret(word, at, option.part));
                    break;
                }
                if word.text == option.name && option.takes != Takes::Attached {
                    if let Some(next) = args.get(index + 1)
                        && !next.text.starts_with('-')
                    {
                        found.extend(secret(next, 0, option.part));
                        index += 1;
                    }
                    break;
                }
            }
            index += 1;
        }
    }
    found
}

/// The index in `args` of the word `sub` when it is one of the first
/// [`SUBCOMMAND_WORDS`] words that are not options.
fn subcommand_at(args: &[Word], sub: &str) -> Option<usize> {
    args.iter()
        .enumerate()
        .filter(|(_, arg)| !arg.text.starts_with('-'))
        .take(SUBCOMMAND_WORDS)
        .find(|(_, arg)| arg.text == sub)
        .map(|(index, _)| index)
}

/// Where the value of `option` starts in `word` when it is attached: right after a
/// short option (`-psecret`), after the `=` of a long one (`--password=secret`).
fn attached(word: &str, option: &PasswordOption) -> Option<usize> {
    if option.takes == Takes::Next {
        return None;
    }
    let rest = word.strip_prefix(option.name)?;
    let short = option.name.len() == 2;
    if short && !rest.is_empty() {
        Some(option.name.len())
    } else if !short && rest.starts_with('=') {
        Some(option.name.len() + 1)
    } else {
        None
    }
}

/// The bytes of the line that hold the secret part of the value that starts at `from`
/// in `word`.
fn secret(word: &Word, from: usize, part: Part) -> Option<Range<usize>> {
    let value = &word.text[from..];
    let skip = match part {
        Part::Whole => 0,
        Part::AfterColon => value.find(':')? + 1,
        Part::AfterPass => value.strip_prefix("pass:").map(|_| "pass:".len())?,
    };
    let secret = &value[skip..];
    if secret.is_empty() || names_another_value(secret) {
        return None;
    }
    word.line_range(from + skip)
}

/// The name of the program that `command` runs and its arguments, after the
/// assignments before it and the wrappers that run it (`sudo`, `env`, `timeout`).
fn program(command: &[Word]) -> Option<(&str, &[Word])> {
    let mut words = command;
    loop {
        let (first, rest) = words.split_first()?;
        if is_assignment(&first.text) {
            words = rest;
            continue;
        }
        let name = first.text.rsplit('/').next().unwrap_or(&first.text);
        let Some(wrapper) = WRAPPERS.iter().find(|wrapper| wrapper.name == name) else {
            return Some((name, rest));
        };
        words = after_wrapper(wrapper, rest);
    }
}

/// The words after the options and operands of `wrapper`.
fn after_wrapper<'w>(wrapper: &Wrapper, mut words: &'w [Word]) -> &'w [Word] {
    let mut operands = wrapper.operands;
    while let Some((first, rest)) = words.split_first() {
        if first.text == "--" {
            return rest;
        }
        if wrapper.valued.contains(&first.text.as_str()) {
            words = rest.get(1..).unwrap_or_default();
        } else if first.text.starts_with('-') || is_assignment(&first.text) {
            words = rest;
        } else if operands > 0 {
            operands -= 1;
            words = rest;
        } else {
            break;
        }
    }
    words
}

/// True for `NAME=value`, an assignment before a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}
