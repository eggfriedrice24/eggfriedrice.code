//! The global options of `systemctl`, `cargo`, `docker` and `kubectl` before their
//! subcommand, for the rules that ask or deny (efr's v0.0.4 live items plan, OQL4).
//!
//! `systemctl --user restart x` runs `restart`, but its second word is `restart` only
//! after the option. [`readings`] finds where the subcommand may stand by the table of
//! the program ([`table`]): an option of the table is skipped with its value, and an
//! option that the table does not know is read both as a flag and with a value in the
//! next word, so a rule that asks or denies matches rather than misses. A rule that
//! allows reads none of this, because an option such as `docker -H host` or
//! `kubectl --context prod` can point the command at another machine.
//!
//! NOTE: the tables hold only the options whose form is sure: systemd 258's
//! `systemctl`, cargo 1.91, the docker CLI 28 and kubectl 1.34, checked on 2026-10-24.
//! An option that is missing is read both ways, which is safe; an option with the wrong
//! form is not, so check a table against the program's `--help` before an option goes
//! in.

use super::git::GitDirs;

/// How an option of a [`Table`] takes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// No value.
    Flag,
    /// A value in the same word (`--host=x`, `-Hx`) or else in the next word.
    Value,
    /// A value that can set an alias for a subcommand, such as cargo's
    /// `--config alias.p=publish`, so efr cannot read the subcommand: a rule that asks
    /// or denies for the program then matches. `inline` is true when a value with `=`
    /// is one setting that efr can read, as cargo's `--config` takes it.
    Config { inline: bool },
}

/// One global option: the name with its dashes, such as `-H` or `--host`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlobalOption {
    pub(crate) name: &'static str,
    pub(crate) kind: Kind,
}

const fn flag(name: &'static str) -> GlobalOption {
    GlobalOption { name, kind: Kind::Flag }
}

const fn value(name: &'static str) -> GlobalOption {
    GlobalOption { name, kind: Kind::Value }
}

/// The global options of one program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Table {
    /// The program, as a rule names it.
    pub(crate) program: &'static str,
    /// Its options.
    pub(crate) options: &'static [GlobalOption],
    /// True when a word that starts with `+` is a flag too: rustup's `cargo +nightly`.
    pub(crate) plus: bool,
    /// The option whose value is the dir that the program works in, such as cargo's
    /// `-C`. A rule with `under` counts that dir too.
    pub(crate) dir: Option<&'static str>,
}

/// `systemctl` (systemd 258). It reads options in any place, but only those before
/// the verb change which word is the verb.
const SYSTEMCTL: Table = Table {
    program: "systemctl",
    options: &[
        value("-t"),
        value("--type"),
        value("--state"),
        value("-p"),
        value("--property"),
        value("-P"),
        value("-H"),
        value("--host"),
        value("-M"),
        value("--machine"),
        value("--job-mode"),
        value("-s"),
        value("--signal"),
        value("--kill-whom"),
        value("--root"),
        value("-n"),
        value("--lines"),
        value("-o"),
        value("--output"),
        value("--preset-mode"),
        value("--message"),
        flag("-h"),
        flag("--help"),
        flag("--version"),
        flag("--system"),
        flag("--user"),
        flag("--global"),
        flag("--runtime"),
        flag("-a"),
        flag("--all"),
        flag("-l"),
        flag("--full"),
        flag("-r"),
        flag("--recursive"),
        flag("--reverse"),
        flag("--after"),
        flag("--before"),
        flag("--with-dependencies"),
        flag("-T"),
        flag("--show-transaction"),
        flag("--show-types"),
        flag("--value"),
        flag("-i"),
        flag("--now"),
        flag("--dry-run"),
        flag("-q"),
        flag("--quiet"),
        flag("--no-warn"),
        flag("--wait"),
        flag("--no-block"),
        flag("--no-wall"),
        flag("--no-reload"),
        flag("--no-legend"),
        flag("--no-pager"),
        flag("--no-ask-password"),
        flag("-f"),
        flag("--force"),
        flag("--failed"),
        flag("--firmware-setup"),
        flag("--plain"),
        flag("--read-only"),
        flag("--mkdir"),
        flag("--marked"),
        flag("--stdin"),
    ],
    plus: false,
    dir: None,
};

