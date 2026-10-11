//! The envelope of one call: its write roots, and what a path outside them is (spec
//! 5.1, 5.3, 5.4, 7.2, 7.5).

use std::path::{Component, Path, PathBuf};

use efr_protocol::{ExitKind, ExitSource, Grant};

use super::names::floor_names;
use super::programs::is_sandbox_device;
use super::scan::OPAQUE;
use super::{Approver, ExitNeed, WriteBind, existing_kind};
use crate::path_class::normalize;
use crate::tables::{
    GIT_SURFACE, PROTECTED_NAMES, floor_group, is_env_file, persistence_floors, sandbox_masks,
};
use crate::{Access, AutoSupport, CallFacts, FloorGroup, Locations, TargetKind};

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
        for root in project_root.into_iter().chain(scratch).chain(locations.envelope_roots()) {
            let root = locations.rehome(root).into_owned();
            if !locations.is_private_root(&root) && !real_roots.contains(&root) {
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

    /// Who approves an `above_root` write of `path` (spec 9.1): the user for `~`, `/`, a
    /// directory above `~` or outside it, and a directory that holds two or more
    /// registered projects or real write roots, a built-in or added floor, efr's config,
    /// or a secret; else the classifier when a user message names the path. A project's
    /// own `.git` and protected names are floors only by their place in a write root,
    /// so they do not count: a named delete of one whole project can reach the
    /// classifier.
    pub(crate) fn above_root_approver(&self, path: &Path) -> Approver {
        let locations = self.locations;
        let below = |root: &PathBuf| root.starts_with(path);
        let mut held: Vec<&Path> = locations.projects_below(path);
        for root in self.real_roots.iter().filter(|root| below(root)) {
            if !held.contains(&root.as_path()) {
                held.push(root);
            }
        }
        let holds = locations.is_at_or_above_home(path)
            || !path.starts_with(locations.home())
            || held.len() >= 2
            || self.floors.iter().chain(locations.floor_roots()).any(below)
            || locations.write_sealed_roots().iter().chain(locations.efr_paths()).any(below)
            || locations.is_secret(path)
            || !locations.secrets_below(path).is_empty();
        if holds { Approver::User } else { Approver::ClassifierWhenNamed }
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

    /// Who approves a `persistence` write of `path`, with the names and the backup of
    /// the exit (spec 9.1 and the decision of 2026-10-10 on floor file edits). The
    /// classifier judges it when a user message names the file, only when all of these
    /// hold: efrd keeps a backup ([`AutoSupport::floor_backup`]); the floor has a group;
    /// a fact says that the target is a file, or that it is missing and its parent is a
    /// directory; the line does not make a symbolic link there; and the target is not in
    /// a `*.wants` or `*.requires` directory, where a file enables a unit. Else only the
    /// user approves it. efrd copies the target first whenever it keeps backups and the
    /// target is no directory, whoever approves it.
    pub(super) fn floor_edit(
        &self,
        need: &mut ExitNeed,
        facts: Option<&CallFacts>,
        support: &AutoSupport,
        linked: bool,
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
        let named = support.floor_backup && file && !linked && !enables && floor.group.is_some();
        need.approver = if named { Approver::ClassifierWhenNamed } else { Approver::User };
        need.backup_before_write = support.floor_backup
            && !matches!(kind, Some(Some(TargetKind::Dir | TargetKind::Other)));
        let home = self.locations.home();
        let mut names = floor_names(&path, home, floor.group);
        if let Some(link) = &floor.link {
            names.extend(floor_names(link, home, floor.group));
        }
        need.names = super::names::dedup(names);
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
        let home = self.locations.home();
        dir == home
            || [".config", ".local", ".local/share", ".local/state"]
                .iter()
                .any(|shared| dir == home.join(shared))
            || self.at_or_above_root(dir)
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
                self.floor_edit(&mut need, facts, support, false);
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
            ExitKind::Persistence => self.floor_edit(&mut need, facts, support, false),
            _ => {}
        }
        need
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
