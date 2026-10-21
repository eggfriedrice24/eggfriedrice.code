//! Exits: what a call of the `auto` mode needs beyond its sandbox.
//!
//! [`predict`] reads the line before the run, with the paths the tool declared and the
//! facts the daemon collected, and finds each exit: a write outside the write roots,
//! a privilege, persistence or upload program, the network, a bus, a device, a
//! destructive idiom. It reads every line, also one that the engine's strict split
//! cannot read, through a lenient split that finds a program too many rather than too
//! few: text can only add a question, and the sandbox holds what it misses. It also
//! turns the model's `needs` into exits.
//!
//! [`views`] gives the views of the host that a line gets with no exit: the network
//! view for a line that only reads the host's network, and the filtered bus for a line
//! with a read form of a systemd tool (`view.rs`).
//!
//! [`unsandboxed_line_problem`] is the one-command rule: a line that runs outside the
//! sandbox must be one command plus read-only helpers, or a `sudo -k` that only forgets
//! the cached password.
//!
//! [`critical_deletes`] is the guard of full mode (`critical.rs`): the deletes of a line
//! of a tree that only the user may risk, such as `~`, `/`, a system dir or a project.
//! `auto` never reads it; it is no exit.

mod cover;
mod critical;
mod envelope;
mod git_code;
mod names;
mod needs;
mod persist;
mod programs;
mod remotes;
mod scan;
mod view;

use std::fmt;
use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, ExitKind, ExitSource, Grant, Needs};

pub(crate) use self::cover::set_copies;
pub use self::critical::critical_deletes;
pub(crate) use self::envelope::Envelope;
use self::envelope::{WriteExit, read_exit, resolve};
#[cfg(test)]
pub(crate) use self::names::floor_names;
pub use self::names::{GENERIC_STEMS, NAME_PHRASES, names_match};
pub(crate) use self::needs::{is_credential_agent, named_desktop};
pub use self::programs::DESKTOP_IPC_READ;
use self::remotes::RemoteUse;
use self::scan::{OPAQUE, Segment, is_assignment};
use crate::command;
use crate::network::{HostList, Preset};
use crate::path_class::normalize;
use crate::policy::read_only_commands;
use crate::{
    Access, AutoSupport, CallFacts, CoverAsk, Egress, GitCodeAsk, Locations, PathAccess, PathClass,
    Requirements, Source, TargetKind,
};

/// Programs that only the `auto` mode treats as a `privilege` exit, besides the ones
/// that always run as another user (`sudo`, `doas` and the rest of the engine's
/// privileged list). They are not on that list, because it holds the programs that no
/// rule may allow in any mode, and a user's `docker ps` rule must keep working in
/// `cautious`.
pub const PRIVILEGE_EXITS: &[&str] =
    &["yay", "paru", "pikaur", "aura", "docker", "podman", "machinectl", "nsenter"];

/// A package ecosystem whose manager fetches from its own registries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Ecosystem {
    /// Cargo and crates.io.
    Crates,
    /// npm, pnpm, yarn and bun with the npm registry.
    Npm,
    /// pip and uv with PyPI.
    Pypi,
    /// The Go module proxy.
    Go,
    /// pacman with the mirrors of `/etc/pacman.d/mirrorlist`.
    Pacman,
}

/// The registry hosts of each ecosystem: a view of the network presets of spec 8.3
/// ([`Preset::hosts`]), so the presets are the one source of these hosts. The pacman
/// mirrors depend on the machine, so this table has none, and the `Pacman` exception
/// has no hosts: efrd reads the mirror list ([`crate::network::parse_pacman`]) into
/// the proxy's host list for `@pacman-mirrors`, so under the proxy a listed mirror
/// needs no review at all, and any other mirror host still needs a user message that
/// names it. This is stricter than DP4, which gave the exception the mirror hosts.
///
/// The classifier's naming rule lets a `host` exit of a package manager's fetch reach
/// these hosts without a user message that names them, when a user message asks to
/// build, install, update or fetch (spec 9.4, a proposal behind this table). An empty
/// table turns the exception off.
pub const REGISTRY_HOSTS: &[(Ecosystem, &[&str])] = &[
    (Ecosystem::Crates, Preset::Crates.hosts()),
    (Ecosystem::Npm, Preset::Npm.hosts()),
    (Ecosystem::Pypi, Preset::Pypi.hosts()),
    (Ecosystem::Go, Preset::Go.hosts()),
];

/// What the model reads, and the user never sees, when a line that would run outside
/// the sandbox holds more than one command.
pub const ONE_COMMAND: &str = "an approved command outside the sandbox must run alone; run \
                               the other parts in a separate call";

/// Who may approve an exit in `auto` (spec 9.1).
///
/// The engine never reads user text, so it cannot finish the route alone: the turn
/// resolves [`Approver::ClassifierWhenNamed`] with [`names_match`] and the user's
/// messages. The enum is deliberately exhaustive: the turn must route every approver.
/// It is ordered from the freest to the strictest, so the approver of a call is the
/// maximum of its exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Approver {
    /// The classifier judges it first, and may hand it to the user.
    Classifier,
    /// The classifier judges it only when a user message names one of
    /// [`ExitNeed::names`]; else only the user may approve it.
    ClassifierWhenNamed,
    /// Only the user may approve it, whatever the messages say.
    User,
}

