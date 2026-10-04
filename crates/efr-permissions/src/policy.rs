//! Ordered rules: an action, a resource and an effect, where the last rule that matches
//! a requirement decides it.
//!
//! Last match wins, as in a firewall's ordered rule list, so a policy reads from the
//! general to the specific and the user's own rules go after the defaults. In the
//! configuration a rule looks like this:
//!
//! ```toml
//! { action = "write", resource = { under = "~/.config/nvim" }, effect = "allow" }
//! { action = "execute", resource = { command = { program = "git", args = ["status"] } }, effect = "allow" }
//! { action = "execute", resource = { command = { program = "find", forbid = ["-delete"] } }, effect = "allow" }
//! ```

mod defaults;

use std::borrow::Cow;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command::{self, is_plain_char};
use crate::path_class::{normalize, rehome};
use crate::{Access, Effect, PathClass, PermissionsError};

/// One rule: when a requirement matches `action` and `resource`, its effect is
/// `effect`, unless a later rule matches too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// What the requirement does.
    pub action: Action,
    /// What it does it to.
    pub resource: Resource,
    /// The effect when the rule decides.
    pub effect: Effect,
}

impl Rule {
    /// A rule from its three parts. [`Policy::new`] checks it.
    pub fn new(action: Action, resource: Resource, effect: Effect) -> Self {
        Rule { action, resource, effect }
    }
}

/// What a requirement does, as a rule matches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Every requirement.
    Any,
    /// Reading a path, or everything below it.
    Read,
    /// Writing a path.
    Write,
    /// Running a command line.
    Execute,
    /// Network access.
    Network,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Action::Any => "any",
            Action::Read => "read",
            Action::Write => "write",
            Action::Execute => "execute",
            Action::Network => "network",
        })
    }
}

/// What a requirement acts on, as a rule matches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    /// Every path, command line and network access.
    Any,
    /// Every path of one class.
    Class(PathClass),
    /// A path and everything below it. It is absolute or starts with `~`, which stands
    /// for the home directory.
    Under(PathBuf),
    /// Every path inside the root of the turn's registered project. It matches nothing
    /// when the scope is not a registered project, or when that root is `~`, `/` or a
    /// directory above `~`.
    Project,
    /// Every simple command that matches the pattern.
    Command(CommandPattern),
}

/// A command that a rule names: a program, the words that must follow it, the words
/// that must not appear, and how many operands it may have.
///
/// A pattern judges one simple command at a time. The engine splits a line on `;`,
/// `&&`, `||`, `|` and newlines, reading quotes as zsh does, and allows a line only
/// when it allows every simple command in it. A line with a command substitution, a
/// parameter expansion, a here-document, an output redirection to anything but
/// `/dev/null`, a group, a background job, an assignment that could change what runs,
/// or a builtin such as `eval` matches no pattern at all, and falls through to the
/// rules for every command line. A program that runs commands as another user (`sudo`,
/// `doas`, `su`, `pkexec`, `run0`) matches no pattern either, even behind a wrapper
/// such as `env`.
///
/// Words are compared after quotes are removed, so `git 'status'` is `git status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandPattern {
    /// The first word of the simple command, such as `git`. A path such as `/bin/ls`
    /// names a different program.
    pub program: String,
    /// The words that must follow the program, in order, such as `["status"]`. A word
    /// may list alternatives separated by `|`, as in `"status|diff|log"`, and an
    /// alternative that ends in `*` matches every word that starts with what comes
    /// before it, as `-Q*` matches `-Qi`. No words matches every invocation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Words that must not appear anywhere after the program:
    ///
    /// - `--name` matches `--name`, `--name=value`, and every abbreviation of it down
    ///   to `--n`, because GNU programs accept any unambiguous abbreviation;
    /// - `-x`, one letter, matches every word that starts with one `-` and holds the
    ///   letter, so `-o` also matches `-uo`;
    /// - `-name`, several letters after one `-`, matches that word, the way `find`
    ///   reads its predicates;
    /// - an option that ends in `*` matches every word that starts with it;
    /// - a word without a leading `-` matches every word without one that contains it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbid: Vec<String>,
    /// The most operands after the words of `args`. An operand is `-`, a word that
    /// does not start with `-`, or any word after `--`. A value given to an option as a
    /// separate word counts too, so the limit errs towards asking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_operands: Option<usize>,
    /// The fewest operands after the words of `args`, counted like `max_operands`, such
    /// as the unit that `systemctl show` must name, because without one it shows the
    /// service manager's environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_operands: Option<usize>,
    /// The most options after the words of `args`. An option is a word that starts with
    /// `-`, other than `-` itself, up to and including `--`. With `max_operands`, a
    /// limit of 0 allows the words of `args` and nothing after them, such as `ps -ef`
    /// alone: one more option could make the program read the whole line another way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_options: Option<usize>,
    /// A directory the command must run in, or below: absolute or starting with `~`.
    /// The directory is where the hidden shell is when the line starts; after a `cd`,
    /// `pushd` or `popd` earlier in the line it is unknown, and the pattern matches
    /// nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<PathBuf>,
}

