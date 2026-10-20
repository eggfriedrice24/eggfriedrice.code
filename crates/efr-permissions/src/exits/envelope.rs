//! The envelope of one call: its write roots, and what a path outside them is (spec
//! 5.1, 5.3, 5.4, 7.2, 7.5).

use std::path::{Component, Path, PathBuf};

use efr_protocol::{ExitKind, ExitSource, Grant};

use super::cover::set_copies;
use super::names::floor_names;
use super::programs::is_sandbox_device;
use super::scan::OPAQUE;
use super::{Approver, ExitNeed, WriteBind, existing_kind};
use crate::path_class::normalize;
use crate::tables::{
    GIT_SURFACE, PROTECTED_NAMES, floor_group, is_env_file, persistence_floors, sandbox_masks,
};
use crate::{
    Access, AutoSupport, CallFacts, CoverAsk, CriticalWhy, FloorGroup, Locations, Source,
    TargetKind,
};

/// The write roots of one call and the locations that classify the rest.
#[derive(Debug)]
pub(crate) struct Envelope<'a> {
    locations: &'a Locations,
    /// The turn's project, `$SCRATCH` and the envelope roots, in normal form under the
    /// home directory.
    roots: Vec<PathBuf>,
    /// The roots of [`roots`](Self::roots) whose writes reach the real files: all but the
    /// private tmp and the cache overlays.
    real_roots: Vec<PathBuf>,
    /// The roots of [`real_roots`](Self::real_roots) that efrd keeps a snapshot of: the
    /// turn's project, `$SCRATCH` and the registered projects, never a
    /// `sandbox.write_roots` entry (phase B).
    snapshot_roots: Vec<PathBuf>,
    /// The turn's project and `$SCRATCH`, where a file tool writes with no exit.
    turn_roots: Vec<PathBuf>,
    /// The built-in floors and masks under the home directory.
    floors: Vec<PathBuf>,
    masks: Vec<PathBuf>,
}

/// The floor that a path lies on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Floor {
    /// The group of the floor, or `None` when only the user may change it.
    pub(crate) group: Option<FloorGroup>,
    /// The same path below the dotfile link whose target holds it.
    pub(crate) link: Option<PathBuf>,
}

/// A floor that only its place inside a write root makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InRootFloor {
    /// An agent or editor config of [`PROTECTED_NAMES`].
    Protected,
    /// A git setting or hook of a repository.
    GitSurface,
}

/// What a write outside the envelope is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WriteExit {
    pub(super) kind: ExitKind,
    /// How a contained call may write the path; [`WriteBind::ExitChild`] for a kind
    /// other than `write`.
    pub(super) bind: WriteBind,
}

impl WriteExit {
    fn of(kind: ExitKind) -> Option<Self> {
        Some(WriteExit { kind, bind: WriteBind::ExitChild })
    }

    fn write(bind: WriteBind) -> Option<Self> {
        Some(WriteExit { kind: ExitKind::Write, bind })
    }
}

impl<'a> Envelope<'a> {
    /// The envelope of a call in the turn whose project root is `project_root` and whose
    /// `$SCRATCH` is `scratch`, both as the engine resolved them.
    pub(crate) fn new(
        locations: &'a Locations,
        project_root: Option<&Path>,
        scratch: Option<&Path>,
    ) -> Self {
        let home = locations.home();
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut real_roots: Vec<PathBuf> = Vec::new();
        let mut snapshot_roots: Vec<PathBuf> = Vec::new();
        let turn_roots: Vec<PathBuf> = project_root
            .into_iter()
            .chain(scratch)
            .map(|root| locations.rehome(root).into_owned())
            .collect();
        for root in project_root.into_iter().chain(scratch).chain(locations.envelope_roots()) {
            let root = locations.rehome(root).into_owned();
            if !locations.is_private_root(&root) && !real_roots.contains(&root) {
                if turn_roots.contains(&root) || locations.is_project_root(&root) {
                    snapshot_roots.push(root.clone());
                }
                real_roots.push(root.clone());
            }
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        Envelope {
            locations,
            roots,
            real_roots,
            snapshot_roots,
            turn_roots,
            floors: persistence_floors(home),
            masks: sandbox_masks(home),
        }
    }

    /// The locations the envelope classifies by.
    pub(super) fn locations(&self) -> &'a Locations {
        self.locations
    }