/// One exit of a call: what it needs, what a "yes" opens, and who may approve it.
#[derive(Clone, PartialEq, Eq)]
pub struct ExitNeed {
    /// The kind.
    pub kind: ExitKind,
    /// Exactly what a "yes" opens for one call. Empty for a kind that runs in the exit
    /// child, for `destructive` (the same sandbox runs it), for a floor kind, for a
    /// desktop program that talks to no desktop socket (`ydotool`), and for a file tool,
    /// which efrd runs itself. A `desktop_ipc` grant names the desktop; efrd finds its
    /// one control socket for the terminal of the prompt.
    pub grants: Vec<Grant>,
    /// The simple command that needs it, its words joined by spaces, or the whole line
    /// when no one command does.
    pub part: String,
    /// Who may approve it. [`ExitNeed`] starts with the default of its kind:
    /// [`Approver::User`] for a kind that [`ExitKind::user_only`] names,
    /// [`Approver::Classifier`] for the rest. Only the places that spec 9.1 names
    /// change it.
    pub approver: Approver,
    /// The forms that name this exit in a user message, for [`names_match`]: the path
    /// forms of a target, the words of a file or a unit, the verb, the remote or the
    /// host of an upload. A `User` exit may have them too; they then fill the record
    /// only.
    pub names: Vec<String>,
    /// True when efr copies the file into the home store before the write (phase B):
    /// for every write of a home path that is no secret and no part of efr's config
    /// ([`copies`](Self::copies) is not empty), while [`AutoSupport::floor_backup`] is
    /// true, whoever approves it.
    pub backup_before_write: bool,
    /// The hosts that the exit reaches, in lower case: for a `host` or `upload` exit the
    /// host of each URL of its command, of each remote operand of `scp`, `sftp` or
    /// `rsync`, the destination of `ssh`, and the hosts behind a git remote name when
    /// efrd's facts give them (phase D2); for an `outside` exit of `ssh`, `mosh`,
    /// `autossh` or an ssh git URL or remote under the proxy, that host. The grant can
    /// open more than these, such as the whole network, and a host that the line hides
    /// (a package manager's registry, a remote with no fact) is not here. The naming
    /// rule of the classifier's allow checks each one (spec 9.4).
    pub hosts: Vec<String>,
    /// The package ecosystem whose registry a `host` exit fetches from, when the line
    /// is a package manager's fetch such as `cargo fetch` or `npm ci`. The classifier's
    /// naming rule may then pass the hosts of [`REGISTRY_HOSTS`] without a name (spec
    /// 9.4).
    pub package_fetch: Option<Ecosystem>,
    /// Whether efr predicted it, the model asked for it, or a delete in a cache needs
    /// it.
    pub source: ExitSource,
    /// The path it is about, for a write, a read, a device or a socket.
    pub target: Option<PathBuf>,
    /// How a contained call writes [`target`](Self::target), for a `write` exit of a
    /// shell call.
    pub bind: Option<WriteBind>,
    /// The same file below a dotfile link, for a floor edit whose target is the real
    /// file behind the link, such as `~/.config/hypr/x.conf` for
    /// `~/dotfiles/hypr/x.conf`: the path that the user knows. efrd names the backup
    /// by it. `None` when no link of a floor leads to the target.
    pub link: Option<PathBuf>,
    /// What efrd copies into its home store before the call, so that `efr undo` can
    /// bring it back (phase B, efr's auto spec 10.8): [`CoverAsk::Home`] for a file or a
    /// new file, [`CoverAsk::HomeDir`] for a directory, for an `above_root` write and
    /// for a dir that a contained call gets whole ([`WriteBind::Target`],
    /// [`WriteBind::MakeDir`], and the parent of [`WriteBind::Parent`] next to the
    /// target). Empty when it copies nothing.
    pub copies: Vec<CoverAsk>,
    /// Whether every ask of [`copies`](Self::copies) is covered, for the exit record:
    /// `Some(false)` when a fact says that one is not, such as a masked path or a dir
    /// over the limits, or when no fact says. A write that is not covered still runs
    /// when its approver says yes. `None` when the exit has nothing to copy.
    pub covered: Option<bool>,
    /// What the persistence form installs, which the record holds so the classifier
    /// reads it (phase B, plan PB19a): the unit file of `systemctl --user enable`, the
    /// source of a link into a unit dir or of a copy into a floor, the file of
    /// `crontab <file>`. Empty for any other exit.
    pub installs: Vec<Installed>,
    /// efr's own notes for the question about this exit, one short sentence each, such
    /// as the file of the git code that a sandboxed command can write, or the project
    /// code that a publish runs (v0.0.4, B4). Empty for most exits.
    pub notes: Vec<String>,
}

/// One file that a persistence form installs, as the classifier reads it.
#[derive(Clone, PartialEq, Eq)]
pub struct Installed {
    /// The real file.
    pub path: PathBuf,
    /// Its text, at most 8 KiB.
    pub text: String,
}

/// The text can hold anything that the file holds, so `Debug` shows its length.
impl fmt::Debug for Installed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Installed")
            .field("path", &self.path)
            .field("text", &format_args!("<{} bytes>", self.text.len()))
            .finish()
    }
}

impl ExitNeed {
    /// An exit of `kind` for `part`, with the default approver of its kind and nothing
    /// else.
    pub(crate) fn new(kind: ExitKind, part: impl Into<String>, source: ExitSource) -> Self {
        ExitNeed {
            kind,
            grants: Vec::new(),
            part: part.into(),
            approver: if kind.user_only() { Approver::User } else { Approver::Classifier },
            names: Vec::new(),
            hosts: Vec::new(),
            backup_before_write: false,
            package_fetch: None,
            source,
            target: None,
            bind: None,
            link: None,
            copies: Vec::new(),
            covered: None,
            installs: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// True when the classifier may not judge it on its own: the approver is
    /// [`Approver::User`], or [`Approver::ClassifierWhenNamed`] before the turn finds a
    /// user message that names it.
    pub fn user_only(&self) -> bool {
        self.approver != Approver::Classifier
    }

    /// True when the call runs in the exit child after a "yes": a kind that runs
    /// unsandboxed, or a `write` that no contained bind serves.
    pub fn runs_unsandboxed(&self) -> bool {
        self.kind.runs_unsandboxed() || self.bind == Some(WriteBind::ExitChild)
    }
}

/// `part` can hold a token, such as a header of `curl`, so `Debug` shows its length,
/// and only the count of the names that come from it.
impl fmt::Debug for ExitNeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExitNeed")
            .field("kind", &self.kind)
            .field("grants", &self.grants)
            .field("part", &format_args!("<{} bytes>", self.part.len()))
            .field("approver", &self.approver)
            .field("names", &format_args!("<{} names>", self.names.len()))
            .field("hosts", &self.hosts)
            .field("backup_before_write", &self.backup_before_write)
            .field("package_fetch", &self.package_fetch)
            .field("source", &self.source)
            .field("target", &self.target)
            .field("bind", &self.bind)
            .field("link", &self.link)
            .field("copies", &self.copies)
            .field("covered", &self.covered)
            .field("installs", &self.installs)
            .field("notes", &self.notes)
            .finish()
    }
}

/// The kind and what it is about, such as `write /home/u/notes.txt` or `host`, without
/// the words of the line.
impl fmt::Display for ExitNeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind.as_str())?;
        if let Some(target) = &self.target {
            return write!(f, " {}", target.display());
        }
        match self.grants.first() {
            Some(Grant::Host { host, port }) => write!(f, " {host}:{port}"),
            Some(Grant::Bus { bus }) => write!(f, " ({bus:?} bus)"),
            Some(Grant::Desktop { desktop }) => write!(f, " ({} socket)", desktop.name()),
            _ => Ok(()),
        }
    }
}

/// Where a path stands for a call of the `auto` mode, for the facts of an exit record
/// and the question that the user reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathFacts {
    /// The path's class.
    pub class: PathClass,
    /// True when the path is a write root of the call or lies below one: the turn's
    /// project, `$SCRATCH` or an envelope root. A program there may have been written
    /// in the sandbox.
    pub in_write_root: bool,
    /// True when the path runs code later outside the sandbox and stays read-only in
    /// it.
    pub floor: bool,
    /// True when the path lies in a folder that a sync service copies off the machine.
    pub synced: bool,
    /// True when a contained call reads the path as empty: a sandbox mask, such as a
    /// credential store, or a project `.env` file.
    pub masked: bool,
    /// What can make the path again, when something can: a tool cache, the build
    /// output of a registered project, or efr's trash. The classifier's naming rule
    /// needs no user message that names such a target (spec 9.4). `None` for a floor, a
    /// mask and a secret, whatever their place.
    pub rebuild: Option<Rebuild>,
}