impl CommandPattern {
    /// A pattern that matches every invocation of `program`.
    pub fn new(program: impl Into<String>) -> Self {
        CommandPattern {
            program: program.into(),
            args: Vec::new(),
            forbid: Vec::new(),
            max_operands: None,
            min_operands: None,
            max_options: None,
            under: None,
        }
    }

    /// Requires these words right after the program.
    #[must_use]
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Refuses a command that holds one of these words after the program.
    #[must_use]
    pub fn with_forbid<I, S>(mut self, forbid: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.forbid = forbid.into_iter().map(Into::into).collect();
        self
    }

    /// Refuses a command with more than `max` operands after the words of `args`.
    #[must_use]
    pub fn with_max_operands(mut self, max: usize) -> Self {
        self.max_operands = Some(max);
        self
    }

    /// Refuses a command with fewer than `min` operands after the words of `args`.
    #[must_use]
    pub fn with_min_operands(mut self, min: usize) -> Self {
        self.min_operands = Some(min);
        self
    }

    /// Refuses a command with more than `max` options after the words of `args`.
    #[must_use]
    pub fn with_max_options(mut self, max: usize) -> Self {
        self.max_options = Some(max);
        self
    }

    /// Requires the command to run in `dir` or below it.
    #[must_use]
    pub fn with_under(mut self, dir: impl Into<PathBuf>) -> Self {
        self.under = Some(dir.into());
        self
    }

    /// True when `line` is one simple command that this pattern matches, and it does
    /// not run as another user. A pattern with [`under`](Self::under) matches no line
    /// here, where the directory is unknown.
    pub fn matches(&self, line: &str) -> bool {
        match command::analyze(line).as_deref() {
            Ok([only]) => {
                self.under.is_none()
                    && command::privileged(only).is_none()
                    && self.matches_words(&only.words)
            }
            Ok(_) | Err(_) => false,
        }
    }

    /// True when the words of one simple command, program first, match this pattern.
    pub(crate) fn matches_words(&self, words: &[String]) -> bool {
        let Some((program, rest)) = words.split_first() else {
            return false;
        };
        if *program != self.program || rest.len() < self.args.len() {
            return false;
        }
        let prefix = self.args.iter().zip(rest).all(|(wanted, word)| {
            wanted.split('|').any(|alternative| match alternative.strip_suffix('*') {
                Some(start) => word.starts_with(start),
                None => word == alternative,
            })
        });
        if !prefix || rest.iter().any(|word| self.forbid.iter().any(|entry| forbids(entry, word))) {
            return false;
        }
        let after = &rest[self.args.len()..];
        let count = operands(after);
        self.max_operands.is_none_or(|max| count <= max)
            && self.min_operands.is_none_or(|min| count >= min)
            && self.max_options.is_none_or(|max| options(after) <= max)
    }
}