    /// The root that `path` lies strictly below.
    fn root_of(&self, path: &Path) -> Option<&Path> {
        self.roots.iter().map(PathBuf::as_path).find(|root| path.starts_with(root) && path != *root)
    }

    /// True when `path` is a write root, lies above one, or lies above efr's config, efr's
    /// own programs and roots or the home directory: a write there replaces or removes a
    /// whole root.
    fn at_or_above_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| root.starts_with(path))
            || self
                .locations
                .write_sealed_roots()
                .iter()
                .chain(self.locations.efr_paths())
                .any(|root| root.starts_with(path) && root != path)
            || self.locations.is_at_or_above_home(path)
    }

    /// True when `path` is a write root of the call or lies below one.
    pub(crate) fn in_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    /// True when `path`, in normal form, is a real write root of the call or lies below
    /// one: the turn's project, `$SCRATCH`, a registered project or a
    /// `sandbox.write_roots` entry, never the private tmp or a cache overlay. A program
    /// there may have been written in the sandbox, and a write there reaches the user's
    /// files.
    pub(crate) fn in_real_root(&self, path: &Path) -> bool {
        let path = self.locations.rehome(path);
        self.real_roots.iter().any(|root| path.starts_with(root))
    }

    /// True when an answer of [`Coverage::Covered`](crate::Coverage) to `ask` may make a
    /// destructive part routine: the part of the rule of efr's auto spec 10.7 that the
    /// engine can check by name alone (phase B). The daemon checks the files.
    ///
    /// A path must lie strictly below a root that efrd keeps a snapshot of (the turn's
    /// project, `$SCRATCH`, a registered project), never at or above a write root, in a
    /// git dir or on a floor. The directory of a git command must be such a root or lie
    /// below one, and not in a git dir. Any other ask never counts here.
    pub(crate) fn may_cover(&self, ask: &CoverAsk) -> bool {
        match ask {
            CoverAsk::Path(path) => {
                let path = self.locations.rehome(path);
                let below = self
                    .snapshot_roots
                    .iter()
                    .any(|root| path.starts_with(root) && path.as_ref() != root.as_path());
                below
                    && !self.at_or_above_root(&path)
                    && !in_git_dir(&path)
                    && !self.is_floor(&path)
                    && !self.locations.is_efr_own(&path)
                    && !self.is_secret(&path)
            }
            CoverAsk::Tracked(dir)
            | CoverAsk::Untracked(dir)
            | CoverAsk::Refs(dir)
            | CoverAsk::Stash(dir) => {
                let dir = self.locations.rehome(dir);
                self.snapshot_roots.iter().any(|root| dir.starts_with(root)) && !in_git_dir(&dir)
            }
            _ => false,
        }
    }

    /// Who approves an `above_root` write of `path` (spec 9.1): the user for `~`, `/`, a
    /// directory above `~` or outside it, and a directory that holds two or more
    /// registered projects or real write roots, a built-in or added floor, efr's config,
    /// or a secret; else the classifier when a user message names the path. A project's
    /// own `.git` and protected names are floors only by their place in a write root,
    /// so they do not count: a named delete of one whole project can reach the
    /// classifier.
    pub(crate) fn above_root_approver(&self, path: &Path) -> Approver {
        let locations = self.locations;
        let user = locations.is_at_or_above_home(path)
            || !path.starts_with(locations.home())
            || self.holds(path).is_some();
        if user { Approver::User } else { Approver::ClassifierWhenNamed }
    }

    /// Why a write that removes `path`, a path in normal form under the home directory,
    /// is critical: it is or holds efr's config or one of efr's own paths, a secret, a
    /// built-in or added floor, or two or more registered projects or real write roots.
    /// `None` when it holds none of them. [`above_root_approver`](Self::above_root_approver)
    /// gives [`Approver::User`] exactly when this is `Some`, or when the path is at or
    /// above the home directory or outside it; the guard of full mode reads the same
    /// test (`exits::critical`), so the two never drift.
    pub(crate) fn holds(&self, path: &Path) -> Option<CriticalWhy> {
        let locations = self.locations;
        let below = |root: &PathBuf| root.starts_with(path);
        if locations.write_sealed_roots().iter().chain(locations.efr_paths()).any(below) {
            return Some(CriticalWhy::HoldsEfr);
        }
        if locations.is_secret(path) || !locations.secrets_below(path).is_empty() {
            return Some(CriticalWhy::HoldsSecret);
        }
        if self.floors.iter().chain(locations.floor_roots()).any(below) {
            return Some(CriticalWhy::HoldsFloor);
        }
        let mut held: Vec<&Path> = locations.projects_below(path);
        for root in self.real_roots.iter().filter(|root| below(root)) {
            if !held.contains(&root.as_path()) {
                held.push(root);
            }
        }
        (held.len() >= 2).then_some(CriticalWhy::HoldsProjects)
    }

    /// True when `path` runs code later outside the sandbox: a built-in or added floor
    /// at or above it, or a protected name or a git setting inside a write root.
    pub(crate) fn is_floor(&self, path: &Path) -> bool {
        self.floor_of(path).is_some()
    }

    /// The floor that `path` lies on, or `None` when it lies on none. Its group is the
    /// group of the floors at or above `path`, or `None` when one of them has no group
    /// (`sandbox.protect`, a floor the daemon added without one, the git surface of a
    /// repository): only the user then approves an edit. Below the target of a dotfile
    /// link, the floor also knows the same path below the link.
    pub(crate) fn floor_of(&self, path: &Path) -> Option<Floor> {
        let mut found = false;
        let mut group: Option<FloorGroup> = None;
        let mut in_no_group = false;
        let mut link: Option<PathBuf> = None;
        if let Some(built_in) = floor_group(self.locations.home(), path) {
            found = true;
            group = Some(built_in);
        }
        for added in
            self.locations.added_floors().iter().filter(|added| path.starts_with(&added.root))
        {
            found = true;
            match added.group {
                Some(added) => group = group.or(Some(added)),
                None => in_no_group = true,
            }
            if let (None, Some(through), Ok(below)) =
                (&link, &added.link, path.strip_prefix(&added.root))
            {
                link = Some(if below.as_os_str().is_empty() {
                    through.clone()
                } else {
                    through.join(below)
                });
            }
        }
        match self.in_root_floor(path) {
            Some(InRootFloor::Protected) => {
                found = true;
                group = group.or(Some(FloorGroup::Agent));
            }
            Some(InRootFloor::GitSurface) => {
                found = true;
                in_no_group = true;
            }
            None => {}
        }
        found.then_some(Floor { group: if in_no_group { None } else { group }, link })
    }

    /// The floor that only its place inside a write root makes of `path`: a protected
    /// name, or the git surface of a repository.
    fn in_root_floor(&self, path: &Path) -> Option<InRootFloor> {
        let root = self.root_of(path)?;
        let relative = path.strip_prefix(root).ok()?;
        let names: Vec<&str> = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect();
        let git_surface =
            names.windows(2).any(|pair| pair[0] == ".git" && GIT_SURFACE.contains(&pair[1]))
                || names.as_slice() == [".git"];
        if git_surface {
            return Some(InRootFloor::GitSurface);
        }
        let protected = names.first().is_some_and(|first| {
            PROTECTED_NAMES.iter().any(|name| name.trim_end_matches('/') == *first)
        });
        protected.then_some(InRootFloor::Protected)
    }

    /// Who approves a `persistence` write of `path`, with the names of the exit (spec
    /// 9.1 and the decision of 2026-10-10 on floor file edits). The classifier judges it
    /// when a user message names the file, only when all of these hold: efrd keeps its
    /// home store ([`AutoSupport::floor_backup`]); the floor has a group; a fact says
    /// that the target is a file, or that it is missing and its parent is a directory;
    /// the line does not make a symbolic link there; and the target is not in a
    /// `*.wants` or `*.requires` directory, where a file enables a unit. Else only the
    /// user approves it. What efrd copies first, whoever approves it, is
    /// [`cover::set_copies`](super::cover::set_copies).
    pub(super) fn floor_edit(
        &self,
        need: &mut ExitNeed,
        facts: Option<&CallFacts>,
        support: &AutoSupport,
        linked: bool,
        from: Option<&[Source]>,
    ) {
        let Some(path) = need.target.clone() else { return };
        let Some(floor) = self.floor_of(&path) else { return };
        let kind = existing_kind(facts, &path);
        let file = match kind {
            Some(Some(TargetKind::File)) => true,
            Some(None) => path
                .parent()
                .is_some_and(|parent| existing_kind(facts, parent) == Some(Some(TargetKind::Dir))),
            Some(Some(TargetKind::Dir | TargetKind::Other)) | None => false,
        };
        let enables = path.ancestors().skip(1).any(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".wants") || name.ends_with(".requires"))
        });
        // NOTE: what a `cp` or `ln` installs is another file, which the classifier must
        // read whole (plan PB19a): then a copy into a floor dir and a link into a unit
        // dir go to it too. A link elsewhere in a floor makes a new name, such as a new
        // unit, so only the user approves it.
        let installed = from.map(|sources| super::persist::readable(self, facts, sources));
        let named = match &installed {
            None => support.floor_backup && file && !linked && !enables && floor.group.is_some(),
            Some(None) => false,
            Some(Some(_)) => {
                let dir = kind == Some(Some(TargetKind::Dir));
                support.floor_backup
                    && (file || dir)
                    && (!linked || enables)
                    && floor.group.is_some()
            }
        };
        if let (true, Some(Some(read))) = (named, installed) {
            need.installs = read;
        }
        need.approver = if named { Approver::ClassifierWhenNamed } else { Approver::User };
        let home = self.locations.home();
        let mut names = floor_names(&path, home, floor.group);
        if let Some(link) = &floor.link {
            names.extend(floor_names(link, home, floor.group));
        }
        need.names = super::names::dedup(names);
        need.link = floor.link;
    }

    /// True when a mask hides `path` in a contained call.
    pub(crate) fn is_masked(&self, path: &Path) -> bool {
        self.mask_of(path).is_some()
    }

    /// The mask that hides `path`: a built-in or added mask at or above it, or the
    /// path itself for a project `.env` file in a write root.
    fn mask_of(&self, path: &Path) -> Option<PathBuf> {
        if let Some(mask) =
            self.masks.iter().chain(self.locations.mask_roots()).find(|mask| path.starts_with(mask))
        {
            return Some(mask.clone());
        }
        let root = self.root_of(path)?;
        let depth = path.strip_prefix(root).ok()?.components().count();
        let name = path.file_name()?.to_str()?;
        (depth <= 3 && is_env_file(name)).then(|| path.to_path_buf())
    }

    /// True for a directory that many programs keep config in, or one at or above a
    /// write root: a bind of it would open too much.
    fn is_shared(&self, dir: &Path) -> bool {
        shared_home_dir(dir, self.locations.home()) || self.at_or_above_root(dir)
    }

    /// The exit of a write of `path`, a path in normal form under the home directory,
    /// or `None` when the sandbox lets the call write it. `made` are the paths that the
    /// line's programs make as exactly what they name, each with `true` for a
    /// directory.
    pub(super) fn write_exit(
        &self,
        path: &Path,
        facts: Option<&CallFacts>,
        made: &[(PathBuf, bool)],
    ) -> Option<WriteExit> {
        let locations = self.locations;
        if locations.is_sealed(path) || locations.is_secret(path) {
            return WriteExit::of(ExitKind::Secret);
        }
        if locations.is_write_sealed(path) || locations.is_efr_own(path) {
            return WriteExit::of(ExitKind::Config);
        }
        if self.at_or_above_root(path) {
            return WriteExit::of(ExitKind::AboveRoot);
        }
        if self.is_floor(path) {
            return WriteExit::of(ExitKind::Persistence);
        }
        // NOTE: a mask never opens for writing, and a grant cannot help a write that
        // needs root, so both run in the exit child.
        if self.mask_of(path).is_some() {
            return WriteExit::write(WriteBind::ExitChild);
        }
        // NOTE: every contained call has its own `/dev` with these nodes, so a write
        // such as `2>/dev/null` stays in the sandbox.
        if self.root_of(path).is_some() || is_sandbox_device(path) {
            return None;
        }
        if locations.synced_roots().iter().any(|synced| path.starts_with(synced)) {
            return WriteExit::of(ExitKind::SyncedWrite);
        }
        if !path.starts_with(locations.home()) {
            return WriteExit::write(WriteBind::ExitChild);
        }
        WriteExit::write(self.bind(path, facts, made))
    }

    /// How a contained call may write `path`, a user path outside every root (spec
    /// 7.5). A missing fact counts as an existing target.
    fn bind(&self, path: &Path, facts: Option<&CallFacts>, made: &[(PathBuf, bool)]) -> WriteBind {
        let parent = nearest_existing_parent(path, facts);
        let shared = self.is_shared(&parent);
        match existing_kind(facts, path) {
            Some(Some(TargetKind::Dir)) => WriteBind::Target,
            Some(None) => match made.iter().find(|(made, _)| made == path) {
                Some((_, true)) => WriteBind::MakeDir,
                Some((_, false)) => WriteBind::MakeFile,
                None if shared => WriteBind::ExitChild,
                None => WriteBind::Parent(parent),
            },
            Some(Some(_)) | None if shared => WriteBind::TargetOnly,
            Some(Some(_)) | None => WriteBind::Parent(parent),
        }
    }

    /// The exit of a `read_file`, `write_file` or edit of `path` in `auto`, which the
    /// daemon runs outside the sandbox: a read of a sandbox mask asks, and so
    /// does a write to a floor inside a write root that the path rules allow. A write
    /// that they ask about is [`Envelope::tool_write_exit`]. `facts` and `support` decide
    /// who approves a floor file edit ([`Envelope::floor_edit`]).
    pub(crate) fn tool_exit(
        &self,
        path: &Path,
        access: Access,
        facts: Option<&CallFacts>,
        support: &AutoSupport,
    ) -> Option<ExitNeed> {
        match access {
            Access::Read | Access::ReadTree => self.masked_read(path, ExitSource::Predicted),
            Access::Write => {
                if self.root_of(path).is_none() || !self.is_floor(path) {
                    return None;
                }
                let mut need = ExitNeed::new(
                    ExitKind::Persistence,
                    path.display().to_string(),
                    ExitSource::Predicted,
                );
                need.target = Some(path.to_path_buf());
                self.floor_edit(&mut need, facts, support, false, None);
                set_copies(&mut need, facts, support);
                Some(need)
            }
        }
    }

    /// The exit of a `write_file`, edit or patch of `path` outside the turn's project
    /// and `$SCRATCH` in `auto` (spec 7.4): the kind that a shell write there would
    /// have, without the sandbox's envelope roots, because efrd writes the host's real
    /// `/tmp` and caches. A plain write is a `write` exit for the classifier, a floor a
    /// `persistence` exit, a synced folder a `synced_write` exit; efr's config and a
    /// secret are floors. efrd writes the file itself, so the exit has no grant and no
    /// bind. `facts` and `support` decide who approves a floor file edit, as for a shell
    /// line ([`Envelope::floor_edit`]).
    pub(crate) fn tool_write_exit(
        &self,
        path: &Path,
        facts: Option<&CallFacts>,
        support: &AutoSupport,
    ) -> ExitNeed {
        let locations = self.locations;
        let kind = if locations.is_sealed(path) || locations.is_secret(path) {
            ExitKind::Secret
        } else if locations.is_write_sealed(path) || locations.is_efr_own(path) {
            ExitKind::Config
        } else if self.at_or_above_root(path) {
            ExitKind::AboveRoot
        } else if self.is_floor(path) {
            ExitKind::Persistence
        } else if locations.synced_roots().iter().any(|synced| path.starts_with(synced)) {
            ExitKind::SyncedWrite
        } else {
            ExitKind::Write
        };
        let mut need = ExitNeed::new(kind, path.display().to_string(), ExitSource::Predicted);
        need.target = Some(path.to_path_buf());
        match kind {
            ExitKind::Write => need.approver = Approver::Classifier,
            ExitKind::AboveRoot => need.approver = self.above_root_approver(path),
            ExitKind::Persistence => self.floor_edit(&mut need, facts, support, false, None),
            _ => {}
        }
        set_copies(&mut need, facts, support);
        need
    }

    /// The ask that covers a file tool's write of `path` when the call deletes or moves
    /// a file (phase B): the snapshot of the turn's project or `$SCRATCH` for a path
    /// strictly below one, else the copy in the home store, which the write exit of
    /// such a path copies.
    pub(crate) fn tool_cover_ask(&self, path: &Path) -> CoverAsk {
        let path = self.locations.rehome(path).into_owned();
        let below = self.turn_roots.iter().any(|root| path.starts_with(root) && path != *root);
        if below { CoverAsk::Path(path) } else { CoverAsk::Home(path) }
    }

    /// The `masked_read` exit of a read of `path`, when a mask hides it.
    pub(super) fn masked_read(&self, path: &Path, source: ExitSource) -> Option<ExitNeed> {
        let mask = self.mask_of(path)?;
        let mut need = ExitNeed::new(ExitKind::MaskedRead, path.display().to_string(), source);
        need.grants = vec![Grant::Unmask { path: mask }];
        need.target = Some(path.to_path_buf());
        Some(need)
    }

    /// True when `path` is an engine secret.
    pub(super) fn is_secret(&self, path: &Path) -> bool {
        self.locations.is_sealed(path) || self.locations.is_secret(path)
    }
}