/// What can make a path again, so that a write there needs no user message that names
/// it (spec 9.4). efr decides it from its own roots and the settings, never from the
/// line or the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rebuild {
    /// A tool cache that a contained call writes through an overlay, such as
    /// `~/.cache` or `~/.cargo`: the tool fetches or builds it again.
    Cache,
    /// A dir with a `sandbox.rebuildable` name, such as `target` or `node_modules`,
    /// below a registered project root: the build makes it again. The same name
    /// elsewhere, such as `~/Documents/build`, is the user's data.
    BuildOutput,
    /// efr's trash below its data dir.
    Trash,
}

/// How a contained call gets the right to write the target of a `write` exit (spec
/// 7.5). Every mask and floor stays on top of the bind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WriteBind {
    /// The target, an existing directory, is bound.
    Target,
    /// The nearest existing parent of the target is bound, so a program may write a
    /// temporary file there and rename it over the target.
    Parent(PathBuf),
    /// The target, an existing file in a shared directory such as `~` or `~/.config`,
    /// is bound alone: writes in place work, a rename over it fails.
    TargetOnly,
    /// efrd makes the target as an empty file first, then binds it: the line's program
    /// makes exactly that file.
    MakeFile,
    /// efrd makes the target as an empty directory first, then binds it.
    MakeDir,
    /// No contained bind serves the target: a system or masked path, or a new target
    /// in a shared directory that a program other than its maker writes. The line runs
    /// in the exit child, and only the user may approve it.
    ExitChild,
}

/// Everything [`predict`] reads for one shell call.
#[derive(Debug, Clone, Copy)]
pub struct ExitInput<'a> {
    /// The command line, as the model wrote it.
    pub line: &'a str,
    /// The directory the line starts in, when it is known.
    pub command_dir: Option<&'a Path>,
    /// The paths the tool declared.
    pub paths: &'a [PathAccess],
    /// True when the tool declared network access.
    pub network: bool,
    /// What the model asked for.
    pub needs: Option<&'a Needs>,
    /// The facts the daemon collected; `None` makes every fact missing.
    pub facts: Option<&'a CallFacts>,
    /// The machine's locations, with the envelope roots.
    pub locations: &'a Locations,
    /// The root of the turn's project, when it may widen what a call does.
    pub project_root: Option<&'a Path>,
    /// The conversation's `$SCRATCH`, when it counts as scratch.
    pub scratch: Option<&'a Path>,
    /// What the machine's sandbox supports.
    pub support: &'a AutoSupport,
    /// The host list of efr's proxy, which the prediction reads under
    /// [`Egress::Proxy`] (phase D2).
    pub hosts: &'a HostList,
}

