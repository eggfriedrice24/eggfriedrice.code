//! git's own options before its subcommand, and the views of a git command that the
//! rules read (efr's v0.0.4 live items plan, DL21 to DL26).
//!
//! `git -C ~/p/app push` runs `push`, but its third word is `push` only after the
//! options of git itself. [`exact`] finds the subcommand by the one table of those
//! options ([`OPTIONS`]), and every reader of a git line in this crate takes the
//! positions from it. [`views`] gives the readings of a line that a rule compares with
//! its `args`: for a rule that asks or denies, every global option is skipped, an option
//! that the table does not know is read both ways, and an alias that the line sets is
//! read as its command; for a rule that allows, only the plain options are skipped.
//!
//! NOTE: the table is git 2.56.0's, checked on 2026-10-21 with `git <option> version`.
//! A new git release can add a global option; check the table against git(1) then. A
//! rule that asks or denies stays safe with an option that the table does not know,
//! but the exits read only the table.

use std::path::{Path, PathBuf};

use crate::path_class::normalize;

/// How a global option of git takes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    /// The value is the next word: `-C <dir>`.
    Next,
    /// The value follows `=` in the same word: `--git-dir=<dir>`.
    Joined,
    /// No value: `--no-pager`.
    Flag,
}

/// What a global option of git does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// It changes neither the repository, nor the config, nor the programs that git
    /// runs, so a rule that allows skips it (DL23).
    Plain,
    /// It picks the repository or the work tree: `-C`, `--git-dir`, `--work-tree`,
    /// `--bare`, `--namespace`.
    Place,
    /// It changes the config, the programs that git runs or what git reads: `-c`,
    /// `--config-env`, `--exec-path=`, `--attr-source`, `--shallow-file`,
    /// `--list-cmds=`, the pager of `-p` and `--paginate`, which the config names, and
    /// `--no-replace-objects`.
    Steer,
    /// It prints and exits, such as `--version`. The parse reads it as a flag and goes
    /// on, so a rule that asks may also match `git --help push`, which is harmless.
    Print,
}

/// One global option of git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GitOption {
    /// The option, without `=` for the joined form.
    pub(crate) name: &'static str,
    /// How it takes its value.
    pub(crate) form: Form,
    /// What it does.
    pub(crate) class: Class,
}

const fn option(name: &'static str, form: Form, class: Class) -> GitOption {
    GitOption { name, form, class }
}

/// The global options of git 2.56.0. git refuses `-C<dir>`, `-c<key>=<value>`,
/// `--shallow-file=<file>`, `--list-cmds <x>`, an abbreviation such as `--no-pag`,
/// `--super-prefix` and `--` before the subcommand, so none of them is here.
pub(crate) const OPTIONS: &[GitOption] = &[
    option("-C", Form::Next, Class::Place),
    option("-c", Form::Next, Class::Steer),
    option("--git-dir", Form::Next, Class::Place),
    option("--work-tree", Form::Next, Class::Place),
    option("--namespace", Form::Next, Class::Place),
    option("--config-env", Form::Next, Class::Steer),
    option("--attr-source", Form::Next, Class::Steer),
    option("--shallow-file", Form::Next, Class::Steer),
    option("--git-dir", Form::Joined, Class::Place),
    option("--work-tree", Form::Joined, Class::Place),
    option("--namespace", Form::Joined, Class::Place),
    option("--config-env", Form::Joined, Class::Steer),
    option("--attr-source", Form::Joined, Class::Steer),
    option("--exec-path", Form::Joined, Class::Steer),
    option("--list-cmds", Form::Joined, Class::Steer),
    option("-p", Form::Flag, Class::Steer),
    option("--paginate", Form::Flag, Class::Steer),
    option("-P", Form::Flag, Class::Plain),
    option("--no-pager", Form::Flag, Class::Plain),
    option("--bare", Form::Flag, Class::Place),
    option("--no-replace-objects", Form::Flag, Class::Steer),
    option("--no-lazy-fetch", Form::Flag, Class::Plain),
    option("--literal-pathspecs", Form::Flag, Class::Plain),
    option("--no-literal-pathspecs", Form::Flag, Class::Plain),
    option("--glob-pathspecs", Form::Flag, Class::Plain),
    option("--noglob-pathspecs", Form::Flag, Class::Plain),
    option("--icase-pathspecs", Form::Flag, Class::Plain),
    option("--no-optional-locks", Form::Flag, Class::Plain),
    option("--no-advice", Form::Flag, Class::Plain),
    option("-v", Form::Flag, Class::Print),
    option("--version", Form::Flag, Class::Print),
    option("-h", Form::Flag, Class::Print),
    option("--help", Form::Flag, Class::Print),
    option("--exec-path", Form::Flag, Class::Print),
    option("--html-path", Form::Flag, Class::Print),
    option("--man-path", Form::Flag, Class::Print),
    option("--info-path", Form::Flag, Class::Print),
];

