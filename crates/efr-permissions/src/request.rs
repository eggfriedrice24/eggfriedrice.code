//! What the engine decides about: a tool call's declared requirements and where the
//! turn stands.
//!
//! `efr-tools` has its own `ToolRequirements`, and the forbidden edge
//! `efr-tools -> efr-permissions` keeps the two crates apart, so
//! `efr-conversation/src/turn.rs` copies one into [`Requirements`] at the check point.

use std::fmt;
use std::path::PathBuf;

use efr_protocol::{GitCodeFacts, Mode, Needs, Origin, Scope};

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
    /// True when the call may ask for a password or a question at the terminal through
    /// a program that [`interactive`](Self::interactive) does not cover, such as `rsync`
    /// to another host or `git` over ssh (the soft flag). The engine gives it no reason;
    /// the check point reads it for how long the call may run and for the question's
    /// text.
    pub may_prompt: bool,
    /// The change of efr's own settings that the call makes, which only the settings
    /// tool declares. It asks in every mode and is denied for a remote origin, whatever
    /// the rules say.
    pub settings: Option<SettingsChange>,
    /// True when the call deletes or moves a file, as a delete or a move of
    /// `apply_patch` does. The call asks in every mode, whatever the rules say, except
    /// in `auto` when `efr undo` can bring back each file that it writes
    /// ([`CallFacts::covered`], phase B). Its paths are judged as writes as well.
    pub destructive: bool,
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
    /// True when the call starts a background job (the shell tool's
    /// `background: true`), which runs on after the call answers, while nobody watches
    /// it (phase C).
    pub background: bool,
    /// The time limit of the job in minutes that the call asked for (the shell tool's
    /// `max_minutes`); `None` takes the default.
    pub max_minutes: Option<u32>,
    /// True when the job has a watch pattern (the shell tool's `watch`).
    pub watch: bool,
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

    /// Marks the call as one that may ask for a password or a question through a
    /// program that [`interactive`](Self::interactive) does not cover.
    pub fn with_may_prompt(mut self) -> Self {
        self.may_prompt = true;
        self
    }

    /// Marks the call as one that changes efr's own settings with `change`.
    pub fn with_settings_change(mut self, change: SettingsChange) -> Self {
        self.settings = Some(change);
        self
    }

    /// Marks the call as one that deletes or moves a file.
    pub fn with_destructive(mut self) -> Self {
        self.destructive = true;
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

    /// Marks the call as one that starts a background job.
    pub fn with_background(mut self) -> Self {
        self.background = true;
        self
    }

    /// Sets the time limit of the job that the call asked for, in minutes.
    pub fn with_max_minutes(mut self, minutes: u32) -> Self {
        self.max_minutes = Some(minutes);
        self
    }

    /// Marks the job as one with a watch pattern.
    pub fn with_watch(mut self) -> Self {
        self.watch = true;
        self
    }

    /// True when the call declared nothing.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
            && self.command.is_none()
            && !self.network
            && !self.interactive
            && self.settings.is_none()
            && !self.destructive
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
    /// Each declared path that reaches another declared path through a symbolic link,
    /// with that path: `(~/.zshrc, ~/dotfiles/zsh/.zshrc)`. A write of the real file
    /// takes the names of its link, so "my zshrc" names both.
    pub links: Vec<(PathBuf, PathBuf)>,
    /// The answer to each [`CoverAsk`] of [`FactRequest::covers`](crate::FactRequest):
    /// whether efr's snapshot or its home store holds what the line can destroy (phase
    /// B). An ask with no answer here is [`Coverage::Unknown`], never covered.
    pub covered: Vec<(CoverAsk, Coverage)>,
    /// What each file of [`FactRequest::sources`](crate::FactRequest) holds: the file
    /// whose bytes a persistence form installs, such as the source of a link into a
    /// unit dir (phase B). A source with no fact here never goes to the classifier.
    pub sources: Vec<SourceFact>,
    /// The git remotes of each directory of
    /// [`FactRequest::remotes`](crate::FactRequest), from one hardened git run per
    /// repository (phase D2, DP22). A directory with no fact here has no remote hosts.
    pub remotes: Vec<(PathBuf, Vec<RemoteFact>)>,
    /// The hosts that efr's proxy refused in this conversation's last call and in its
    /// jobs (phase D2, DP23). A fact for the reviewer, never a name: it shows only that
    /// a command tried the host.
    pub refused_hosts: Vec<String>,
    /// Each path of [`FactRequest::real_paths`](crate::FactRequest), the write grant
    /// paths and the named sockets of the call, with its path after every link (phase
    /// E). An always-allow rule covers no path that has no entry here.
    pub real_paths: Vec<(PathBuf, PathBuf)>,
    /// The forwarded ports of the conversation's running contained jobs, which a
    /// contained call of the conversation reaches on `127.0.0.1` and `[::1]` through its
    /// reach helper (v0.0.4 shell items, DS43). A URL on `localhost`, `127.0.0.1` or
    /// `[::1]` with such a port is no network use, and a `needs.hosts` entry of one
    /// makes no exit. Empty when efrd forwards nothing for the conversation; the fact
    /// only predicts, the sandbox enforces.
    pub reach_ports: Vec<u16>,
    /// The git code of the repositories of
    /// [`FactRequest::git_code`](crate::FactRequest), merged: checked only when efrd
    /// checked each one (v0.0.4, DV22). `None` for a line that asks for none; for a
    /// line that asks, `None` counts as not checked, and only the user may approve its
    /// `outside` or `upload` exit.
    pub git_code: Option<GitCodeFacts>,
    /// The sockets of [`FactRequest::sockets`](crate::FactRequest) that an ssh process
    /// listens on: ssh control sockets, each a logged-in session (OQS17). Only the user
    /// approves a socket grant of one. efrd lists every socket of the request here when
    /// it cannot look, so a lost fact asks the user.
    pub ssh_sockets: Vec<PathBuf>,
}