/// The exits of one shell call: the ones efr finds in the line, its declared paths and
/// its network need, then the ones the model asked for in `needs`, each once.
///
/// A missing fact counts as the stricter case: a target exists, and a directory holds
/// tracked files.
pub fn predict(input: &ExitInput<'_>) -> Vec<ExitNeed> {
    let envelope = Envelope::new(input.locations, input.project_root, input.scratch);
    let start = input.command_dir.and_then(normalize);
    let segments = scan::scan(input.line);
    // NOTE: a line of the network view rule runs in the host's network namespace with
    // `Grant::NetView` ([`views`]), so its `ss` and `ip` need no `host_view` exit.
    let net_view = view::net_view(&segments, input.support);
    let rustup_env = segments.iter().any(|segment| programs::sets_rustup_env(&segment.words));
    // NOTE: a remote shell, an ssh git URL or remote, and a git push run outside the
    // sandbox with the user's ssh agent and keys; a variable of the line, such as
    // `GIT_SSH_COMMAND` or `SSH_ASKPASS`, would choose the command that they run there.
    let ssh_env = segments.iter().any(|segment| programs::sets_ssh_env(&segment.words));
    let mut found = Found::default();
    let mut made: Vec<(PathBuf, bool)> = Vec::new();
    let mut writes: Vec<(PathBuf, String)> = Vec::new();
    let mut fetch: Option<Ecosystem> = None;
    let mut linked: Vec<PathBuf> = Vec::new();
    let mut installs: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    let mut git_triggers: Vec<git_code::Trigger> = Vec::new();
    let mut git_steered = false;
    for segment in &segments {
        let dir = if segment.after_cd { None } else { start.as_deref() };
        let at = |word: &str| resolve(word, dir, input.locations);
        let part = part_text(segment);
        let assigned = segment.words.iter().any(|word| is_assignment(word));
        for redirect in &segment.redirects {
            let Some(path) = at(&redirect.target) else { continue };
            // NOTE: a redirection to a node of the sandbox's own `/dev`, such as
            // `2>/dev/null`, writes no file; one to a host device needs its grant. A
            // path through a node, such as `/dev/fd/3/x`, is a write that efr cannot
            // place, which the exit child runs.
            if programs::is_sandbox_device(&path) {
                continue;
            }
            if programs::is_host_device(&path) {
                found.push(predicted(programs::device_exit(path.clone()), &part));
            }
            let bare = match segment.words.as_slice() {
                [] => true,
                [only] => only == ":" || only == "true",
                _ => false,
            };
            if bare && !redirect.append && exists(input.facts, &path) {
                let targets = [Some(path.clone())];
                let asks = cover::path_asks(&targets);
                found.extend_from(destructive(&envelope, &part, &targets, &asks, input));
            }
            made.push((path.clone(), false));
            writes.push((path, part.clone()));
        }
        for (command, by_shell) in programs::placed_commands(&segment.words) {
            fetch = fetch.or_else(|| programs::package_fetch(command));
            git_triggers.extend(git_code::trigger(command, dir, input.locations));
            git_steered |= git_code::steers_code(command);
            let mount_view =
                input.support.mount_view && programs::runs_mount_view(command, by_shell);
            let rustup = command.first().is_some_and(|word| programs::base_name(word) == "rustup");
            // NOTE: the hosts behind the remote names of a git command (DP22); `None`
            // when no fact describes them, which keeps the line's own hosts only.
            let remote = remotes::remote_command(command)
                .filter(|_| !remotes::steers_git(&segment.words))
                .and_then(|remote| {
                    let git_dir = remotes::remote_dir(command, dir, input.locations)?;
                    let chosen = remotes::selected(&remote, &git_dir, input.facts)?;
                    Some((remote.kind, chosen))
                });
            let remote_hosts = |kind: ExitKind| -> Vec<String> {
                match &remote {
                    Some((RemoteUse::Push, chosen)) if kind == ExitKind::Upload => {
                        remotes::hosts(&remotes::urls(RemoteUse::Push, chosen))
                    }
                    Some((use_kind, chosen)) if kind == ExitKind::Host => {
                        remotes::hosts(&remotes::urls(*use_kind, chosen))
                    }
                    _ => Vec::new(),
                }
            };
            for mut exit in programs::classify(command, input.support, input.hosts) {
                if exit.kind == ExitKind::Outside && rustup && rustup_env {
                    exit.approver = Approver::User;
                }
                if exit.kind == ExitKind::Outside && !exit.hosts.is_empty() && ssh_env {
                    exit.approver = Approver::User;
                }
                // NOTE: a read form of a systemd tool reads the filtered bus of its view
                // (phase E, [`views`]); every other bus form keeps its exit.
                if exit.bus_read && input.support.bus_proxy {
                    continue;
                }
                if exit.kind == ExitKind::Outside && mount_view {
                    continue;
                }
                if exit.kind == ExitKind::HostView && net_view {
                    continue;
                }
                if matches!(exit.kind, ExitKind::Host | ExitKind::Upload) {
                    let mut need = predicted(exit, &part);
                    for host in programs::line_hosts(command) {
                        if !need.hosts.contains(&host) {
                            need.hosts.push(host);
                        }
                    }
                    for host in remote_hosts(need.kind) {
                        if !need.hosts.contains(&host) {
                            need.hosts.push(host);
                        }
                    }
                    // NOTE: a git push whose hosts efr cannot tell, because its remote
                    // facts were dropped (`--git-dir`, `-c`, a `GIT_*` variable, a
                    // `cd` first) or name no host (a local path, a transport that efr
                    // does not read), would pass the naming rule with nothing to check;
                    // and a variable of the line can choose what it runs. With no facts
                    // at all (no sandbox in efrd) the rule of phase A stays.
                    if need.kind == ExitKind::Upload
                        && let Some(rest) = remotes::git_push(command)
                    {
                        if let Some(host) = programs::scp_url_host(rest)
                            && !need.hosts.contains(&host)
                        {
                            need.hosts.push(host);
                        }
                        if (need.hosts.is_empty() && input.facts.is_some()) || ssh_env {
                            need.approver = Approver::User;
                        }
                    }
                    found.push(need);
                    continue;
                }
                if exit.kind == ExitKind::Destructive {
                    let targets: Vec<Option<PathBuf>> = match programs::destructive_targets(command)
                    {
                        Some(words) => words.into_iter().map(at).collect(),
                        None => vec![git_dir(command, dir, input.locations)],
                    };
                    let asks = cover::program_asks(command, assigned, dir, input.locations, &at);
                    found.extend_from(destructive(&envelope, &part, &targets, &asks, input));
                    continue;
                }
                let mut need = predicted(exit, &part);
                if need.kind == ExitKind::Persistence {
                    let facts = input.facts;
                    persist::route(
                        &mut need,
                        command,
                        input.line,
                        &envelope,
                        facts,
                        input.support,
                        &at,
                    );
                }
                found.push(need);
            }
            if let Some((kind, chosen)) = &remote
                && *kind != RemoteUse::Push
                && input.support.egress == Egress::Proxy
            {
                let mut exit = remotes::proxy_fetch_exit(*kind, chosen, input.hosts, &part);
                if let Some(need) = exit.as_mut().filter(|need| need.kind == ExitKind::Outside)
                    && ssh_env
                {
                    need.approver = Approver::User;
                }
                found.extend_from(exit);
            }
            installs.extend(persist::installs(command, &at));
            for (word, dir_kind) in programs::made_paths(command) {
                if let Some(path) = at(word) {
                    made.push((path, dir_kind));
                }
            }
            linked.extend(programs::link_paths(command).into_iter().filter_map(at));
            for word in programs::dd_targets(command)
                .into_iter()
                .chain(programs::sed_in_place_targets(command))
            {
                if let Some(path) = at(word) {
                    writes.push((path, part.clone()));
                }
            }
            let operands = programs::recursive_rm_operands(command);
            let tracked = operands.iter().any(|word| {
                at(word).and_then(|path| input.facts?.tracked(&path)).is_none_or(|count| count > 0)
            });
            if tracked {
                let targets: Vec<Option<PathBuf>> = operands.into_iter().map(at).collect();
                let asks = cover::path_asks(&targets);
                found.extend_from(destructive(&envelope, &part, &targets, &asks, input));
            }
        }
    }
    for PathAccess { path, access } in input.paths {
        let Some(path) = normalize(path).map(|path| input.locations.rehome(&path).into_owned())
        else {
            continue;
        };
        match access {
            Access::Write => {
                let part = naming_part(&segments, &path, start.as_deref(), input.locations)
                    .unwrap_or_else(|| input.line.to_owned());
                writes.push((path, part));
            }
            Access::Read | Access::ReadTree => {
                if let Some(need) = read_exit(&envelope, &path, *access, ExitSource::Predicted) {
                    found.push(need);
                }
            }
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    let cx = WriteFacts {
        facts: input.facts,
        support: input.support,
        linked: &linked,
        installs: &installs,
    };
    for (path, part) in writes {
        if seen.contains(&path) {
            continue;
        }
        let exit = envelope.write_exit(&path, input.facts, &made);
        if let Some(need) = write_need(&envelope, &cx, exit, &path, &part, ExitSource::Predicted) {
            found.push(need);
        }
        seen.push(path);
    }
    let networked = found.0.iter().any(|need| {
        matches!(
            need.kind,
            ExitKind::Host
                | ExitKind::HostView
                | ExitKind::Upload
                | ExitKind::Privilege
                | ExitKind::Outside
        )
    });
    if input.network && !networked && input.support.egress == Egress::None {
        let mut need = ExitNeed::new(ExitKind::Host, input.line, ExitSource::Predicted);
        need.grants = vec![Grant::OpenNetwork];
        for segment in &segments {
            for command in programs::commands(&segment.words) {
                for host in programs::line_hosts(command) {
                    if !need.hosts.contains(&host) {
                        need.hosts.push(host);
                    }
                }
            }
        }
        found.push(need);
    }
    if let Some(asked) = input.needs {
        let dir = start.as_deref();
        let cx = needs::NeedsContext { support: input.support, hosts: input.hosts };
        for need in needs::exits(asked, &envelope, dir, input.facts, &made, &cx) {
            found.push(need);
        }
    }
    let mut found = found.0;
    for need in &mut found {
        match need.kind {
            // NOTE: the stricter of the form's approver and the rule of the program
            // words, so a `rustup` form that must be named stays so.
            ExitKind::Outside => {
                need.approver = need.approver.max(outside_approver(input, &envelope));
            }
            ExitKind::Host => need.package_fetch = fetch,
            ExitKind::Bus if need.grants.contains(&WHOLE_SESSION_BUS) => {
                whole_session_bus(need);
            }
            _ => {}
        }
    }
    // NOTE: after the rules above, so that nothing lowers the approver again.
    git_code::apply(&mut found, &git_triggers, input.facts, input.locations.home());
    if git_steered {
        git_code::apply_steering(&mut found, git_code::STEERING_NOTE);
    }
    // NOTE: a variable of the line, such as `GIT_CONFIG_COUNT` or `HOME`, chooses the
    // config and the programs that git runs, also the git of a gh line; the git code
    // facts come from the hidden shell's environment, not the line's.
    if !git_triggers.is_empty()
        && segments.iter().any(|segment| git_code::sets_git_env(&segment.words))
    {
        git_code::apply_steering(&mut found, git_code::ENV_STEERING_NOTE);
    }
    named_targets(found, input.locations.home())
}

/// The notes for the model about the entries of the call's `needs` that [`predict`]
/// dropped: a host that is no host name in ASCII, and under efr's proxy a host that the
/// allow list holds or the user's config denies (spec 7.3, DP21).
pub fn notes(input: &ExitInput<'_>) -> Vec<String> {
    let cx = needs::NeedsContext { support: input.support, hosts: input.hosts };
    input.needs.map(|asked| needs::host_notes(asked, &cx)).unwrap_or_default()
}

/// The views of the host that the shell call of `input` gets with no exit, no
/// classifier and no question.
///
/// - `Grant::NetView` (phase D1) for a line that only reads the host's network, such as
///   `ss -tln | grep 8080` or `cat /proc/net/dev`, on a machine that runs NetView calls
///   ([`AutoSupport::netview`]). [`predict`] then finds no `host_view` exit for it. The
///   launch drops the view beside any grant other than `write` and `unmask`, and the
///   NetView call's seccomp profile and Landlock rights hold whatever code it runs.
/// - `Grant::BusFilter` of a bus (phase E) for a line with at least one simple command,
///   in any place of the line, that is a read form of a systemd tool on that bus, such
///   as `systemctl --failed` or `loginctl list-sessions`, on a machine that runs the
///   filtered bus proxy ([`AutoSupport::bus_proxy`]). [`predict`] then finds no `bus`
///   exit for that command; every other bus form of the line keeps its exit. The
///   proxy's rules hold whatever the line runs: the rule is a fit, not a security layer.
///
/// The bus views come after the network view, and the launch never joins the two.
pub fn views(input: &ExitInput<'_>) -> Vec<Grant> {
    let segments = scan::scan(input.line);
    let mut grants = view::grants(&segments, input.support);
    grants.extend(view::bus_grants(&segments, input.support));
    grants
}

/// The grant of the whole session bus, which a `bus` exit of a session bus program or
/// of `needs.bus = "session"` asks for.
const WHOLE_SESSION_BUS: Grant = Grant::Bus { bus: BusKind::Session };

/// The note of a `bus` exit for the whole session bus.
pub(crate) const WHOLE_SESSION_BUS_NOTE: &str =
    "the whole session bus can start programs outside the sandbox";

/// A `bus` exit for the whole session bus is the user's own question, never the
/// reviewer's (v0.0.4, DV18): the user manager on that bus starts any program outside
/// the sandbox (`StartTransientUnit`), and an unlocked keyring there (the Secret
/// Service) hands out every secret with no prompt. efr does not look for a keyring
/// first, because one that starts on demand has no owner yet. The system bus keeps its
/// approver.
fn whole_session_bus(need: &mut ExitNeed) {
    need.approver = Approver::User;
    if !need.notes.iter().any(|note| note == WHOLE_SESSION_BUS_NOTE) {
        need.notes.push(WHOLE_SESSION_BUS_NOTE.to_owned());
    }
}

/// Who approves an `outside` exit of `input` (spec 9.1): the user when a program word of
/// the line, or a program that a wrapper runs, changed in this turn or lies in a real
/// write root, so that code the sandbox wrote never gets the user's full rights; the
/// user too when a fact about a program word is missing. The user also when the line
/// runs an interpreter on code that efr cannot read (`python3 -c`, `sh -c`, a script):
/// such code can edit a floor file or read a masked file with no name, no backup and no
/// exit of its own. Else the classifier. A cache overlay and the private tmp are no
/// real write root: the sandbox never writes the real files there.
fn outside_approver(input: &ExitInput<'_>, envelope: &Envelope<'_>) -> Approver {
    let Some(facts) = input.facts else {
        return Approver::User;
    };
    let unread = scan::scan(input.line).iter().any(|segment| {
        programs::commands(&segment.words).into_iter().any(programs::runs_unread_code)
    });
    if unread {
        return Approver::User;
    }
    let request = fact_requests(input.line, input.command_dir, input.locations);
    let trusted =
        request.programs.iter().filter(|word| !request.builtins.contains(word)).all(|word| {
            match facts.programs.iter().find(|(known, _, _)| known == word) {
                None | Some((_, _, true)) => false,
                Some((_, Some(path), false)) => {
                    normalize(path).is_some_and(|path| !envelope.in_real_root(&path))
                }
                Some((_, None, false)) => true,
            }
        });
    if trusted { Approver::Classifier } else { Approver::User }
}

/// The `destructive` exit of the simple command `part`, whose paths are `targets` (spec
/// 9.1): the classifier judges it only when efr knows every path and each lies in a
/// real write root; else only the user approves it.
///
/// `None` when the turn's snapshot covers every ask of `asks` and the machine has undo
/// with `sandbox.ask_destructive = "unless_undoable"` ([`AutoSupport::undo`]): `efr undo`
/// brings back what the part destroys, so it runs with no review (spec 10.7). Only this
/// exit goes; every other exit of the line keeps its route.
fn destructive(
    envelope: &Envelope<'_>,
    part: &str,
    targets: &[Option<PathBuf>],
    asks: &[CoverAsk],
    input: &ExitInput<'_>,
) -> Option<ExitNeed> {
    if cover::covers_all(asks, envelope, input.facts, input.support) {
        return None;
    }
    let mut need = ExitNeed::new(ExitKind::Destructive, part, ExitSource::Predicted);
    let known = !targets.is_empty()
        && targets
            .iter()
            .all(|target| target.as_ref().is_some_and(|path| envelope.in_real_root(path)));
    need.approver = if known { Approver::Classifier } else { Approver::User };
    Some(need)
}

/// The repository that a `git` command of `words` works on: the directory of its `-C`
/// options before the subcommand, each relative to the one before as git takes them, or
/// the directory the command runs in. `None` when a `-C` or `dir` is not known.
fn git_dir(words: &[String], dir: Option<&Path>, locations: &Locations) -> Option<PathBuf> {
    let args = words.get(1..).unwrap_or_default();
    let mut cwd = dir.map(Path::to_path_buf);
    for global in command::git::exact(args).globals.iter().filter(|global| global.is("-C")) {
        // NOTE: git ignores an empty `-C`.
        match global.value(args) {
            Some("") => {}
            Some(value) => cwd = Some(resolve(value, cwd.as_deref(), locations)?),
            None => return None,
        }
    }
    cwd
}

/// `needs` with the forms of each target in [`ExitNeed::names`], where nothing named
/// it yet: the path forms of every target, and the words of a file that runs code later.
pub(crate) fn named_targets(mut needs: Vec<ExitNeed>, home: &Path) -> Vec<ExitNeed> {
    for need in &mut needs {
        let Some(target) = &need.target else { continue };
        if !need.names.is_empty() {
            continue;
        }
        let mut forms = names::path_forms(target, home);
        if need.kind == ExitKind::Persistence {
            forms.extend(names::word_forms(target));
        }
        need.names = names::dedup(forms);
    }
    needs
}

/// `needs` where the target of each need is the real file behind a declared link
/// ([`CallFacts::links`]) with the names of the need of that link added: a file tool's
/// write of `~/dotfiles/zsh/.zshrc` behind `~/.zshrc` is named by "my zshrc", as the
/// write of the link is. The need of the link must be in the same call, so a name never
/// reaches a path that the call does not write through the link.
pub(crate) fn link_names(mut needs: Vec<ExitNeed>, facts: Option<&CallFacts>) -> Vec<ExitNeed> {
    let Some(facts) = facts.filter(|facts| !facts.links.is_empty()) else {
        return needs;
    };
    let mut added: Vec<(usize, Vec<String>)> = Vec::new();
    for (at, need) in needs.iter().enumerate() {
        let Some(target) = &need.target else { continue };
        for (link, _) in facts.links.iter().filter(|(_, real)| real == target) {
            let names = needs
                .iter()
                .filter(|other| other.target.as_ref() == Some(link))
                .flat_map(|other| other.names.iter().cloned());
            added.push((at, names.collect()));
        }
    }
    for (at, names) in added {
        let need = &mut needs[at];
        for name in names {
            if !need.names.contains(&name) {
                need.names.push(name);
            }
        }
    }
    needs
}

/// What the daemon looks up on disk before [`predict`] reads a line, because the
/// engine reads no file: the facts of [`CallFacts`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FactRequest {
    /// Paths whose kind a fact gives: the targets that the line writes or makes
    /// (redirections, `dd of=`, `sed -i`, `tee`, `touch`, `mkdir`), each operand of
    /// `rm -r`, and the paths of `needs.write` ([`FactRequest::with_needs`]). The daemon
    /// adds the declared write paths and the parents of each target up to the first one
    /// that exists.
    pub targets: Vec<PathBuf>,
    /// The directories of `rm -r` whose tracked files a fact counts.
    pub tracked: Vec<PathBuf>,
    /// The program words of the line, each once, also behind wrappers such as `env`
    /// and `sudo`; a word that only the shell can read is left out.
    pub programs: Vec<String>,
    /// The words of [`programs`](Self::programs) that the shell runs itself in every
    /// place of the line: a builtin or a reserved word of zsh, such as `:`, `cd` or
    /// `printf`, where the shell finds the program. Such a word leads to no file, even
    /// when the `PATH` has one of its name. Behind `env`, `sudo` or `find -exec` only a
    /// file runs, so a word there is not here.
    pub builtins: Vec<String>,
    /// What the daemon must say efr holds before the call, so that `efr undo` can
    /// bring it back (phase B, efr's auto spec 10.7 and 10.8). The answers go to
    /// [`CallFacts::covered`]; an ask with no answer is never covered.
    ///
    /// For a destructive part of the line: a [`CoverAsk::Path`] for each target
    /// (`rm -r`, `dd of=`, `truncate`, `: >`, `find -delete`), or the tracked files,
    /// untracked files, refs or stash of the repository of a `git` command, whose
    /// directory the ask names; the daemon finds the root of its work tree. A form that
    /// undo never brings back (`shred`, `git clean -x`, `git reflog expire`,
    /// `git gc --prune`) asks nothing. For each path that the line writes: the
    /// [`CoverAsk::Home`] and [`CoverAsk::HomeDir`] asks of the home store, also for
    /// its parent dir ([`FactRequest::with_writes`] adds the declared write paths). The
    /// engine reads the `Home` and `HomeDir` asks only for a write outside the snapshot
    /// roots, so the daemon may answer one inside a root with no work. For a named
    /// persistence form (plan PB19, PB20): a [`CoverAsk::UnitLinks`] for each unit of
    /// `systemctl --user enable` and the like, [`CoverAsk::CronTable`] for a `crontab`
    /// form that replaces or removes the crontab, [`CoverAsk::Linger`] for `loginctl
    /// enable-linger` and `disable-linger`.
    pub covers: Vec<CoverAsk>,
    /// The files whose bytes a persistence form of the line installs (phase B, plan
    /// PB19a): the unit files of `systemctl --user enable` and the like, the file of
    /// `crontab <file>`, the sources of a `cp` or `ln` into a floor. The answers go to
    /// [`CallFacts::sources`]; the daemon never reads a mask or a secret for them.
    pub sources: Vec<Source>,
    /// The directories of git commands that reach a remote (`fetch`, `pull`,
    /// `ls-remote`, `remote update`, `push`, `submodule update`; phase D2, DP22). The
    /// answers go to [`CallFacts::remotes`]. A line that sets a `GIT_*` variable, and a
    /// git command with `-c`, `--git-dir` or `--work-tree`, asks for none: the facts of
    /// the directory would not be the ones that the command uses.
    pub remotes: Vec<PathBuf>,
    /// The paths whose real path, after every link, a fact gives (phase E): each path
    /// that the line writes below the home directory and each of its parents below the
    /// home directory (a write grant binds the target or its nearest existing parent),
    /// the same for each path of `needs.write` ([`FactRequest::with_needs`]) and each
    /// declared write path ([`FactRequest::with_writes`]), and each socket of
    /// `needs.sockets`. The daemon answers each path that it can resolve: for a path
    /// that does not exist, the real path of its nearest existing parent with the rest
    /// joined. The answers go to [`CallFacts::real_paths`]; an always-allow rule covers
    /// no grant whose path has no answer.
    pub real_paths: Vec<PathBuf>,
    /// The repositories whose git code efrd checks, because the line runs git there
    /// outside the sandbox when it gets an `outside` or `upload` exit: a git command
    /// that reaches a remote, or a gh command that runs git in the repository (v0.0.4,
    /// DV22). The answer goes to [`CallFacts::git_code`]; with no answer such an exit
    /// is one that only the user may approve.
    pub git_code: Vec<GitCodeAsk>,
}

/// The facts that [`predict`] reads for `line`, which starts in `command_dir`: the
/// same lenient split, so every path it asks about is here.
pub fn fact_requests(line: &str, command_dir: Option<&Path>, locations: &Locations) -> FactRequest {
    let start = command_dir.and_then(normalize);
    let mut request = FactRequest::default();
    // The program words in a place where only a file runs.
    let mut files: Vec<String> = Vec::new();
    let add = |list: &mut Vec<PathBuf>, path: PathBuf| {
        if !list.contains(&path) {
            list.push(path);
        }
    };
    let home = locations.home();
    // NOTE: the turn's project is not known here; the dir of the line stands in for
    // it, so a copy into an agent config there asks for its source too. A source too
    // many costs one small read.
    let envelope = Envelope::new(locations, start.as_deref(), None);
    let mut covers: Vec<CoverAsk> = Vec::new();
    for segment in scan::scan(line) {
        let dir = if segment.after_cd { None } else { start.as_deref() };
        let at = |word: &str| resolve(word, dir, locations);
        // NOTE: a node of the sandbox's own `/dev` is no exit, whatever the host has,
        // and a path through one leads where the daemon cannot look.
        let written_at = |word: &str| {
            at(word).filter(|path| {
                !programs::is_sandbox_device(path) && !programs::through_open_file(path)
            })
        };
        let assigned = segment.words.iter().any(|word| is_assignment(word));
        let bare = match segment.words.as_slice() {
            [] => true,
            [only] => only == ":" || only == "true",
            _ => false,
        };
        for redirect in &segment.redirects {
            if let Some(path) = written_at(&redirect.target) {
                if bare && !redirect.append {
                    covers.extend(cover::path_asks(&[Some(path.clone())]));
                }
                covers.extend(cover::write_asks(&path, home));
                add_real_paths(&mut request.real_paths, &path, home);
                add(&mut request.targets, path);
            }
        }
        for (command, by_shell) in programs::placed_commands(&segment.words) {
            let destroys =
                programs::classify(command, &AutoSupport::default(), &HostList::default())
                    .iter()
                    .any(|found| found.kind == ExitKind::Destructive);
            if destroys {
                covers.extend(cover::program_asks(command, assigned, dir, locations, &at));
            }
            covers.extend(persist::covers(command));
            if let Some(git_code::Trigger::Ask(ask)) = git_code::trigger(command, dir, locations)
                && !request.git_code.contains(&ask)
            {
                request.git_code.push(ask);
            }
            if remotes::remote_command(command).is_some()
                && !remotes::steers_git(&segment.words)
                && let Some(git_dir) = remotes::remote_dir(command, dir, locations)
            {
                add(&mut request.remotes, git_dir);
            }
            for source in persist::sources(command, &envelope, &at) {
                if !request.sources.contains(&source) {
                    request.sources.push(source);
                }
            }
            if let Some(program) = command.first()
                && !program.contains([OPAQUE, '$'])
            {
                if !request.programs.contains(program) {
                    request.programs.push(program.clone());
                }
                if by_shell && programs::is_shell_word(program) {
                    if !request.builtins.contains(program) {
                        request.builtins.push(program.clone());
                    }
                } else if !files.contains(program) {
                    files.push(program.clone());
                }
            }
            let written = programs::made_paths(command)
                .into_iter()
                .map(|(word, _)| word)
                .chain(programs::dd_targets(command))
                .chain(programs::sed_in_place_targets(command));
            for word in written {
                if let Some(path) = written_at(word) {
                    covers.extend(cover::write_asks(&path, home));
                    add_real_paths(&mut request.real_paths, &path, home);
                    add(&mut request.targets, path);
                }
            }
            let operands = programs::recursive_rm_operands(command);
            let removed: Vec<Option<PathBuf>> = operands.iter().map(|word| at(word)).collect();
            covers.extend(cover::path_asks(&removed));
            for path in removed.into_iter().flatten() {
                covers.extend(cover::write_asks(&path, home));
                add(&mut request.targets, path.clone());
                add(&mut request.tracked, path);
            }
        }
    }
    for ask in covers {
        if !request.covers.contains(&ask) {
            request.covers.push(ask);
        }
    }
    // NOTE: a word that runs as a file in one place of the line resolves as a file, so
    // the question shows that file and whether the sandbox wrote it.
    request.builtins.retain(|word| !files.contains(word));
    request
}

impl FactRequest {
    /// Adds the paths of `needs.write`, resolved as [`predict`] resolves them. Without
    /// a fact, a missing target counts as an existing file, and a grant that binds only
    /// that file cannot start.
    #[must_use]
    pub fn with_needs(
        mut self,
        needs: Option<&Needs>,
        command_dir: Option<&Path>,
        locations: &Locations,
    ) -> Self {
        let Some(needs) = needs else { return self };
        let start = command_dir.and_then(normalize);
        for word in needs.write.iter().take(Needs::MAX_WRITE) {
            if let Some(path) = resolve(word, start.as_deref(), locations) {
                self.add_covers(cover::write_asks(&path, locations.home()));
                add_real_paths(&mut self.real_paths, &path, locations.home());
                if !self.targets.contains(&path) {
                    self.targets.push(path);
                }
            }
        }
        for word in needs.sockets.iter().take(Needs::MAX_SOCKETS) {
            if let Some(path) = resolve(word, start.as_deref(), locations)
                && !self.real_paths.contains(&path)
            {
                self.real_paths.push(path);
            }
        }
        self
    }

    /// Adds the asks of the home store for each declared write path of a shell call
    /// (`writes`), as [`fact_requests`] adds them for the paths that the line names
    /// (phase B).
    #[must_use]
    pub fn with_writes(mut self, writes: &[PathBuf], locations: &Locations) -> Self {
        for path in writes {
            if let Some(path) = normalize(path) {
                let path = locations.rehome(&path).into_owned();
                self.add_covers(cover::write_asks(&path, locations.home()));
                add_real_paths(&mut self.real_paths, &path, locations.home());
            }
        }
        self
    }

    fn add_covers(&mut self, asks: Vec<CoverAsk>) {
        for ask in asks {
            if !self.covers.contains(&ask) {
                self.covers.push(ask);
            }
        }
    }
}

/// Adds `path` and each of its parents strictly below `home` to `list`, each once,
/// when `path` lies strictly below `home`: a contained write grant binds the target or
/// its nearest existing parent, which lies below the home directory (phase E).
fn add_real_paths(list: &mut Vec<PathBuf>, path: &Path, home: &Path) {
    for dir in path.ancestors().take_while(|dir| dir.starts_with(home) && *dir != home) {
        if !list.iter().any(|known| known == dir) {
            list.push(dir.to_path_buf());
        }
    }
}

/// The facts that the engine reads for a file tool call (`write_file`, `edit`,
/// `apply_patch`), which has no line, only its declared paths (phase B): for each path
/// that it writes, the [`CoverAsk::Home`] ask of the home store, and when it deletes or
/// moves a file ([`Requirements::destructive`](crate::Requirements)) also the
/// [`CoverAsk::Path`] ask of the snapshot. The engine reads the one that fits the place
/// of the path: the snapshot inside the turn's project and `$SCRATCH`, else the home
/// store. The daemon still adds the kinds of the paths and their parents to `targets`.
pub fn tool_fact_requests(requirements: &Requirements, locations: &Locations) -> FactRequest {
    let mut request = FactRequest::default();
    for PathAccess { path, access } in &requirements.paths {
        if *access != Access::Write {
            continue;
        }
        let Some(path) = normalize(path).map(|path| locations.rehome(&path).into_owned()) else {
            continue;
        };
        let mut asks = vec![CoverAsk::Home(path.clone())];
        if requirements.destructive {
            asks.push(CoverAsk::Path(path));
        }
        request.add_covers(asks);
    }
    request
}

/// The exits found so far, each kind, grant set and target once.
#[derive(Default)]
struct Found(Vec<ExitNeed>);

impl Found {
    /// Adds `need` as [`Found::push`] does, when there is one.
    fn extend_from(&mut self, need: Option<ExitNeed>) {
        if let Some(need) = need {
            self.push(need);
        }
    }

    /// Adds `need`, or merges it into a known exit of the same kind, grants and target:
    /// the stricter approver of the two wins, and the names and hosts of both stay.
    fn push(&mut self, need: ExitNeed) {
        let known = self.0.iter_mut().find(|known| {
            known.kind == need.kind && known.grants == need.grants && known.target == need.target
        });
        match known {
            Some(known) => {
                // NOTE: two named persistence forms of one line, such as a unit change and
                // a crontab, are one exit; a message that names one of them must not
                // send the other to the classifier.
                let both_named = known.approver == Approver::ClassifierWhenNamed
                    && need.approver == Approver::ClassifierWhenNamed;
                if known.kind == ExitKind::Persistence && both_named && known.names != need.names {
                    known.approver = Approver::User;
                }
                known.approver = known.approver.max(need.approver);
                for name in need.names {
                    if !known.names.contains(&name) {
                        known.names.push(name);
                    }
                }
                for host in need.hosts {
                    if !known.hosts.contains(&host) {
                        known.hosts.push(host);
                    }
                }
                // NOTE: the one exit copies what both forms copy, and it is covered only
                // when both are.
                for ask in need.copies {
                    if !known.copies.contains(&ask) {
                        known.copies.push(ask);
                    }
                }
                known.installs.extend(need.installs);
                for note in need.notes {
                    if !known.notes.contains(&note) {
                        known.notes.push(note);
                    }
                }
                known.backup_before_write |= need.backup_before_write;
                known.covered = match (known.covered, need.covered) {
                    (Some(a), Some(b)) => Some(a && b),
                    (a, b) => a.or(b),
                };
            }
            None => self.0.push(need),
        }
    }
}

/// The predicted exit of the simple command `part` that a program table found.
fn predicted(exit: programs::Found, part: &str) -> ExitNeed {
    let mut need = ExitNeed::new(exit.kind, part, ExitSource::Predicted);
    need.grants = exit.grants;
    need.target = exit.target;
    need.approver = exit.approver;
    need.names = exit.names;
    need.hosts = exit.hosts;
    need.notes = exit.notes;
    need
}

/// What the exit of a write needs beyond its path: the daemon's facts, the machine's
/// support, the paths that the line makes as symbolic links, and what a `cp` or `ln`
/// of the line puts where (destination, sources).
pub(super) struct WriteFacts<'a> {
    pub(super) facts: Option<&'a CallFacts>,
    pub(super) support: &'a AutoSupport,
    pub(super) linked: &'a [PathBuf],
    pub(super) installs: &'a [(PathBuf, Vec<PathBuf>)],
}