/// The option of the table that `word` is: a word of its own for the next and the flag
/// forms, `--name=value` for the joined form. `None` for a word that git refuses or that
/// this table does not know.
pub(crate) fn lookup(word: &str) -> Option<&'static GitOption> {
    match word.split_once('=') {
        Some((name, _)) if name.starts_with("--") => {
            OPTIONS.iter().find(|option| option.form == Form::Joined && option.name == name)
        }
        _ => OPTIONS.iter().find(|option| option.form != Form::Joined && option.name == word),
    }
}

/// One global option of a git command, as [`exact`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Global {
    /// The position of the option's word among the arguments.
    pub(crate) at: usize,
    /// The option of the table, or `None` for an option that the table does not know,
    /// which [`exact`] reads as a flag.
    pub(crate) option: Option<&'static GitOption>,
}

impl Global {
    /// The option's name: the word without `=value`, also for an option that the table
    /// does not know.
    pub(crate) fn name<'a, S: AsRef<str>>(&self, args: &'a [S]) -> &'a str {
        let word = args[self.at].as_ref();
        match word.split_once('=') {
            Some((name, _)) if self.option.is_none_or(|option| option.form == Form::Joined) => name,
            _ => word,
        }
    }

    /// True for the option `name` of the table, in either form.
    pub(crate) fn is(&self, name: &str) -> bool {
        self.option.is_some_and(|option| option.name == name)
    }

    /// The option's value: the next word, the text after `=`, or `None` for a flag and
    /// for a next form at the end of the line.
    pub(crate) fn value<'a, S: AsRef<str>>(&self, args: &'a [S]) -> Option<&'a str> {
        match self.option?.form {
            Form::Next => args.get(self.at + 1).map(AsRef::as_ref),
            Form::Joined => args[self.at].as_ref().split_once('=').map(|(_, value)| value),
            Form::Flag => None,
        }
    }
}

/// The global options and the subcommand of a git command, as [`exact`] reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitParse {
    /// The global options before the subcommand, in order.
    pub(crate) globals: Vec<Global>,
    /// The position of the subcommand among the arguments, or `None` when no word is
    /// left for it.
    pub(crate) sub: Option<usize>,
}

/// The global options and the subcommand of git with `args` (the words after `git`),
/// by the table: an option of the next form takes the next word, and the first word
/// that does not start with `-` is the subcommand. An option that the table does not
/// know counts as a flag, as the parsers of the exits always read it.
pub(crate) fn exact<S: AsRef<str>>(args: &[S]) -> GitParse {
    let mut globals = Vec::new();
    let mut at = 0;
    while let Some(word) = args.get(at).map(AsRef::as_ref) {
        if !word.starts_with('-') {
            return GitParse { globals, sub: Some(at) };
        }
        let option = lookup(word);
        globals.push(Global { at, option });
        at += match option {
            Some(GitOption { form: Form::Next, .. }) => 2,
            _ => 1,
        };
    }
    GitParse { globals, sub: None }
}

/// The words of a git command with only its plain options before the subcommand
/// removed (DL23), program first, or `None` when another option stands before it, no
/// subcommand follows, or the command is no `git`. A rule that allows reads this view.
pub(crate) fn plain_view(words: &[String]) -> Option<Vec<String>> {
    let (program, args) = words.split_first()?;
    let parse = exact(args);
    let plain = parse
        .globals
        .iter()
        .all(|global| global.option.is_some_and(|option| option.class == Class::Plain));
    let sub = parse.sub?;
    (plain && !parse.globals.is_empty())
        .then(|| std::iter::once(program.clone()).chain(args[sub..].iter().cloned()).collect())
}

