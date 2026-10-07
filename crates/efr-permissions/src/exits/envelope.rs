//! The envelope of one call: its write roots, and what a path outside them is (spec
//! 5.1, 5.3, 5.4, 7.2, 7.5).

use std::path::{Component, Path, PathBuf};

use efr_protocol::{ExitKind, ExitSource, Grant};

use super::programs::is_sandbox_device;
use super::scan::OPAQUE;
use super::{ExitNeed, WriteBind, existing_kind};
use crate::path_class::normalize;
use crate::tables::{GIT_SURFACE, PROTECTED_NAMES, is_env_file, persistence_floors, sandbox_masks};
use crate::{Access, CallFacts, Locations, TargetKind};

/// The write roots of one call and the locations that classify the rest.
#[derive(Debug)]
pub(crate) struct Envelope<'a> {
    locations: &'a Locations,
    /// The turn's project, `$SCRATCH` and the envelope roots, in normal form under the
    /// home directory.
    roots: Vec<PathBuf>,
    /// The built-in floors and masks under the home directory.
    floors: Vec<PathBuf>,
    masks: Vec<PathBuf>,
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
        for root in project_root.into_iter().chain(scratch).chain(locations.envelope_roots()) {
            let root = locations.rehome(root).into_owned();
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        Envelope { locations, roots, floors: persistence_floors(home), masks: sandbox_masks(home) }
    }

    /// The locations the envelope classifies by.
    pub(super) fn locations(&self) -> &'a Locations {
        self.locations
    }

    /// The root that `path` lies strictly below.
    fn root_of(&self, path: &Path) -> Option<&Path> {
        self.roots.iter().map(PathBuf::as_path).find(|root| path.starts_with(root) && path != *root)
    }

    /// True when `path` is a write root, lies above one, or lies above efr's config or
    /// the home directory: a write there replaces or removes a whole root.
    fn at_or_above_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| root.starts_with(path))
            || self
                .locations
                .write_sealed_roots()
                .iter()
                .any(|root| root.starts_with(path) && root != path)
            || self.locations.is_at_or_above_home(path)
    }

    /// True when `path` is a write root of the call or lies below one.
    pub(crate) fn in_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    /// True when `path` runs code later outside the sandbox: a built-in or added floor
    /// at or above it, or a protected name or a git setting inside a write root.
    pub(crate) fn is_floor(&self, path: &Path) -> bool {
        if self
            .floors
            .iter()
            .chain(self.locations.floor_roots())
            .any(|floor| path.starts_with(floor))
        {
            return true;
        }
        let Some(root) = self.root_of(path) else {
            return false;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        let names: Vec<&str> = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect();
        let protected = names.first().is_some_and(|first| {
            PROTECTED_NAMES.iter().any(|name| name.trim_end_matches('/') == *first)
        });
        let git_surface =
            names.windows(2).any(|pair| pair[0] == ".git" && GIT_SURFACE.contains(&pair[1]))
                || names.as_slice() == [".git"];
        protected || git_surface
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
        if locations.is_write_sealed(path) {
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
    /// does a write to a floor inside a write root. The path rules judge the rest.
    pub(crate) fn tool_exit(&self, path: &Path, access: Access) -> Option<ExitNeed> {
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
                Some(need)
            }
        }
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
