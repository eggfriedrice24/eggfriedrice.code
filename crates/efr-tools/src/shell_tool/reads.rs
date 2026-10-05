//! Which words of a simple command name paths it reads, and whether it reads them
//! alone or with everything below, or reads the working directory without naming it.
//!
//! Every program's operands count as paths, and so does an option's value that looks
//! like one, so the permission engine sees `cat ~/.ssh/id_ed25519` or
//! `date -f ~/.ssh/id_ed25519` as a read of the key. A few programs whose arguments are
//! never files (`echo`, `printf`, `basename`) declare nothing, so naming a secret path
//! in them is not refused. The programs that search or list recursively (`rg`, `grep
//! -r`, `find`, `du`, `tree`, `ls -R`, `diff`) read everything below their paths, and
//! with no path they read the working directory; their options are parsed by table,
//! and an option the table does not know makes the reading fail closed: every operand
//! is a path and the working directory is read too.
//!
//! The writer programs (`rm`, `mkdir`, `cp`, `mv` and more, see [`writes`](super::writes))
//! write their operands instead, and so do `git rm`, `git mv` and `git worktree add`.

use super::words::{Word, is_assignment};
use super::writes;

/// How much of a path a command reads, the lesser first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Depth {
    /// The path itself, or the listing of a directory.
    One,
    /// The path and everything below it.
    Tree,
}

/// A path a command names: a word, or the value of an option such as `--file=x`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Named {
    /// The path as written.
    pub(super) text: String,
    /// True when it starts with `~` meaning the home directory.
    pub(super) tilde: bool,
    /// Where the first pattern character is, in bytes of `text`.
    pub(super) pattern: Option<usize>,
    /// How much of it the command reads.
    pub(super) depth: Depth,
}

/// What one simple command reads, and what it writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Reads {
    /// The paths it names and reads.
    pub(super) named: Vec<Named>,
    /// The paths it names and writes: creates, changes, moves or deletes.
    pub(super) writes: Vec<Named>,
    /// True when it may create or move a symbolic link at a path it writes, so a later
    /// command of the line may reach anything through that path.
    pub(super) links: bool,
    /// How much of the working directory it reads without naming it, if any.
    pub(super) cwd: Option<Depth>,
}

/// Programs whose arguments are never paths they open.
const NO_PATHS: &[&str] = &[
    "echo", "printf", "print", "basename", "dirname", "which", "type", "whence", "true", "false",
    "sleep", "seq", "whoami", "groups", "id", "uname", "nproc", "uptime",
];

/// Programs that run the program named among their arguments.
const WRAPPERS: &[&str] = &[
    "sudo",
    "sudoedit",
    "doas",
    "pkexec",
    "run0",
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
];

/// The options of a program that searches or lists, as far as the reading needs them.
struct Reader {
    /// The program names it covers.
    programs: &'static [&'static str],
    /// How much of each path operand it reads, unless `recursive` turns it on.
    depth: Depth,
    /// Short options that make it read everything below its paths.
    recursive_short: &'static str,
    /// Long options that make it read everything below its paths.
    recursive_long: &'static [&'static str],
    /// True when, with no path operand, it reads the working directory: always for a
    /// tree reader, and for `grep` only when it is recursive.
    cwd: bool,
    /// True when its first operand is a pattern, unless `-e` or the like gives one.
    pattern_first: bool,
    /// Options that give the pattern, or say there is none, so every operand is a path.
    pattern_short: &'static str,
    pattern_long: &'static [&'static str],
    /// Short options without and with a value.
    flags: &'static str,
    values: &'static str,
    /// Long options without and with a value, without the leading `--`.
    long_flags: &'static [&'static str],
    long_values: &'static [&'static str],
    /// The options whose value is a file the program reads, short and long.
    file_short: &'static str,
    file_long: &'static [&'static str],
}