/// A repository whose git code efrd checks before a git or gh line may run outside the
/// sandbox (v0.0.4, DV22): its hooks, the config keys that run a program and the
/// programs that they run, as git finds them from the line's options.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitCodeAsk {
    /// The directory git starts in: the line's directory after each `-C`.
    pub dir: PathBuf,
    /// The `--git-dir` of the line, resolved from [`dir`](Self::dir).
    pub git_dir: Option<PathBuf>,
    /// The `--work-tree` of the line, resolved from [`dir`](Self::dir).
    pub work_tree: Option<PathBuf>,
    /// True when the line can reach the submodules (`push`, `fetch`, `pull`,
    /// `submodule update`, a gh command that pushes): efrd then checks each module git
    /// dir too.
    pub recurse: bool,
}

/// One git remote of a directory, or one submodule URL of its `.gitmodules` (phase D2,
/// DP22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFact {
    /// The remote's name, such as `origin`, or the submodule's name.
    pub name: String,
    /// Every fetch URL, after `insteadOf`.
    pub fetch: Vec<RemoteUrl>,
    /// Every URL that git pushes to: each `pushurl`, else each `url`, after
    /// `pushInsteadOf`.
    pub push: Vec<RemoteUrl>,
    /// A remote of a pinned project git dir whose URLs equal those of the
    /// conversation's first fact request (DP22). Never for a submodule.
    pub project: bool,
    /// The remote of the current branch.
    pub upstream: bool,
    /// A `submodule.*.url` of `.gitmodules`, not a `remote.*`.
    pub submodule: bool,
}

/// The host and port of one remote URL (phase D2, DP22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    /// The host, in lower case.
    pub host: String,
    /// The port: the URL's own, else the default of its scheme.
    pub port: u16,
    /// `ssh://`, `git@host:` or another form that git runs ssh for.
    pub ssh: bool,
    /// The URL holds a password or a token (DP25).
    pub credentials: bool,
}

impl CallFacts {
    /// Whether efr's snapshot or its home store covers `ask`, or
    /// [`Coverage::Unknown`] when no fact says.
    pub fn coverage(&self, ask: &CoverAsk) -> Coverage {
        self.covered
            .iter()
            .find(|(known, _)| known == ask)
            .map_or(Coverage::Unknown, |(_, coverage)| *coverage)
    }

