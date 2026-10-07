//! What the engine decides about: a tool call's declared requirements and where the
//! turn stands.
//!
//! `efr-tools` has its own `ToolRequirements`, and the forbidden edge
//! `efr-tools -> efr-permissions` keeps the two crates apart, so
//! `efr-conversation/src/turn.rs` copies one into [`Requirements`] at the check point.

use std::fmt;
use std::path::PathBuf;

use efr_protocol::{Mode, Needs, Origin, Scope};

use crate::Policy;

/// Everything that [`Engine::decide`](crate::Engine::decide) looks at for one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionInput {
    /// What the call declared that it needs.
    pub requirements: Requirements,
    /// The turn's scope, derived again from the shell's working directory every turn.
    pub scope: Scope,
    /// The surface that the turn came from.
    pub origin: Origin,
    /// The permission mode the turn runs with, which picks the built-in policy. The
    /// engine caps it at `cautious` for a remote origin
    /// ([`effective_mode`](crate::effective_mode)).
    pub mode: Mode,
    /// The conversation's `$SCRATCH` and its own rules.
    pub conversation_policy: ConversationPolicy,
}

/// What a tool call declared that it needs.
///
/// The engine cannot judge what a tool does not declare, so declaring is the tool's
/// contract. A call that declares nothing is allowed for a turn from the shell, the CLI
/// or the proxy, and needs approval for a turn from any other origin, which fails
/// closed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requirements {
    /// Paths that the call reads or writes, each as the absolute path the tool resolved.
    pub paths: Vec<PathAccess>,
    /// The command line that the call runs in the hidden shell, as the model wrote it.
    pub command: Option<String>,
    /// The directory the command line starts in: where the hidden shell is, or where a
    /// new one starts. A command rule with `under` matches only when it is known.
    pub command_dir: Option<PathBuf>,
    /// True when the call talks to the network itself.
    pub network: bool,
    /// True when the call may wait for input at the terminal, such as a `sudo` prompt.
    pub interactive: bool,
    /// The change of efr's own settings that the call makes, which only the settings
    /// tool declares. It asks in every mode and is denied for a remote origin, whatever
    /// the rules say.
    pub settings: Option<SettingsChange>,
    /// What the model asks for beyond the `auto` sandbox (the shell tool's `needs`).
    /// Each entry is an exit that asks; outside `auto` it is ignored.
    pub needs: Option<Needs>,
    /// True when the call types its line into a shell that runs inside the hidden one
    /// (the shell tool's `nested_shell`). The `auto` mode denies it.
    pub nested: bool,
    /// Facts about the files and programs of the line that the daemon collected, for
    /// the exits of the `auto` mode. `None`, or a path that they leave out, makes the
    /// engine assume the stricter case.
    pub facts: Option<CallFacts>,
}

impl Requirements {
    /// No requirements.
    pub fn none() -> Self {
        Requirements::default()
    }

    /// Adds a path that the call reads.
    pub fn with_read(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(PathAccess { path: path.into(), access: Access::Read });
        self
    }

    /// Adds a directory that the call reads with everything below it, such as the root
    /// of a recursive search.
    pub fn with_read_tree(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(PathAccess { path: path.into(), access: Access::ReadTree });
        self
    }

    /// Adds a path that the call writes.
    pub fn with_write(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(PathAccess { path: path.into(), access: Access::Write });
        self
    }

    /// Sets the command line that the call runs.
    pub fn with_command(mut self, line: impl Into<String>) -> Self {
        self.command = Some(line.into());
        self
    }

    /// Sets the directory the command line starts in.
    pub fn with_command_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.command_dir = Some(dir.into());
        self
    }

    /// Marks the call as one that talks to the network.
    pub fn with_network(mut self) -> Self {
        self.network = true;
        self
    }

    /// Marks the call as one that may wait for input at the terminal.
    pub fn with_interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// Marks the call as one that changes efr's own settings with `change`.
    pub fn with_settings_change(mut self, change: SettingsChange) -> Self {
        self.settings = Some(change);
        self
    }

    /// Sets what the model asks for beyond the `auto` sandbox.
    pub fn with_needs(mut self, needs: Needs) -> Self {
        self.needs = Some(needs);
        self
    }

    /// Marks the call as one that types its line into a nested shell.
    pub fn with_nested(mut self) -> Self {
        self.nested = true;
        self
    }

    /// Sets the facts that the daemon collected about the line.
    pub fn with_facts(mut self, facts: CallFacts) -> Self {
        self.facts = Some(facts);
        self
    }

    /// True when the call declared nothing.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
            && self.command.is_none()
            && !self.network
            && !self.interactive
            && self.settings.is_none()
            && self.needs.as_ref().is_none_or(Needs::is_empty)
            && !self.nested
    }
}