/// The exit of a declared read of `path` in a shell call: a secret is a floor, a
/// masked path a `masked_read`. A read of a directory that holds masks needs nothing,
/// because they read as empty.
pub(super) fn read_exit(
    envelope: &Envelope<'_>,
    path: &Path,
    _access: Access,
    source: ExitSource,
) -> Option<ExitNeed> {
    if envelope.is_secret(path) {
        let mut need = ExitNeed::new(ExitKind::Secret, path.display().to_string(), source);
        need.target = Some(path.to_path_buf());
        return Some(need);
    }
    envelope.masked_read(path, source)
}

/// True for the home directory and the dirs below it that many programs keep config
/// in (`~/.config`, `~/.local`, `~/.local/share`, `~/.local/state`).
pub(super) fn shared_home_dir(dir: &Path, home: &Path) -> bool {
    dir == home
        || [".config", ".local", ".local/share", ".local/state"]
            .iter()
            .any(|shared| dir == home.join(shared))
}

/// True when `path` has a `.git` component: it is a git dir or lies in one.
fn in_git_dir(path: &Path) -> bool {
    path.components().any(|component| component == Component::Normal(".git".as_ref()))
}

/// The nearest parent of `path` that exists, or that no fact says is missing.
fn nearest_existing_parent(path: &Path, facts: Option<&CallFacts>) -> PathBuf {
    let mut dir = path.parent();
    while let Some(candidate) = dir {
        match existing_kind(facts, candidate) {
            Some(None) => dir = candidate.parent(),
            _ => return candidate.to_path_buf(),
        }
    }
    PathBuf::from("/")
}

/// `word` as an absolute path in normal form under the home directory: `~` and `~/...`
/// mean the home directory, and a relative word lies in `dir`. `None` for a word that
/// only the shell can resolve (an expansion, a pattern), an option, or a relative word
/// whose directory is unknown.
pub(super) fn resolve(word: &str, dir: Option<&Path>, locations: &Locations) -> Option<PathBuf> {
    if word.is_empty()
        || word.starts_with('-')
        || word.contains([OPAQUE, '*', '?', '['])
        || (word.starts_with('~') && word != "~" && !word.starts_with("~/"))
    {
        return None;
    }
    let path = match word.strip_prefix('~') {
        Some(rest) => locations.home().join(rest.trim_start_matches('/')),
        None if word.starts_with('/') => PathBuf::from(word),
        None => dir?.join(word),
    };
    let path = normalize(&path)?;
    Some(locations.rehome(&path).into_owned())
}