/// The `write` exit, or another kind, of a write of `path` that `exit` describes.
///
/// The classifier judges every `write`, also one that runs in the exit child (spec
/// 9.1); an `above_root` write goes to it only for one path that holds nothing that
/// only the user may risk ([`Envelope::above_root_approver`]), and a floor file edit
/// only when a user message names it and efrd keeps a backup
/// ([`Envelope::floor_edit`]).
fn write_need(
    envelope: &Envelope<'_>,
    cx: &WriteFacts<'_>,
    exit: Option<WriteExit>,
    path: &Path,
    part: &str,
    source: ExitSource,
) -> Option<ExitNeed> {
    let exit = exit?;
    let mut need = ExitNeed::new(exit.kind, part, source);
    need.target = Some(path.to_path_buf());
    match exit.kind {
        ExitKind::Write => {
            need.grants = match &exit.bind {
                WriteBind::Parent(parent) => vec![Grant::Write { path: parent.clone() }],
                WriteBind::ExitChild => Vec::new(),
                _ => vec![Grant::Write { path: path.to_path_buf() }],
            };
            need.approver = Approver::Classifier;
            need.bind = Some(exit.bind);
        }
        ExitKind::AboveRoot => need.approver = envelope.above_root_approver(path),
        ExitKind::Persistence => {
            let linked = cx.linked.iter().any(|link| link == path);
            let from: Vec<Source> = cx
                .installs
                .iter()
                .filter(|(dest, _)| dest == path)
                .flat_map(|(_, sources)| sources.iter().cloned().map(Source::File))
                .collect();
            let from = (!from.is_empty()).then_some(from.as_slice());
            envelope.floor_edit(&mut need, cx.facts, cx.support, linked, from);
        }
        _ => {}
    }
    set_copies(&mut need, cx.facts, cx.support);
    Some(need)
}

