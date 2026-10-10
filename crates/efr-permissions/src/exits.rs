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
//! [`unsandboxed_line_problem`] is the one-command rule: a line that runs outside the
//! sandbox must be one command plus read-only helpers, or a `sudo -k` that only forgets
//! the cached password.

mod envelope;
mod names;
mod needs;
mod programs;
mod scan;

use std::fmt;
use std::path::{Path, PathBuf};

use efr_protocol::{ExitKind, ExitSource, Grant, Needs};

pub(crate) use self::envelope::Envelope;
use self::envelope::{WriteExit, read_exit, resolve};
#[cfg(test)]
pub(crate) use self::names::floor_names;
pub use self::names::{GENERIC_STEMS, NAME_PHRASES, names_match};
pub use self::programs::DESKTOP_IPC_READ;
use self::scan::{OPAQUE, Segment};
use crate::command;
use crate::path_class::normalize;
use crate::policy::read_only_commands;
use crate::{Access, AutoSupport, CallFacts, Egress, Locations, PathAccess, PathClass, TargetKind};

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

/// The registry hosts of each ecosystem, from the network presets of spec 8.3. The
/// pacman mirrors depend on the machine, so they are not here: the daemon reads them
/// from the mirror list when it needs them.
///
/// The classifier's naming rule lets a `host` exit of a package manager's fetch reach
/// these hosts without a user message that names them, when a user message asks to
/// build, install, update or fetch (spec 9.4, a proposal behind this table). An empty
/// table turns the exception off.
pub const REGISTRY_HOSTS: &[(Ecosystem, &[&str])] = &[
    (Ecosystem::Crates, &["crates.io", "index.crates.io", "static.crates.io"]),
    (Ecosystem::Npm, &["registry.npmjs.org", "registry.yarnpkg.com"]),
    (Ecosystem::Pypi, &["pypi.org", "files.pythonhosted.org"]),
    (Ecosystem::Go, &["proxy.golang.org", "sum.golang.org"]),
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
    /// child, for `destructive` (the same sandbox runs it), for a floor kind, for
    /// `desktop_ipc`, whose socket the daemon finds, and for a file tool, which efrd
    /// runs itself.
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
    /// True when efrd copies the target into its backup store before the call runs: a
    /// `persistence` write of a file, or of a new file, while
    /// [`AutoSupport::floor_backup`] is true, whoever approves it.
    pub backup_before_write: bool,
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
            backup_before_write: false,
            package_fetch: None,
            source,
            target: None,
            bind: None,
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
            .field("backup_before_write", &self.backup_before_write)
            .field("package_fetch", &self.package_fetch)
            .field("source", &self.source)
            .field("target", &self.target)
            .field("bind", &self.bind)
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
    let mut found = Found::default();
    let mut made: Vec<(PathBuf, bool)> = Vec::new();
    let mut writes: Vec<(PathBuf, String)> = Vec::new();
    let mut fetch: Option<Ecosystem> = None;
    let mut linked: Vec<PathBuf> = Vec::new();
    for segment in &segments {
        let dir = if segment.after_cd { None } else { start.as_deref() };
        let at = |word: &str| resolve(word, dir, input.locations);
        let part = part_text(segment);
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
                found.push(destructive(&envelope, &part, &[Some(path.clone())]));
            }
            made.push((path.clone(), false));
            writes.push((path, part.clone()));
        }
        for command in programs::commands(&segment.words) {
            fetch = fetch.or_else(|| programs::package_fetch(command));
            for exit in programs::classify(command, input.support) {
                // NOTE: from phase E the filtered bus proxy decides a bus call.
                if exit.kind == ExitKind::Bus && input.support.bus_proxy {
                    continue;
                }
                if exit.kind == ExitKind::Destructive {
                    let targets: Vec<Option<PathBuf>> = match programs::destructive_targets(command)
                    {
                        Some(words) => words.into_iter().map(at).collect(),
                        None => vec![git_dir(command, dir, input.locations)],
                    };
                    found.push(destructive(&envelope, &part, &targets));
                    continue;
                }
                found.push(predicted(exit, &part));
            }
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
                found.push(destructive(&envelope, &part, &targets));
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
    let cx = WriteFacts { facts: input.facts, support: input.support, linked: &linked };
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
        found.push(need);
    }
    if let Some(asked) = input.needs {
        let dir = start.as_deref();
        for need in needs::exits(asked, &envelope, dir, input.facts, &made, input.support) {
            found.push(need);
        }
    }
    let mut found = found.0;
    for need in &mut found {
        match need.kind {
            ExitKind::Outside => need.approver = outside_approver(input, &envelope),
            ExitKind::Host => need.package_fetch = fetch,
            _ => {}
        }
    }
    named_targets(found, input.locations.home())
}