/// True when the forbidden `entry` matches `word`, by the rules of
/// [`CommandPattern::forbid`].
fn forbids(entry: &str, word: &str) -> bool {
    let (option, wild) = match entry.strip_suffix('*') {
        Some(start) => (start, true),
        None => (entry, false),
    };
    if option.starts_with("--") {
        if !word.starts_with("--") || word == "--" {
            return false;
        }
        let name = word.split_once('=').map_or(word, |(name, _)| name);
        let abbreviation = name.len() > 2 && option.starts_with(name);
        return name == option || abbreviation || (wild && name.starts_with(option));
    }
    if let Some(letters) = option.strip_prefix('-') {
        let mut chars = letters.chars();
        if let (Some(letter), None, false) = (chars.next(), chars.next(), wild) {
            return word.starts_with('-') && !word.starts_with("--") && word[1..].contains(letter);
        }
        return if wild { word.starts_with(option) } else { word == option };
    }
    !word.starts_with('-') && word.contains(option)
}

/// How many of `words` are operands: `-`, a word without a leading `-`, or any word
/// after `--`.
fn operands(words: &[String]) -> usize {
    let mut count = 0;
    let mut after_options = false;
    for word in words {
        if after_options || word == "-" || !word.starts_with('-') {
            count += 1;
        } else if word == "--" {
            after_options = true;
        }
    }
    count
}

/// How many of `words` are options: words that start with `-`, other than `-`, up to
/// and including `--`.
fn options(words: &[String]) -> usize {
    let mut count = 0;
    for word in words {
        if word.starts_with('-') && word != "-" {
            count += 1;
            if word == "--" {
                break;
            }
        }
    }
    count
}

/// An ordered list of rules in which the last match wins.
///
/// Every rule is checked when the policy is built, from code or from the configuration,
/// so a policy never holds a rule that cannot match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Rule>", into = "Vec<Rule>")]
pub struct Policy {
    rules: Vec<Rule>,
}

impl Policy {
    /// A policy of the given rules, in order.
    ///
    /// Fails on the first rule that names a relative path, a program, argument or
    /// forbidden word that is not a plain word, or an action that its resource can
    /// never match.
    pub fn new(rules: Vec<Rule>) -> Result<Self, PermissionsError> {
        for (index, rule) in rules.iter().enumerate() {
            check(index, rule)?;
        }
        Ok(Policy { rules })
    }

    /// A policy with no rules. Alone it denies every requirement.
    pub fn empty() -> Self {
        Policy::default()
    }

    /// The built-in policy: the path-class table of the design, then read-only
    /// commands that run without approval.
    ///
    /// | # | Action | Resource | Effect |
    /// |---|---|---|---|
    /// | 0 | any | any | ask |
    /// | 1 | read | any | allow |
    /// | 2 | write | class user data | ask |
    /// | 3 | write | project | allow |
    /// | 4 | write | class user config | ask |
    /// | 5 | write | class system | ask |
    /// | 6 | write | class scratch | allow |
    /// | 7 | any | class secrets | deny |
    /// | 8 and on | execute | a read-only command below | allow |
    ///
    /// So reading is free outside secrets, only scratch and the user data of the turn's
    /// project are free to write, network access needs approval, and a command line
    /// needs approval unless every simple command in it is one of these (`args` must
    /// follow the program, `forbid` must not appear, `min` and `max` bound the
    /// operands):
    ///
    /// | # | Program | Args | Forbid | Min | Max |
    /// |---|---|---|---|---|---|
    #[doc = include_str!("policy/defaults.md")]
    ///
    /// `env` and `printenv` are left out on purpose, because they print every variable,
    /// tokens included. A read-only command still reads paths: the shell tool declares
    /// the paths among its arguments, and the path rules above judge them, so
    /// `cat ~/.ssh/id_ed25519` is denied although `cat` is allowed.
    pub fn defaults() -> Self {
        let mut rules = vec![
            Rule::new(Action::Any, Resource::Any, Effect::Ask),
            Rule::new(Action::Read, Resource::Any, Effect::Allow),
            Rule::new(Action::Write, Resource::Class(PathClass::UserData), Effect::Ask),
            Rule::new(Action::Write, Resource::Project, Effect::Allow),
            Rule::new(Action::Write, Resource::Class(PathClass::UserConfig), Effect::Ask),
            Rule::new(Action::Write, Resource::Class(PathClass::System), Effect::Ask),
            Rule::new(Action::Write, Resource::Class(PathClass::Scratch), Effect::Allow),
            Rule::new(Action::Any, Resource::Class(PathClass::Secrets), Effect::Deny),
        ];
        rules.extend(
            defaults::read_only().into_iter().map(|pattern| {
                Rule::new(Action::Execute, Resource::Command(pattern), Effect::Allow)
            }),
        );
        Policy { rules }
    }