/// True when a fact says `path` exists, or none says.
fn exists(facts: Option<&CallFacts>, path: &Path) -> bool {
    facts.and_then(|facts| facts.target(path)).is_none_or(|kind| kind.is_some())
}

/// The kind of `path` when a fact says that it exists.
fn existing_kind(facts: Option<&CallFacts>, path: &Path) -> Option<Option<TargetKind>> {
    facts.and_then(|facts| facts.target(path))
}

/// The words of a segment for a reason's text, with what only the shell knows shown as
/// `...`.
fn part_text(segment: &Segment) -> String {
    let words: Vec<String> = segment.words.iter().map(|word| word.replace(OPAQUE, "...")).collect();
    words.join(" ")
}

/// The simple command whose words or redirections name `path`.
fn naming_part(
    segments: &[Segment],
    path: &Path,
    start: Option<&Path>,
    locations: &Locations,
) -> Option<String> {
    segments
        .iter()
        .find(|segment| {
            let dir = if segment.after_cd { None } else { start };
            let names = |word: &str| resolve(word, dir, locations).as_deref() == Some(path);
            segment.words.iter().any(|word| names(word))
                || segment.redirects.iter().any(|redirect| names(&redirect.target))
        })
        .map(part_text)
}

/// True when a simple command of `line` runs the zsh function `df` or `findmnt` of the
/// mount view: by its bare name, where the shell itself finds the program, such as
/// `df -h` or `time findmnt`. efrd then gives the call a snapshot of the host's mounts.
/// A path or a wrapper, such as `/bin/df` or `watch -n 5 df -h`, runs the file, which
/// reads no snapshot; [`predict`] gives that line an `outside` exit.
pub fn mount_view_line(line: &str) -> bool {
    scan::scan(line).iter().any(|segment| {
        programs::placed_commands(&segment.words)
            .into_iter()
            .any(|(command, by_shell)| programs::runs_mount_view(command, by_shell))
    })
}