/// How many options that the table does not know [`views`] reads both ways before it
/// counts every later word as a possible subcommand: 4 options, 16 readings.
const MAX_UNKNOWN: usize = 4;

/// How deep [`views`] reads an alias of an alias before it gives up and fails closed.
const MAX_ALIAS_DEPTH: usize = 4;

/// The dirs that a git command works in by its own options: the `-C` dirs in order,
/// each relative to the one before, and the `--work-tree` dir (DL25).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GitDirs {
    /// The words of each `-C`, in order.
    pub(crate) chain: Vec<String>,
    /// The word of `--work-tree`, the last one when there are several.
    pub(crate) work_tree: Option<String>,
}

impl GitDirs {
    /// The dir of the `-C` chain and the `--work-tree` dir, in normal form, from the
    /// dir `start` where the command starts. A dir that efr cannot place, such as a
    /// relative one when `start` is not known, is left out.
    pub(crate) fn resolve(&self, start: Option<&Path>, home: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        let mut cwd = start.map(Path::to_path_buf);
        for word in self.chain.iter().filter(|word| !word.is_empty()) {
            cwd = place(word, cwd.as_deref(), home);
        }
        if !self.chain.is_empty()
            && let Some(cwd) = &cwd
        {
            dirs.push(cwd.clone());
        }
        if let Some(tree) = self.work_tree.as_deref().and_then(|w| place(w, cwd.as_deref(), home)) {
            dirs.push(tree);
        }
        dirs
    }
}

/// `word` as a dir in normal form: `~` and `~/x` below `home`, an absolute path, or a
/// path relative to `base`.
fn place(word: &str, base: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let joined = if word == "~" {
        home.to_path_buf()
    } else if let Some(rest) = word.strip_prefix("~/") {
        home.join(rest)
    } else if word.starts_with('/') {
        PathBuf::from(word)
    } else if word.starts_with('~') {
        // NOTE: `~user` is another user's home, which efr does not know.
        return None;
    } else {
        base?.join(word)
    };
    normalize(&joined)
}

/// The readings of a git command for a rule that asks or denies, from [`views`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Views {
    /// Each reading as `git <subcommand> <rest>`, program first, with the global options
    /// before the subcommand removed.
    pub(crate) words: Vec<Vec<String>>,
    /// True when the line sets an alias for its subcommand that efr cannot read: a
    /// shell command (`!`), a value with a quote, a value from `--config-env`, or an
    /// alias of an alias too deep. A rule that asks or denies for `git` then matches.
    pub(crate) unreadable: bool,
    /// The dirs of each reading's `-C` and `--work-tree`.
    pub(crate) dirs: Vec<GitDirs>,
}

impl Views {
    fn add_words(&mut self, words: Vec<String>) {
        if !self.words.contains(&words) {
            self.words.push(words);
        }
    }

    fn add_dirs(&mut self, dirs: GitDirs) {
        if !self.dirs.contains(&dirs) {
            self.dirs.push(dirs);
        }
    }
}

/// An alias that a git command sets on its line with `-c alias.<name>=<value>`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Alias {
    /// The alias's name, in lower case, as git compares it.
    name: String,
    /// Its value, or `None` when efr cannot read it.
    value: Option<String>,
}

/// One reading of the global options, while [`views`] walks them.
#[derive(Debug, Clone, Default)]
struct Branch {
    at: usize,
    unknown: usize,
    dirs: GitDirs,
    aliases: Vec<Alias>,
}

/// The readings of the git command `words` (program first) for a rule that asks or
/// denies (DL22, DL25, DL26): every global option of the table is removed with its
/// value; an option that the table does not know is read as a flag and as an option
/// with a value in the next word; after [`MAX_UNKNOWN`] of them, every later word that
/// does not start with `-` counts as a possible subcommand. An alias that the line sets
/// for the subcommand adds the reading of its value. So such a rule fails towards
/// asking, also for a git that is newer than the table.
pub(crate) fn views(words: &[String]) -> Views {
    let mut out = Views::default();
    tight(words, Vec::new(), 0, &mut out);
    out
}