    /// The rules, in order.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// This policy followed by `later`, whose rules therefore win.
    pub fn then(mut self, later: Policy) -> Policy {
        self.rules.extend(later.rules);
        self
    }

    /// Appends a rule, which then wins over every rule before it.
    pub fn push(&mut self, rule: Rule) -> Result<(), PermissionsError> {
        check(self.rules.len(), &rule)?;
        self.rules.push(rule);
        Ok(())
    }

    /// The position and effect of the last rule that matches `target`.
    pub(crate) fn last_match(
        &self,
        target: &Target<'_>,
        cx: &MatchContext<'_>,
    ) -> Option<(usize, Effect)> {
        self.rules
            .iter()
            .enumerate()
            .rev()
            .find(|(_, rule)| rule.matches(target, cx))
            .map(|(index, rule)| (index, rule.effect))
    }
}

impl TryFrom<Vec<Rule>> for Policy {
    type Error = PermissionsError;

    fn try_from(rules: Vec<Rule>) -> Result<Self, Self::Error> {
        Policy::new(rules)
    }
}

impl From<Policy> for Vec<Rule> {
    fn from(policy: Policy) -> Self {
        policy.rules
    }
}

/// What a rule is matched against: one path, one simple command, a command line that
/// cannot be split, or network access.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Target<'a> {
    /// A path in normal form, with its class.
    Path {
        /// The path.
        path: &'a Path,
        /// What the call does with it.
        access: Access,
        /// Its class.
        class: PathClass,
    },
    /// One simple command of a line, program first, unquoted.
    Command {
        /// The words.
        words: &'a [String],
        /// True when it runs as another user, which no command pattern allows.
        privileged: bool,
        /// The directory it runs in, in normal form, when it is known.
        dir: Option<&'a Path>,
    },
    /// A line that cannot be split into simple commands: only the rules for every
    /// command line match it.
    Opaque,
    /// Network access by the call itself.
    Network,
}

/// What rule matching needs beyond the target.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MatchContext<'a> {
    /// The home directory, for `~` in `under` paths.
    pub(crate) home: &'a Path,
    /// The other forms of the home directory. A path or an `under` root in one of them
    /// is compared as the same path under `home`.
    pub(crate) home_aliases: &'a [PathBuf],
    /// The root of the turn's project, when it may widen permissions, already under
    /// `home` when it lies under an alias.
    pub(crate) project_root: Option<&'a Path>,
}

impl MatchContext<'_> {
    fn rehome<'p>(&self, path: &'p Path) -> Cow<'p, Path> {
        rehome(self.home, self.home_aliases, path)
    }
}

impl Rule {
    fn matches(&self, target: &Target<'_>, cx: &MatchContext<'_>) -> bool {
        match *target {
            Target::Path { path, access, class } => {
                let action = match access {
                    Access::Read | Access::ReadTree => Action::Read,
                    Access::Write => Action::Write,
                };
                self.action_is(action) && self.matches_path(path, class, cx)
            }
            Target::Command { words, privileged, dir } => {
                self.action_is(Action::Execute)
                    && match &self.resource {
                        Resource::Any => true,
                        Resource::Command(pattern) => {
                            !privileged
                                && pattern.matches_words(words)
                                && pattern.under.as_deref().is_none_or(|root| {
                                    dir.is_some_and(|dir| self.contains(root, dir, cx))
                                })
                        }
                        Resource::Class(_) | Resource::Under(_) | Resource::Project => false,
                    }
            }
            Target::Opaque => self.action_is(Action::Execute) && self.resource == Resource::Any,
            Target::Network => self.action_is(Action::Network) && self.resource == Resource::Any,
        }
    }