    /// What `path` is, `Some(None)` when it does not exist, or `None` when no fact
    /// says.
    pub fn target(&self, path: &std::path::Path) -> Option<Option<TargetKind>> {
        self.targets.iter().find(|(known, _)| known == path).map(|(_, kind)| *kind)
    }

    /// How many tracked files lie below `dir`, when a fact says.
    pub fn tracked(&self, dir: &std::path::Path) -> Option<u32> {
        self.tracked_counts.iter().find(|(known, _)| known == dir).map(|(_, count)| *count)
    }

    /// What the daemon found for `source`, when a fact says.
    pub fn source(&self, source: &Source) -> Option<&SourceFact> {
        self.sources.iter().find(|fact| fact.source == *source)
    }
}

/// A file whose bytes a persistence form installs, which the classifier must read
/// before it may allow the form (efr's auto spec 9.1, plan PB19a): the source of
/// `ln -s <src>` into a unit dir or of `cp <src>` into a floor, the file of
/// `crontab <file>`, the unit file of `systemctl --user enable <unit>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// A file that the line names, resolved as the line resolves it.
    File(PathBuf),
    /// The unit file of this unit name, in the user's unit dirs.
    Unit(String),
}

/// What the daemon found for one [`Source`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFact {
    /// The source.
    pub source: Source,
    /// The real file, with every link followed; `None` when it is no regular file or
    /// the daemon found none.
    pub path: Option<PathBuf>,
    /// True when the file changed in this turn, or when the daemon could not tell.
    pub changed: bool,
    /// Its text, when it is UTF-8 and at most
    /// [`ActionFacts::MAX_EDIT_BYTES`](efr_protocol::ActionFacts::MAX_EDIT_BYTES), and no
    /// mask or secret holds it.
    pub text: Option<String>,
}

/// What a destructive line or file tool can destroy, which efr must hold before the
/// call so that `efr undo` can bring it back (efr's auto spec, section 10.7, phase B).
/// The engine asks; the daemon answers each ask in [`CallFacts::covered`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoverAsk {
    /// Each file at or below the path, inside a snapshot root.
    Path(PathBuf),
    /// Every tracked file of the git work tree at this root, and its index equal to
    /// `HEAD`: `git reset --hard`, `git checkout` or `git restore` that discards.
    Tracked(PathBuf),
    /// Every untracked file of the work tree at this root: `git clean -f`.
    Untracked(PathBuf),
    /// The branches, tags and `HEAD` of the repository at this root: `git branch -D`,
    /// or a `reset` or `switch` that moves a ref.
    Refs(PathBuf),
    /// The one stash entry of the repository at this root: `git stash drop` or
    /// `git stash clear`.
    Stash(PathBuf),
    /// One file outside the snapshot roots, which the home store copies.
    Home(PathBuf),
    /// A directory outside the snapshot roots that the call can change as a whole,
    /// within `snapshot.home_max_files` and `snapshot.home_max_mib`.
    HomeDir(PathBuf),
    /// The links that enabling this unit makes in the user's unit directories.
    UnitLinks(String),
    /// The user's crontab.
    CronTable,
    /// The linger setting of the user. A `loginctl` form that names a user carries
    /// that word here: efrd covers it only when the word is its own user, by name or
    /// by uid, so a form for another user stays with the user.
    Linger(Option<String>),
}

/// The daemon's answer to one [`CoverAsk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Coverage {
    /// efr holds all of it, so `efr undo` can bring it back.
    Covered,
    /// The snapshot left a file out, such as a large or an ignored file.
    LeftOut,
    /// More files or bytes than the limits of the copy.
    OverLimit,
    /// The sandbox hides the path, so efr never copies it.
    Masked,
    /// The root has no snapshot from before the turn, or a snapshot of it failed.
    NoSnapshot,
    /// The store has no copy of the project's refs.
    NoMirror,
    /// The ask names a path that no copy may cover, such as a root itself, a git
    /// directory or content that is only staged.
    NotAllowed,
    /// No fact says. Never covered.
    Unknown,
}

impl Coverage {
    /// True only for [`Coverage::Covered`].
    pub fn is_covered(self) -> bool {
        self == Coverage::Covered
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

#[cfg(test)]
mod tests;