/// `cargo` (1.91), behind rustup's `+toolchain`.
const CARGO: Table = Table {
    program: "cargo",
    options: &[
        value("--color"),
        value("-C"),
        value("-Z"),
        value("--explain"),
        GlobalOption { name: "--config", kind: Kind::Config { inline: true } },
        flag("-V"),
        flag("--version"),
        flag("--list"),
        flag("-v"),
        flag("--verbose"),
        flag("-q"),
        flag("--quiet"),
        flag("--locked"),
        flag("--offline"),
        flag("--frozen"),
        flag("-h"),
        flag("--help"),
    ],
    plus: true,
    dir: Some("-C"),
};

/// The docker CLI (28). Its global options must come before the subcommand.
const DOCKER: Table = Table {
    program: "docker",
    options: &[
        value("--config"),
        value("-c"),
        value("--context"),
        value("-H"),
        value("--host"),
        value("-l"),
        value("--log-level"),
        value("--tlscacert"),
        value("--tlscert"),
        value("--tlskey"),
        flag("-D"),
        flag("--debug"),
        flag("--tls"),
        flag("--tlsverify"),
        flag("-v"),
        flag("--version"),
        flag("-h"),
        flag("--help"),
    ],
    plus: false,
    dir: None,
};

/// `kubectl` (1.34). It reads options in any place, but only those before the
/// subcommand change which word is the subcommand. `-v` takes a log level.
const KUBECTL: Table = Table {
    program: "kubectl",
    options: &[
        value("-n"),
        value("--namespace"),
        value("--context"),
        value("--kubeconfig"),
        value("--cluster"),
        value("--user"),
        value("-s"),
        value("--server"),
        value("--token"),
        value("--as"),
        value("--as-group"),
        value("--as-uid"),
        value("--certificate-authority"),
        value("--client-certificate"),
        value("--client-key"),
        value("--request-timeout"),
        value("--tls-server-name"),
        value("--cache-dir"),
        value("--profile"),
        value("--profile-output"),
        value("--username"),
        value("--password"),
        value("-v"),
        value("--v"),
        value("--vmodule"),
        value("--log-flush-frequency"),
        // NOTE: a kuberc file can define aliases of subcommands.
        GlobalOption { name: "--kuberc", kind: Kind::Config { inline: false } },
        flag("--insecure-skip-tls-verify"),
        flag("--match-server-version"),
        flag("--disable-compression"),
        flag("--warnings-as-errors"),
        flag("-h"),
        flag("--help"),
    ],
    plus: false,
    dir: None,
};

/// Every table.
pub(crate) const TABLES: &[Table] = &[SYSTEMCTL, CARGO, DOCKER, KUBECTL];

/// The table of `program`, as a rule names it, or `None` for a program without one.
pub(crate) fn table(program: &str) -> Option<&'static Table> {
    TABLES.iter().find(|table| table.program == program)
}

impl Table {
    fn option(&self, name: &str) -> Option<&'static GlobalOption> {
        self.options.iter().find(|option| option.name == name)
    }

    /// The short option `-letter`.
    fn short(&self, letter: char) -> Option<&'static GlobalOption> {
        self.options.iter().find(|option| {
            let mut chars = option.name.chars();
            chars.next() == Some('-') && chars.next() == Some(letter) && chars.next().is_none()
        })
    }

    /// How the option word `word` reads, with `next`, the word after it.
    fn step<'a>(&self, word: &'a str, next: Option<&'a str>) -> Step<'a> {
        if word == "--" || (self.plus && word.starts_with('+')) {
            return Step::Known { option: None, value: None, width: 1 };
        }
        if let Some(long) = word.strip_prefix("--") {
            return match long.split_once('=') {
                // NOTE: every parser reads `--name=value` as one word.
                Some((name, value)) => Step::Known {
                    option: self.option(&word[..name.len() + 2]),
                    value: Some(value),
                    width: 1,
                },
                None => match self.option(word) {
                    Some(option) if option.kind == Kind::Flag => {
                        Step::Known { option: Some(option), value: None, width: 1 }
                    }
                    Some(option) => Step::Known { option: Some(option), value: next, width: 2 },
                    None => Step::Unknown,
                },
            };
        }
        // NOTE: short options in one word, as in `-qH host` or `-n5`: a letter with a
        // value takes the rest of the word, or else the next word.
        for (at, letter) in word.char_indices().skip(1) {
            match self.short(letter) {
                Some(option) if option.kind == Kind::Flag => {}
                Some(option) => {
                    let rest = &word[at + letter.len_utf8()..];
                    return if rest.is_empty() {
                        Step::Known { option: Some(option), value: next, width: 2 }
                    } else {
                        let rest = rest.strip_prefix('=').unwrap_or(rest);
                        Step::Known { option: Some(option), value: Some(rest), width: 1 }
                    };
                }
                None => return Step::Unknown,
            }
        }
        Step::Known { option: None, value: None, width: 1 }
    }

    /// True when `word` stands where an option of the program may.
    fn is_option(&self, word: &str) -> bool {
        (word.starts_with('-') && word != "-") || (self.plus && word.starts_with('+'))
    }
}