/// Why a line may not run outside the sandbox, or `None` when it may.
///
/// The exit child runs the whole line with the user's full rights, so one approved
/// exit must not carry other code with it. The line must be one that the engine can
/// split, with no substitution, `eval`, group or function definition; redirections stay
/// allowed, because their paths are declared and get their own exits. At most one of
/// its simple commands may be other than a helper: a command that the `cautious`
/// read-only table allows, such as `echo` in `echo x | sudo tee /etc/x`, or a `sudo`
/// that only forgets the cached password, such as `sudo -k` in `sudo -k; sudo true`.
/// The answer is the text for the model; the call gets no question.
pub fn unsandboxed_line_problem(line: &str) -> Option<String> {
    let commands = match command::analyze_with_redirects(line) {
        Ok(commands) => commands,
        Err(construct) => {
            return Some(format!(
                "{ONE_COMMAND}. This line holds {construct}, so efr cannot see each command \
                 in it"
            ));
        }
    };
    let helpers = read_only_commands();
    let carrying = commands
        .iter()
        .filter(|simple| !forgets_password(simple))
        .filter(|simple| {
            command::privileged(simple).is_some()
                || !helpers
                    .iter()
                    .any(|helper| helper.matches_command(&simple.words, simple.pattern))
        })
        .count();
    (carrying > 1).then(|| ONE_COMMAND.to_owned())
}

/// The options of `sudo` that only forget the cached password and run nothing.
const FORGET_PASSWORD: &[&str] = &["-k", "-K", "--reset-timestamp", "--remove-timestamp"];

/// True for `sudo` with one option that only forgets the cached password, such as
/// `sudo -k`: it runs no program and opens nothing, so the next `sudo` asks for the
/// password again. With any other word, such as `sudo -k make`, it runs a program.
fn forgets_password(simple: &command::SimpleCommand) -> bool {
    match simple.words.as_slice() {
        [program, option] => {
            program == "sudo" && !simple.pattern && FORGET_PASSWORD.contains(&option.as_str())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
