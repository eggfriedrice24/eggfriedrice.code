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
//! ```

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::path_class::normalize;
use crate::{Access, Effect, PathClass, PermissionsError, Subject};

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
    /// Reading a path.
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
    /// A command line that matches the pattern.
    Command(CommandPattern),
}

/// A command that a rule names: a program and the first arguments.
///
/// A pattern matches only a simple command line: plain words of ASCII letters, digits
/// and `-_./:,+@%=` separated by spaces or tabs. A line with any other character, such
/// as `;`, `|`, `$`, a quote, a glob or a newline, never matches, because its first
/// word does not say what runs; it falls through to the rules before. A pattern with
/// no arguments matches every invocation of the program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandPattern {
    /// The first word of the line, such as `git`.
    pub program: String,
    /// The words that must follow it, such as `["status"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl CommandPattern {
    /// A pattern that matches every invocation of `program`.
    pub fn new(program: impl Into<String>) -> Self {
        CommandPattern { program: program.into(), args: Vec::new() }
    }

    /// Requires these words right after the program.
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// True when `line` is a simple command line whose words start with the program
    /// and then the arguments of this pattern.
    pub fn matches(&self, line: &str) -> bool {
        let Some(words) = simple_words(line) else {
            return false;
        };
        let mut pattern =
            std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str));
        let mut words = words.into_iter();
        pattern.all(|expected| words.next() == Some(expected))
    }
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
    /// Fails on the first rule that names a relative path, a program or argument that
    /// is not a plain word, or an action that its resource can never match.
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

    /// The built-in policy, the path-class table of the design:
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
    ///
    /// So commands and network access need approval, reading is free outside secrets,
    /// and only scratch and the user data of the turn's project are free to write.
    pub fn defaults() -> Self {
        let rules = vec![
            Rule::new(Action::Any, Resource::Any, Effect::Ask),
            Rule::new(Action::Read, Resource::Any, Effect::Allow),
            Rule::new(Action::Write, Resource::Class(PathClass::UserData), Effect::Ask),
            Rule::new(Action::Write, Resource::Project, Effect::Allow),
            Rule::new(Action::Write, Resource::Class(PathClass::UserConfig), Effect::Ask),
            Rule::new(Action::Write, Resource::Class(PathClass::System), Effect::Ask),
            Rule::new(Action::Write, Resource::Class(PathClass::Scratch), Effect::Allow),
            Rule::new(Action::Any, Resource::Class(PathClass::Secrets), Effect::Deny),
        ];
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

    /// The position and effect of the last rule that matches `subject`.
    pub(crate) fn last_match(
        &self,
        subject: &Subject,
        cx: &MatchContext<'_>,
    ) -> Option<(usize, Effect)> {
        self.rules
            .iter()
            .enumerate()
            .rev()
            .find(|(_, rule)| rule.matches(subject, cx))
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

/// What rule matching needs beyond the subject.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MatchContext<'a> {
    /// The home directory, for `~` in `under` paths.
    pub(crate) home: &'a Path,
    /// The root of the turn's project, when it may widen permissions.
    pub(crate) project_root: Option<&'a Path>,
}

impl Rule {
    fn matches(&self, subject: &Subject, cx: &MatchContext<'_>) -> bool {
        match subject {
            Subject::Path { path, access, class: Some(class) } => {
                let action = match access {
                    Access::Read => Action::Read,
                    Access::Write => Action::Write,
                };
                self.action_is(action) && self.matches_path(path, *class, cx)
            }
            Subject::Command { line } => {
                self.action_is(Action::Execute)
                    && match &self.resource {
                        Resource::Any => true,
                        Resource::Command(pattern) => pattern.matches(line),
                        Resource::Class(_) | Resource::Under(_) | Resource::Project => false,
                    }
            }
            Subject::Network => self.action_is(Action::Network) && self.resource == Resource::Any,
            // Relative paths, interactive calls and empty requirements are decided by
            // the engine itself, never by a rule.
            Subject::Path { class: None, .. } | Subject::Interactive | Subject::Nothing => false,
        }
    }

    fn action_is(&self, action: Action) -> bool {
        self.action == Action::Any || self.action == action
    }

    fn matches_path(&self, path: &Path, class: PathClass, cx: &MatchContext<'_>) -> bool {
        match &self.resource {
            Resource::Any => true,
            Resource::Class(wanted) => *wanted == class,
            Resource::Under(root) => {
                expand(root, cx.home).is_some_and(|root| path.starts_with(root))
            }
            Resource::Project => cx.project_root.is_some_and(|root| path.starts_with(root)),
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
            let home_relative = path.components().next() == Some(Component::Normal("~".as_ref()));
            if !home_relative && normalize(path).is_none() {
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
            if let Some(argument) = pattern.args.iter().find(|argument| !is_plain_word(argument)) {
                return Err(PermissionsError::RuleArgumentInvalid {
                    index,
                    argument: argument.clone(),
                });
            }
        }
        Resource::Any | Resource::Class(_) | Resource::Project => {}
    }
    Ok(())
}

/// The words of a simple command line, or `None` when the line holds a character that
/// the shell may treat specially, or no word at all.
fn simple_words(line: &str) -> Option<Vec<&str>> {
    if !line.chars().all(|c| c == ' ' || c == '\t' || is_plain_char(c)) {
        return None;
    }
    let words: Vec<&str> = line.split([' ', '\t']).filter(|word| !word.is_empty()).collect();
    if words.is_empty() { None } else { Some(words) }
}

fn is_plain_word(word: &str) -> bool {
    !word.is_empty() && word.chars().all(is_plain_char)
}

/// Characters that neither bash nor zsh treat specially inside a word. The list is an
/// allowlist on purpose: a character that is missing makes a line fall back to the
/// rules before, which is safe; a character wrongly present would let an allowed
/// program smuggle in another command.
fn is_plain_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '-' | '_' | '.' | '/' | ':' | ',' | '+' | '@' | '%' | '=')
}

#[cfg(test)]
mod tests;