/// Who approves an `outside` exit of `input` (spec 9.1): the user when a program word of
/// the line, or a program that a wrapper runs, changed in this turn or lies in a real
/// write root, so that code the sandbox wrote never gets the user's full rights; the
/// user too when a fact about a program word is missing. Else the classifier. A cache
/// overlay and the private tmp are no real write root: the sandbox never writes the
/// real files there.
fn outside_approver(input: &ExitInput<'_>, envelope: &Envelope<'_>) -> Approver {
    let Some(facts) = input.facts else {
        return Approver::User;
    };
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
fn destructive(envelope: &Envelope<'_>, part: &str, targets: &[Option<PathBuf>]) -> ExitNeed {
    let mut need = ExitNeed::new(ExitKind::Destructive, part, ExitSource::Predicted);
    let known = !targets.is_empty()
        && targets
            .iter()
            .all(|target| target.as_ref().is_some_and(|path| envelope.in_real_root(path)));
    need.approver = if known { Approver::Classifier } else { Approver::User };
    need
}

/// The repository that a `git` command of `words` works on: the directory of `-C`, or
/// the directory the command runs in.
fn git_dir(words: &[String], dir: Option<&Path>, locations: &Locations) -> Option<PathBuf> {
    let at = words.iter().position(|word| word == "-C");
    match at.and_then(|at| words.get(at + 1)) {
        Some(other) => resolve(other, dir, locations),
        None => dir.map(Path::to_path_buf),
    }
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
        for redirect in &segment.redirects {
            if let Some(path) = written_at(&redirect.target) {
                add(&mut request.targets, path);
            }
        }
        for (command, by_shell) in programs::placed_commands(&segment.words) {
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
                    add(&mut request.targets, path);
                }
            }
            for word in programs::recursive_rm_operands(command) {
                if let Some(path) = at(word) {
                    add(&mut request.targets, path.clone());
                    add(&mut request.tracked, path);
                }
            }
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
            if let Some(path) = resolve(word, start.as_deref(), locations)
                && !self.targets.contains(&path)
            {
                self.targets.push(path);
            }
        }
        self
    }
}

/// The exits found so far, each kind, grant set and target once.
#[derive(Default)]
struct Found(Vec<ExitNeed>);

impl Found {
    /// Adds `need`, or merges it into a known exit of the same kind, grants and target:
    /// the stricter approver of the two wins, and the names of both stay.
    fn push(&mut self, need: ExitNeed) {
        let known = self.0.iter_mut().find(|known| {
            known.kind == need.kind && known.grants == need.grants && known.target == need.target
        });
        match known {
            Some(known) => {
                known.approver = known.approver.max(need.approver);
                for name in need.names {
                    if !known.names.contains(&name) {
                        known.names.push(name);
                    }
                }
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
    need
}

/// What the exit of a write needs beyond its path: the daemon's facts, the machine's
/// support, and the paths that the line makes as symbolic links.
pub(super) struct WriteFacts<'a> {
    pub(super) facts: Option<&'a CallFacts>,
    pub(super) support: &'a AutoSupport,
    pub(super) linked: &'a [PathBuf],
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
            envelope.floor_edit(&mut need, cx.facts, cx.support, linked);
        }
        _ => {}
    }
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