const READERS: &[Reader] = &[
    Reader {
        programs: &["ls"],
        depth: Depth::One,
        recursive_short: "R",
        recursive_long: &["recursive"],
        cwd: true,
        pattern_first: false,
        pattern_short: "",
        pattern_long: &[],
        flags: "aAbBcCdDfFgGhHiklLmnNopqQrRsStuUvxXZ1",
        values: "ITw",
        long_flags: &[
            "all",
            "almost-all",
            "author",
            "escape",
            "directory",
            "classify",
            "file-type",
            "human-readable",
            "si",
            "inode",
            "dereference",
            "numeric-uid-gid",
            "literal",
            "hide-control-chars",
            "show-control-chars",
            "quote-name",
            "recursive",
            "reverse",
            "size",
            "context",
            "zero",
            "full-time",
            "group-directories-first",
            "no-group",
            "color",
            "hyperlink",
        ],
        long_values: &[
            "block-size",
            "format",
            "hide",
            "ignore",
            "indicator-style",
            "quoting-style",
            "sort",
            "tabsize",
            "time",
            "time-style",
            "width",
        ],
        file_short: "",
        file_long: &[],
    },
    Reader {
        programs: &["rg"],
        depth: Depth::Tree,
        recursive_short: "",
        recursive_long: &[],
        cwd: true,
        pattern_first: true,
        pattern_short: "ef",
        pattern_long: &["regexp", "file", "files", "type-list"],
        flags: "abcFHhiIlLnNopPqsSuUvVwxz0.",
        values: "ABCdEefgjMmrtT",
        long_flags: &[
            "hidden",
            "no-ignore",
            "no-ignore-vcs",
            "no-ignore-parent",
            "no-ignore-dot",
            "no-ignore-global",
            "no-ignore-exclude",
            "ignore-case",
            "smart-case",
            "case-sensitive",
            "line-number",
            "no-line-number",
            "heading",
            "no-heading",
            "fixed-strings",
            "word-regexp",
            "line-regexp",
            "count",
            "count-matches",
            "files-with-matches",
            "files-without-match",
            "follow",
            "json",
            "vimgrep",
            "invert-match",
            "only-matching",
            "no-filename",
            "with-filename",
            "multiline",
            "multiline-dotall",
            "unrestricted",
            "stats",
            "trim",
            "pretty",
            "no-messages",
            "null",
            "column",
            "text",
            "binary",
            "byte-offset",
            "quiet",
            "passthru",
            "search-zip",
            "sort-files",
            "no-config",
            "files",
            "type-list",
            "crlf",
            "pcre2",
            "one-file-system",
            "no-require-git",
            "glob-case-insensitive",
            "include-zero",
            "color",
        ],
        long_values: &[
            "regexp",
            "file",
            "glob",
            "iglob",
            "type",
            "type-not",
            "type-add",
            "type-clear",
            "max-count",
            "after-context",
            "before-context",
            "context",
            "max-columns",
            "threads",
            "encoding",
            "replace",
            "max-depth",
            "max-filesize",
            "sort",
            "sortr",
            "colors",
            "ignore-file",
            "pre",
            "pre-glob",
            "engine",
            "context-separator",
            "path-separator",
        ],
        file_short: "f",
        file_long: &["file", "ignore-file"],
    },
    Reader {
        programs: &["grep", "egrep", "fgrep"],
        depth: Depth::One,
        recursive_short: "rR",
        recursive_long: &["recursive", "dereference-recursive"],
        cwd: true,
        pattern_first: true,
        pattern_short: "ef",
        pattern_long: &["regexp", "file"],
        flags: "abcEFGhHiIlLnoPqrRsTuUvVwxyZz0123456789",
        values: "efmABCdDX",
        long_flags: &[
            "extended-regexp",
            "fixed-strings",
            "basic-regexp",
            "perl-regexp",
            "ignore-case",
            "no-ignore-case",
            "word-regexp",
            "line-regexp",
            "null-data",
            "no-messages",
            "invert-match",
            "byte-offset",
            "line-number",
            "with-filename",
            "no-filename",
            "files-without-match",
            "files-with-matches",
            "count",
            "only-matching",
            "quiet",
            "silent",
            "recursive",
            "dereference-recursive",
            "text",
            "initial-tab",
            "null",
            "color",
            "colour",
            "line-buffered",
        ],
        long_values: &[
            "regexp",
            "file",
            "max-count",
            "after-context",
            "before-context",
            "context",
            "directories",
            "devices",
            "include",
            "exclude",
            "exclude-dir",
            "exclude-from",
            "label",
            "binary-files",
            "group-separator",
        ],
        file_short: "f",
        file_long: &["file", "exclude-from"],
    },
    Reader {
        programs: &["du"],
        depth: Depth::Tree,
        recursive_short: "",
        recursive_long: &[],
        cwd: true,
        pattern_first: false,
        pattern_short: "",
        pattern_long: &[],
        flags: "abcDhHkLlmPsSx0",
        values: "BdtX",
        long_flags: &[
            "all",
            "apparent-size",
            "bytes",
            "total",
            "dereference-args",
            "human-readable",
            "inodes",
            "count-links",
            "dereference",
            "no-dereference",
            "null",
            "separate-dirs",
            "si",
            "summarize",
            "one-file-system",
            "time",
        ],
        long_values: &[
            "block-size",
            "max-depth",
            "threshold",
            "exclude-from",
            "files0-from",
            "time-style",
            "exclude",
        ],
        file_short: "X",
        file_long: &["exclude-from", "files0-from"],
    },
    Reader {
        programs: &["tree"],
        depth: Depth::Tree,
        recursive_short: "",
        recursive_long: &[],
        cwd: true,
        pattern_first: false,
        pattern_short: "",
        pattern_long: &[],
        flags: "adlfxACDFgihnNpqQrstSuUvJXRc",
        values: "HTLPIo",
        long_flags: &[
            "noreport",
            "dirsfirst",
            "filesfirst",
            "inodes",
            "device",
            "du",
            "si",
            "prune",
            "gitignore",
            "matchdirs",
            "ignore-case",
            "info",
            "fromfile",
            "metafirst",
            "nolinks",
        ],
        long_values: &["charset", "filelimit", "timefmt", "sort", "infofile", "gitfile"],
        file_short: "",
        file_long: &["infofile", "gitfile"],
    },
];