fn tight(words: &[String], aliases: Vec<Alias>, depth: usize, out: &mut Views) {
    let Some((program, args)) = words.split_first() else { return };
    let mut stack = vec![Branch { aliases, ..Branch::default() }];
    while let Some(mut branch) = stack.pop() {
        let Some(word) = args.get(branch.at) else { continue };
        if !word.starts_with('-') {
            reading(program, args, branch.at, &branch, depth, out);
            continue;
        }
        let Some(option) = lookup(word) else {
            if branch.unknown == MAX_UNKNOWN {
                for at in (branch.at + 1..args.len()).filter(|at| !args[*at].starts_with('-')) {
                    reading(program, args, at, &branch, depth, out);
                }
                continue;
            }
            branch.unknown += 1;
            let mut valued = branch.clone();
            valued.at += 2;
            branch.at += 1;
            stack.push(valued);
            stack.push(branch);
            continue;
        };
        let global = Global { at: branch.at, option: Some(option) };
        let value = global.value(args).map(str::to_owned);
        if option.form == Form::Next && value.is_none() {
            continue;
        }
        match option.name {
            "-C" => branch.dirs.chain.extend(value),
            "--work-tree" => branch.dirs.work_tree = value,
            "-c" | "--config-env" => {
                let from_env = option.name == "--config-env";
                if let Some(alias) = value.as_deref().and_then(|value| alias_of(value, from_env)) {
                    branch.aliases.retain(|known| known.name != alias.name);
                    branch.aliases.push(alias);
                }
            }
            _ => {}
        }
        branch.at += if option.form == Form::Next { 2 } else { 1 };
        stack.push(branch);
    }
}

/// The alias that the config value `value` of `-c` or `--config-env` sets, or `None`
/// for another key. A value from the environment, a shell command, a value with a quote
/// or a backslash, an empty value and a key with no value cannot be read.
fn alias_of(value: &str, from_env: bool) -> Option<Alias> {
    let (key, text) = match value.split_once('=') {
        Some((key, text)) => (key, Some(text)),
        None => (value, None),
    };
    let (section, name) = key.split_once('.')?;
    if !section.eq_ignore_ascii_case("alias") || name.is_empty() {
        return None;
    }
    let readable = |text: &&str| {
        !from_env
            && !text.trim().is_empty()
            && !text.starts_with('!')
            && !text.contains(['\'', '"', '\\'])
    };
    Some(Alias { name: name.to_ascii_lowercase(), value: text.filter(readable).map(str::to_owned) })
}

/// Adds the reading of git with `args` whose subcommand stands at `sub`, and the
/// reading of the alias that the branch sets for it.
fn reading(
    program: &str,
    args: &[String],
    sub: usize,
    branch: &Branch,
    depth: usize,
    out: &mut Views,
) {
    let view: Vec<String> =
        std::iter::once(program.to_owned()).chain(args[sub..].iter().cloned()).collect();
    out.add_words(view);
    out.add_dirs(branch.dirs.clone());
    let name = args[sub].to_ascii_lowercase();
    let Some(alias) = branch.aliases.iter().rev().find(|alias| alias.name == name) else {
        return;
    };
    match &alias.value {
        Some(value) if depth < MAX_ALIAS_DEPTH => {
            let expanded: Vec<String> = std::iter::once(program.to_owned())
                .chain(value.split_whitespace().map(str::to_owned))
                .chain(args[sub + 1..].iter().cloned())
                .collect();
            let mut inner = Views::default();
            tight(&expanded, branch.aliases.clone(), depth + 1, &mut inner);
            out.unreadable |= inner.unreadable;
            for words in inner.words {
                out.add_words(words);
            }
            for dirs in inner.dirs {
                // NOTE: git reads the alias's own `-C` after the line's.
                let mut chain = branch.dirs.chain.clone();
                chain.extend(dirs.chain);
                let work_tree = dirs.work_tree.or_else(|| branch.dirs.work_tree.clone());
                out.add_dirs(GitDirs { chain, work_tree });
            }
        }
        _ => out.unreadable = true,
    }
}

#[cfg(test)]
mod tests;