    fn action_is(&self, action: Action) -> bool {
        self.action == Action::Any || self.action == action
    }

    /// True when `path` is `root`, an `under` path, or below it, in any form of `~`.
    fn contains(&self, root: &Path, path: &Path, cx: &MatchContext<'_>) -> bool {
        expand(root, cx.home).is_some_and(|root| cx.rehome(path).starts_with(cx.rehome(&root)))
    }

    fn matches_path(&self, path: &Path, class: PathClass, cx: &MatchContext<'_>) -> bool {
        match &self.resource {
            Resource::Any => true,
            Resource::Class(wanted) => *wanted == class,
            Resource::Under(root) => self.contains(root, path, cx),
            Resource::Project => {
                cx.project_root.is_some_and(|root| cx.rehome(path).starts_with(root))
            }
            Resource::Command(_) => false,
        }
    }
}

/// An `under` path in normal form, with a leading `~` replaced by the home directory.
fn expand(root: &Path, home: &Path) -> Option<PathBuf> {
    match root.strip_prefix("~") {
        Ok(rest) => normalize(&home.join(rest)),
        Err(_) => normalize(root),
    }
}

fn check(index: usize, rule: &Rule) -> Result<(), PermissionsError> {
    let fits = match &rule.resource {
        Resource::Any => true,
        Resource::Class(_) | Resource::Under(_) | Resource::Project => {
            matches!(rule.action, Action::Any | Action::Read | Action::Write)
        }
        Resource::Command(_) => matches!(rule.action, Action::Any | Action::Execute),
    };
    if !fits {
        return Err(PermissionsError::RuleNeverMatches { index, action: rule.action });
    }
    match &rule.resource {
        Resource::Under(path) => {
            if !is_rooted(path) {
                return Err(PermissionsError::RulePathNotAbsolute { index, path: path.clone() });
            }
        }
        Resource::Command(pattern) => {
            if !is_plain_word(&pattern.program) || pattern.program.starts_with(['-', '=']) {
                return Err(PermissionsError::RuleProgramInvalid {
                    index,
                    program: pattern.program.clone(),
                });
            }
            if let Some(argument) = pattern.args.iter().find(|argument| !is_valid_arg(argument)) {
                return Err(PermissionsError::RuleArgumentInvalid {
                    index,
                    argument: argument.clone(),
                });
            }
            if let Some(word) = pattern.forbid.iter().find(|word| !is_valid_forbid(word)) {
                return Err(PermissionsError::RuleForbidInvalid { index, word: word.clone() });
            }
            if let Some(path) = pattern.under.as_deref().filter(|path| !is_rooted(path)) {
                return Err(PermissionsError::RulePathNotAbsolute {
                    index,
                    path: path.to_path_buf(),
                });
            }
        }
        Resource::Any | Resource::Class(_) | Resource::Project => {}
    }
    Ok(())
}

/// True for an `under` path: absolute, or starting with `~` for the home directory.
fn is_rooted(path: &Path) -> bool {
    path.components().next() == Some(Component::Normal("~".as_ref())) || normalize(path).is_some()
}

fn is_plain_word(word: &str) -> bool {
    !word.is_empty() && word.chars().all(is_plain_char)
}

/// An `args` word: alternatives separated by `|`, each a plain word that may end in
/// `*` after at least one character.
fn is_valid_arg(word: &str) -> bool {
    word.split('|')
        .all(|alternative| is_plain_word(alternative.strip_suffix('*').unwrap_or(alternative)))
}

/// A `forbid` word: a plain word other than `-` and `--`; only an option may end in
/// `*`.
fn is_valid_forbid(word: &str) -> bool {
    let (body, wild) = match word.strip_suffix('*') {
        Some(body) => (body, true),
        None => (word, false),
    };
    is_plain_word(body) && body != "-" && body != "--" && (!wild || body.starts_with('-'))
}

#[cfg(test)]
mod tests;
