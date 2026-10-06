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
//! sandbox must be one command plus read-only helpers.

mod envelope;
mod needs;
mod programs;
mod scan;

use std::fmt;
use std::path::{Path, PathBuf};

use efr_protocol::{ExitKind, ExitSource, Grant, Needs};

pub(crate) use self::envelope::Envelope;
use self::envelope::{WriteExit, read_exit, resolve};
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

/// What the model reads, and the user never sees, when a line that would run outside
/// the sandbox holds more than one command.
pub const ONE_COMMAND: &str = "an approved command outside the sandbox must run alone; run \
                               the other parts in a separate call";

/// One exit of a call: what it needs, what a "yes" opens, and who may approve it.
#[derive(Clone, PartialEq, Eq)]
pub struct ExitNeed {
    /// The kind.
    pub kind: ExitKind,
    /// Exactly what a "yes" opens for one call. Empty for a kind that runs in the exit
    /// child, for `destructive` (the same sandbox runs it), for a floor kind and for
    /// `desktop_ipc`, whose socket the daemon finds.
    pub grants: Vec<Grant>,
    /// The simple command that needs it, its words joined by spaces, or the whole line
    /// when no one command does.
    pub part: String,
    /// True when only the user may approve it: a kind that is user only in every phase
    /// ([`ExitKind::user_only`]), or a `write` that no contained bind serves, which then
    /// runs in the exit child.
    pub user_only: bool,
    /// Whether efr predicted it or the model asked for it.
    pub source: ExitSource,
    /// The path it is about, for a write, a read, a device or a socket.
    pub target: Option<PathBuf>,
    /// How a contained call writes [`target`](Self::target), for a `write` exit.
    pub bind: Option<WriteBind>,
}

impl ExitNeed {
    fn new(kind: ExitKind, part: impl Into<String>, source: ExitSource) -> Self {
        ExitNeed {
            kind,
            grants: Vec::new(),
            part: part.into(),
            user_only: kind.user_only(),
            source,
            target: None,
            bind: None,
        }
    }

    /// True when the call runs in the exit child after a "yes": a kind that runs
    /// unsandboxed, or a `write` that no contained bind serves.
    pub fn runs_unsandboxed(&self) -> bool {
        self.kind.runs_unsandboxed() || self.bind == Some(WriteBind::ExitChild)
    }
}

/// `part` can hold a token, such as a header of `curl`, so `Debug` shows its length.
impl fmt::Debug for ExitNeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExitNeed")
            .field("kind", &self.kind)
            .field("grants", &self.grants)
            .field("part", &format_args!("<{} bytes>", self.part.len()))
            .field("user_only", &self.user_only)
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
    for segment in &segments {
        let dir = if segment.after_cd { None } else { start.as_deref() };
        let at = |word: &str| resolve(word, dir, input.locations);
        let part = part_text(segment);
        for redirect in &segment.redirects {
            let Some(path) = at(&redirect.target) else { continue };
            let bare = match segment.words.as_slice() {
                [] => true,
                [only] => only == ":" || only == "true",
                _ => false,
            };
            if bare && !redirect.append && exists(input.facts, &path) {
                found.push(ExitNeed::new(ExitKind::Destructive, &part, ExitSource::Predicted));
            }
            made.push((path.clone(), false));
            writes.push((path, part.clone()));
        }
        for command in programs::commands(&segment.words) {
            for exit in programs::classify(command, input.support) {
                // NOTE: from phase 5 the filtered bus proxy decides a bus call.
                if exit.kind == ExitKind::Bus && input.support.bus_proxy {
                    continue;
                }
                let mut need = ExitNeed::new(exit.kind, &part, ExitSource::Predicted);
                need.grants = exit.grants;
                need.target = exit.target;
                found.push(need);
            }
            for (word, dir_kind) in programs::made_paths(command) {
                if let Some(path) = at(word) {
                    made.push((path, dir_kind));
                }
            }
            for word in programs::dd_targets(command)
                .into_iter()
                .chain(programs::sed_in_place_targets(command))
            {
                if let Some(path) = at(word) {
                    writes.push((path, part.clone()));
                }
            }
            let tracked = programs::recursive_rm_operands(command).into_iter().any(|word| {
                at(word).and_then(|path| input.facts?.tracked(&path)).is_none_or(|count| count > 0)
            });
            if tracked {
                found.push(ExitNeed::new(ExitKind::Destructive, &part, ExitSource::Predicted));
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
    for (path, part) in writes {
        if seen.contains(&path) {
            continue;
        }
        let exit = envelope.write_exit(&path, input.facts, &made);
        if let Some(need) = write_need(exit, &path, &part, ExitSource::Predicted) {
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
    found.0
}

/// The exits found so far, each kind, grant set and target once.
#[derive(Default)]
struct Found(Vec<ExitNeed>);

impl Found {
    fn push(&mut self, need: ExitNeed) {
        let known = self.0.iter().any(|known| {
            known.kind == need.kind && known.grants == need.grants && known.target == need.target
        });
        if !known {
            self.0.push(need);
        }
    }
}

/// The `write` exit, or another kind, of a write of `path` that `exit` describes.
fn write_need(
    exit: Option<WriteExit>,
    path: &Path,
    part: &str,
    source: ExitSource,
) -> Option<ExitNeed> {
    let exit = exit?;
    let mut need = ExitNeed::new(exit.kind, part, source);
    need.target = Some(path.to_path_buf());
    if exit.kind == ExitKind::Write {
        need.grants = match &exit.bind {
            WriteBind::Parent(parent) => vec![Grant::Write { path: parent.clone() }],
            WriteBind::ExitChild => Vec::new(),
            _ => vec![Grant::Write { path: path.to_path_buf() }],
        };
        need.user_only = exit.bind == WriteBind::ExitChild;
        need.bind = Some(exit.bind);
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
/// its simple commands may be other than a read-only helper, a command that the
/// `cautious` read-only table allows, such as `echo` in `echo x | sudo tee /etc/x`.
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
        .filter(|simple| {
            command::privileged(simple).is_some()
                || !helpers
                    .iter()
                    .any(|helper| helper.matches_command(&simple.words, simple.pattern))
        })
        .count();
    (carrying > 1).then(|| ONE_COMMAND.to_owned())
}

#[cfg(test)]
mod tests;