/// What the simple command `words` (program first, assignments skipped) reads.
pub(super) fn reads(words: &[Word]) -> Reads {
    let Some((program, args)) = words.split_first() else {
        return Reads::default();
    };
    let name = program.text.rsplit('/').next().unwrap_or(&program.text);
    if NO_PATHS.contains(&name) {
        return Reads::default();
    }
    if WRAPPERS.contains(&name) {
        return wrapped(args);
    }
    if let Some(reads) = writes::writer(name, args) {
        return reads;
    }
    if name == "git"
        && let Some(reads) = git(args)
    {
        return reads;
    }
    if name == "find" {
        return find(args);
    }
    if name == "diff" {
        let mut reads = generic(args);
        for named in &mut reads.named {
            named.depth = Depth::Tree;
        }
        return reads;
    }
    match READERS.iter().find(|reader| reader.programs.contains(&name)) {
        Some(reader) => reader.read(args),
        None => generic(args),
    }
}

/// `git rm`, `git mv` and `git worktree add`, which delete, move or create paths in the
/// work tree: every word after the subcommand that `generic` would read, written
/// instead. `None` for any other git command, whose words are read.
fn git(args: &[Word]) -> Option<Reads> {
    let words: Vec<&str> = args.iter().map(|word| word.text.as_str()).collect();
    let after = match words.as_slice() {
        ["rm" | "mv", ..] => &args[1..],
        ["worktree", "add", ..] => &args[2..],
        _ => return None,
    };
    let Reads { named, .. } = generic(after);
    Some(Reads { writes: named, links: true, ..Reads::default() })
}

/// Every operand, and every option value that looks like a path, read alone.
pub(super) fn generic(args: &[Word]) -> Reads {
    let mut named = Vec::new();
    let mut after_options = false;
    for word in args {
        let text = word.text.as_str();
        if !after_options && text == "--" {
            after_options = true;
            continue;
        }
        if text == "-" || word.expansion {
            continue;
        }
        if after_options || !text.starts_with('-') {
            named.push(from_word(word, Depth::One));
            // NOTE: `HEAD:path` names a path in a repository, and `host:path` one on a
            // host; the part after the colon counts as a path too.
            if let Some((_, after)) = text.split_once(':')
                && !after.is_empty()
                && !text.contains("://")
            {
                named.push(named_value(after));
            }
        } else if let Some(value) = option_value(word) {
            named.push(value);
        }
    }
    Reads { named, ..Reads::default() }
}

/// A wrapper such as `sudo` or `timeout`: every word as a path, and the program it
/// runs, found as the first word that is not an option, an assignment or a number.
fn wrapped(args: &[Word]) -> Reads {
    let mut reads = generic(args);
    let inner = args.iter().position(|word| {
        let text = word.text.as_str();
        !text.starts_with('-')
            && !is_assignment(text)
            && !text.chars().next().is_some_and(|c| c.is_ascii_digit())
    });
    if let Some(at) = inner {
        let inner = self::reads(&args[at..]);
        reads.named.extend(inner.named);
        reads.writes.extend(inner.writes);
        reads.links |= inner.links;
        reads.cwd = reads.cwd.max(inner.cwd);
    }
    reads
}

/// `find`: the start points before the expression, read with everything below them,
/// or the working directory when there are none.
fn find(args: &[Word]) -> Reads {
    let mut named = Vec::new();
    let mut words = args.iter().peekable();
    // Options of find itself, before the start points.
    while let Some(word) = words.peek() {
        match word.text.as_str() {
            "-H" | "-L" | "-P" => {
                words.next();
            }
            "-D" => {
                words.next();
                words.next();
            }
            text if text.starts_with("-O") => {
                words.next();
            }
            _ => break,
        }
    }
    for word in words {
        let text = word.text.as_str();
        if text.starts_with('-') || matches!(text, "(" | "!" | ",") {
            break;
        }
        if !word.expansion {
            named.push(from_word(word, Depth::Tree));
        }
    }
    let cwd = named.is_empty().then_some(Depth::Tree);
    Reads { named, cwd, ..Reads::default() }
}