/// How one option word reads.
enum Step<'a> {
    /// An option whose form is known: a flag of the table (`option` may be `None` for
    /// `--`, a `+toolchain` or a `--name=value` that the table does not know), with
    /// its value and the number of words it takes.
    Known { option: Option<&'static GlobalOption>, value: Option<&'a str>, width: usize },
    /// An option that the table does not know, which may take the next word.
    Unknown,
}

/// How many options that the table does not know [`readings`] reads both ways before
/// it counts every later word that does not start with `-` as a possible subcommand:
/// 4 options, 16 readings.
const MAX_UNKNOWN: usize = 4;

/// The most places of the subcommand that [`readings`] keeps; a command with more
/// fails closed, so a long line of unknown options costs no more than this.
const MAX_SUBCOMMANDS: usize = 64;

/// The readings of a command of a program with a [`Table`], from [`readings`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Readings {
    /// Each place among the arguments where the subcommand may stand, once.
    pub(crate) subcommands: Vec<usize>,
    /// True when an option may set an alias ([`Kind::Config`]), or when the subcommand
    /// may stand in more than [`MAX_SUBCOMMANDS`] places. A rule that asks or denies
    /// for the program then matches.
    pub(crate) unreadable: bool,
    /// The dirs of the table's dir option in each reading, as a chain of `-C` words.
    pub(crate) dirs: Vec<GitDirs>,
}

impl Readings {
    fn add(&mut self, at: usize, dirs: &GitDirs) {
        if !self.subcommands.contains(&at) {
            if self.subcommands.len() == MAX_SUBCOMMANDS {
                self.unreadable = true;
                return;
            }
            self.subcommands.push(at);
        }
        if !self.dirs.contains(dirs) {
            self.dirs.push(dirs.clone());
        }
    }
}

/// One reading of the options, while [`readings`] walks them.
#[derive(Debug, Clone, Default)]
struct Branch {
    at: usize,
    unknown: usize,
    dirs: GitDirs,
}

/// The places where the subcommand of `table`'s program with `args` (the words after
/// the program) may stand, for a rule that asks or denies: each option of the table is
/// skipped with its value; an option that the table does not know is read as a flag
/// and as an option with a value in the next word; after [`MAX_UNKNOWN`] of them,
/// every later word that does not start with `-` counts. So such a rule fails towards
/// asking, also for a program that is newer than its table.
pub(crate) fn readings(table: &Table, args: &[String]) -> Readings {
    let mut out = Readings::default();
    let mut stack = vec![Branch::default()];
    while let Some(mut branch) = stack.pop() {
        let Some(word) = args.get(branch.at) else { continue };
        if !table.is_option(word) {
            out.add(branch.at, &branch.dirs);
            continue;
        }
        match table.step(word, args.get(branch.at + 1).map(String::as_str)) {
            Step::Known { option, value, width } => {
                if width == 2 && value.is_none() {
                    // NOTE: a value at the end of the line: no subcommand follows.
                    continue;
                }
                match option.map(|option| (option, option.kind)) {
                    Some((_, Kind::Config { inline })) => {
                        out.unreadable |= !inline || value.is_none_or(may_set_alias);
                    }
                    Some((option, _)) if table.dir == Some(option.name) => {
                        branch.dirs.chain.extend(value.map(str::to_owned));
                    }
                    _ => {}
                }
                branch.at += width;
                stack.push(branch);
            }
            Step::Unknown if branch.unknown == MAX_UNKNOWN => {
                for at in (branch.at + 1..args.len()).filter(|at| !table.is_option(&args[*at])) {
                    out.add(at, &branch.dirs);
                }
            }
            Step::Unknown => {
                branch.unknown += 1;
                let mut valued = branch.clone();
                valued.at += 2;
                branch.at += 1;
                stack.push(valued);
                stack.push(branch);
            }
        }
    }
    out
}

/// True when the inline config value `value`, such as cargo's `build.jobs=4`, may set
/// an alias: a value with no `=` names a file, and a key that names `alias` sets one.
fn may_set_alias(value: &str) -> bool {
    value.split_once('=').is_none_or(|(key, _)| key.contains("alias"))
}

#[cfg(test)]
mod tests;
