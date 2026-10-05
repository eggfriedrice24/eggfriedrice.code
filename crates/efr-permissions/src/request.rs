//! What the engine decides about: a tool call's declared requirements and where the
//! turn stands.
//!
//! `efr-tools` has its own `ToolRequirements`, and the forbidden edge
//! `efr-tools -> efr-permissions` keeps the two crates apart, so
//! `efr-conversation/src/turn.rs` copies one into [`Requirements`] at the check point.

use std::fmt;
use std::path::PathBuf;

use efr_protocol::{Mode, Origin, Scope};

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

    /// True when the call declared nothing.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.command.is_none() && !self.network && !self.interactive
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