impl Reader {
    /// Whether the long option `name`, or the one option it abbreviates, takes a value;
    /// `None` when the table does not know it or it is ambiguous.
    fn long_option(&self, name: &str) -> Option<bool> {
        if self.long_values.contains(&name) {
            return Some(true);
        }
        if self.long_flags.contains(&name) {
            return Some(false);
        }
        let abbreviates = |full: &&&str| !name.is_empty() && full.starts_with(name);
        let values = self.long_values.iter().filter(abbreviates).count();
        let flags = self.long_flags.iter().filter(abbreviates).count();
        match (flags, values) {
            (0, 0) => None,
            (_, 0) => Some(false),
            (0, _) => Some(true),
            _ => None,
        }
    }

    fn read(&self, args: &[Word]) -> Reads {
        let mut operands: Vec<&Word> = Vec::new();
        let mut values: Vec<Named> = Vec::new();
        let mut certain = true;
        let mut recursive = false;
        let mut pattern_given = false;
        let mut words = args.iter();
        let mut after_options = false;
        while let Some(word) = words.next() {
            let text = word.text.as_str();
            if after_options || text == "-" || !text.starts_with('-') {
                operands.push(word);
                continue;
            }
            if text == "--" {
                after_options = true;
                continue;
            }
            if let Some(long) = text.strip_prefix("--") {
                let (name, inline) = match long.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (long, None),
                };
                // GNU programs take any unambiguous abbreviation, so a short name that
                // could stand for a recursive option counts as one.
                let abbreviates = |full: &&str| full.starts_with(name) && !name.is_empty();
                if self.recursive_long.iter().any(abbreviates)
                    || (name == "directories" && inline == Some("recurse"))
                {
                    recursive = true;
                }
                if self.pattern_long.contains(&name) {
                    pattern_given = true;
                }
                let takes_value = match self.long_option(name) {
                    Some(takes_value) => takes_value,
                    None => {
                        certain = false;
                        false
                    }
                };
                let value = match inline {
                    Some(value) => Some(value.to_owned()),
                    None if takes_value => words.next().map(|next| next.text.clone()),
                    None => None,
                };
                if name == "directories" && value.as_deref() == Some("recurse") {
                    recursive = true;
                }
                if let Some(value) = value.filter(|_| self.file_long.contains(&name)) {
                    values.push(named_value(&value));
                }
                continue;
            }
            for (at, letter) in text[1..].char_indices() {
                if self.recursive_short.contains(letter) {
                    recursive = true;
                }
                if self.pattern_short.contains(letter) {
                    pattern_given = true;
                }
                if self.values.contains(letter) {
                    let rest = &text[1 + at + letter.len_utf8()..];
                    let value = if rest.is_empty() {
                        words.next().map(|next| next.text.clone())
                    } else {
                        Some(rest.to_owned())
                    };
                    if letter == 'd' && value.as_deref() == Some("recurse") {
                        recursive = true;
                    }
                    if let Some(value) = value.filter(|_| self.file_short.contains(letter)) {
                        values.push(named_value(&value));
                    }
                    break;
                }
                if !self.flags.contains(letter) {
                    certain = false;
                }
            }
        }
        let depth = if recursive { Depth::Tree } else { self.depth };
        let mut paths: Vec<&Word> = operands;
        if certain && self.pattern_first && !pattern_given && !paths.is_empty() {
            paths.remove(0);
        }
        let mut named: Vec<Named> = paths
            .iter()
            .filter(|word| word.text != "-" && !word.expansion)
            .map(|word| from_word(word, depth))
            .collect();
        named.extend(values);
        let reads_cwd = self.cwd && (depth == Depth::Tree || !self.pattern_first);
        let cwd = (reads_cwd && (!certain || paths.is_empty())).then_some(depth);
        Reads { named, cwd, ..Reads::default() }
    }
}

/// The path in an option such as `--file=x` or `-f/x`: the text after `=`, or from the
/// first `/` on.
pub(super) fn option_value(word: &Word) -> Option<Named> {
    let text = word.text.as_str();
    let value = match text.split_once('=') {
        Some((_, value)) => value,
        None => &text[text.find('/')?..],
    };
    (!value.is_empty()).then(|| named_value(value))
}

/// A path given as an option's value. zsh replaces a `~` there only with
/// MAGIC_EQUAL_SUBST, which the user's startup files may set, so `~/` counts as home.
pub(super) fn named_value(value: &str) -> Named {
    let pattern = value.find(['*', '?', '[']);
    Named {
        text: value.to_owned(),
        tilde: value == "~" || value.starts_with("~/"),
        pattern,
        depth: Depth::One,
    }
}

pub(super) fn from_word(word: &Word, depth: Depth) -> Named {
    Named { text: word.text.clone(), tilde: word.tilde, pattern: word.pattern, depth }
}

#[cfg(test)]
mod tests;