/// Facts about the files and programs of a command line, which the daemon collects
/// before the engine decides, because the engine reads no file.
///
/// The engine assumes the stricter case for a fact that is missing: a target exists,
/// and a directory holds tracked files. So a lost fact asks, and never runs more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallFacts {
    /// Paths that the line names, each with what it is now, or `None` when it does not
    /// exist. The parents of a write target belong here too, so the engine can find the
    /// nearest one that exists.
    pub targets: Vec<(PathBuf, Option<TargetKind>)>,
    /// How many files git tracks below a directory, from the hardened `git status`.
    pub tracked_counts: Vec<(PathBuf, u32)>,
    /// Each program word of the line: the word, the path it resolves to, and whether
    /// that file changed in this turn.
    pub programs: Vec<(String, Option<PathBuf>, bool)>,
}

impl CallFacts {
    /// What `path` is, `Some(None)` when it does not exist, or `None` when no fact
    /// says.
    pub fn target(&self, path: &std::path::Path) -> Option<Option<TargetKind>> {
        self.targets.iter().find(|(known, _)| known == path).map(|(_, kind)| *kind)
    }

    /// How many tracked files lie below `dir`, when a fact says.
    pub fn tracked(&self, dir: &std::path::Path) -> Option<u32> {
        self.tracked_counts.iter().find(|(known, _)| known == dir).map(|(_, count)| *count)
    }
}

/// What a path is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// Anything else: a device, a socket, a pipe.
    Other,
}

/// A change of efr's own settings (`config.toml`), as the settings tool plans it.
///
/// The engine never reads the file; the summary is what the approval shows, and
/// `loosens` marks a change that lets more run without a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsChange {
    /// What changes, in plain words, such as `set model.name = "gpt-5.4"`.
    pub summary: String,
    /// True when the change loosens permissions: a new allow rule, a mode toward
    /// `auto`, `per_call` to `keep`, a removed deny or ask rule, a removed secret path.
    pub loosens: bool,
}

impl SettingsChange {
    /// A change described by `summary`.
    pub fn new(summary: impl Into<String>, loosens: bool) -> Self {
        SettingsChange { summary: summary.into(), loosens }
    }
}

/// One path and what the call does with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAccess {
    /// The path. A relative path is denied, because its class is unknown.
    pub path: PathBuf,
    /// Read or write.
    pub access: Access,
}

/// What a call does with a path. Creating, deleting and changing the mode of a file
/// count as writing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Access {
    /// The call reads the path or lists the directory.
    Read,
    /// The call reads the path and may read anything below it, as a recursive search,
    /// a recursive listing or a glob does. A rule for reading matches it, and the
    /// engine also asks when a secret lies below the path.
    ReadTree,
    /// The call creates, changes or deletes the path.
    Write,
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Access::Read => "read",
            Access::ReadTree => "read all under",
            Access::Write => "write",
        })
    }
}

/// What one conversation adds to the machine policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationPolicy {
    /// The conversation's `$SCRATCH` directory. Paths inside it are
    /// [`PathClass::Scratch`](crate::PathClass::Scratch). A relative path, `/`, `~` or
    /// a directory above `~` makes nothing scratch.
    pub scratch: PathBuf,
    /// Rules that the conversation collected, such as "always allow writes under
    /// `~/.config/nvim`". They are read after the machine policy, so they win, except
    /// that they can only tighten decisions about secrets and system paths.
    pub rules: Policy,
}

impl ConversationPolicy {
    /// A policy with the given `$SCRATCH` and no rules of its own.
    pub fn new(scratch: impl Into<PathBuf>) -> Self {
        ConversationPolicy { scratch: scratch.into(), rules: Policy::empty() }
    }

    /// Replaces the conversation's rules.
    pub fn with_rules(mut self, rules: Policy) -> Self {
        self.rules = rules;
        self
    }
}
