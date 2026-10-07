//! The surface guard (the spec's section 5.6, layer 2): git settings and hooks that a
//! call planted where a pin cannot stop them.
//!
//! Before the run the launcher records a [`SurfaceManifest`] of each pinned git dir and
//! of each git dir that [`scan_git_dirs`] finds in the guard roots; after the run it
//! records another and [`check_surface`] compares the two. A change marked
//! `quarantined` must move to `$SBX/quarantine/<call>/`, outside the sandbox's view,
//! before anything reads it; efrd then asks the user whether to keep it.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::{ReportedFile, SurfaceChange};

use crate::fs_view::{FileKind, FsView};
use crate::git_config::{cargo_code_keys, code_keys};
use crate::paths::{is_within, normalize};

/// The most bytes of one file that the manifest keeps; a longer file counts as
/// changed in every comparison.
pub const MAX_SURFACE_FILE: usize = 256 * 1024;

/// Lists a git config with git itself, so git's own parser decides what it holds:
/// `git config --file <config> --list --no-includes -z`, in a scrubbed environment.
pub trait ConfigLister {
    /// The listing of the config file `config`.
    fn list(&self, config: &Path) -> io::Result<String>;
}

/// A rule of the surface guard; its name is the wire `SurfaceChange::rule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SurfaceRule {
    /// A `commondir` in a main git dir, which points git at another config.
    CommondirInMainGitDir,
    /// A new `config.worktree` in a main git dir.
    ConfigWorktreeAppeared,
    /// A worktree's `commondir` that does not point back to its main git dir.
    WorktreeCommondirElsewhere,
    /// A new `config.worktree` of a worktree.
    WorktreeConfigWorktreeAppeared,
    /// A config with a key that runs a program.
    ConfigCodeKey,
    /// A config that git cannot list.
    ConfigUnreadable,
    /// A `commondir` of a nested repository that points outside it.
    CommondirOutsideRepo,
    /// A hook that is not a `*.sample`.
    HookPlanted,
    /// A new submodule git dir with a config key that runs a program.
    ModuleCodeKey,
    /// A new submodule git dir with a hook that is not a `*.sample`.
    ModuleHook,
    /// `objects/info/alternates` is new or changed: objects come from elsewhere.
    AlternatesChanged,
    /// A new agent or editor config at a repository root, such as `.envrc`.
    ProtectedNameCreated,
}

impl SurfaceRule {
    /// The rule's name on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            SurfaceRule::CommondirInMainGitDir => "commondir_in_main_git_dir",
            SurfaceRule::ConfigWorktreeAppeared => "config_worktree_appeared",
            SurfaceRule::WorktreeCommondirElsewhere => "worktree_commondir_elsewhere",
            SurfaceRule::WorktreeConfigWorktreeAppeared => "worktree_config_worktree_appeared",
            SurfaceRule::ConfigCodeKey => "config_code_key",
            SurfaceRule::ConfigUnreadable => "config_unreadable",
            SurfaceRule::CommondirOutsideRepo => "commondir_outside_repo",
            SurfaceRule::HookPlanted => "hook_planted",
            SurfaceRule::ModuleCodeKey => "module_code_key",
            SurfaceRule::ModuleHook => "module_hook",
            SurfaceRule::AlternatesChanged => "alternates_changed",
            SurfaceRule::ProtectedNameCreated => "protected_name_created",
        }
    }

    /// True when the launcher moves the entry to quarantine; the others are reported.
    pub const fn quarantines(self) -> bool {
        !matches!(self, SurfaceRule::AlternatesChanged | SurfaceRule::ProtectedNameCreated)
    }
}

/// A git dir that the guard records.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitDirTarget {
    /// The git dir.
    pub git_dir: PathBuf,
    /// True for a pinned top git dir (a main git dir).
    pub main: bool,
    /// The repository root, where protected names count; `None` for a bare or a
    /// registered git dir.
    pub work_tree: Option<PathBuf>,
}

/// What one path held.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Snap {
    Missing,
    File(Vec<u8>),
    TooLarge,
    Dir,
    Link(PathBuf),
    Other,
}

impl Snap {
    fn exists(&self) -> bool {
        *self != Snap::Missing
    }

    fn text(&self) -> Option<String> {
        match self {
            Snap::File(bytes) => Some(String::from_utf8_lossy(bytes).trim().to_owned()),
            _ => None,
        }
    }
}

/// A config file and its code keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConfigState {
    snap: Snap,
    keys: Vec<String>,
    unreadable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeState {
    commondir: Snap,
    config_worktree: ConfigState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleState {
    config: ConfigState,
    hooks: Hooks,
}

/// A hooks dir: missing, a link, or its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hooks {
    Missing,
    Link(PathBuf),
    Entries(BTreeMap<String, Snap>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoState {
    main: bool,
    work_tree: Option<PathBuf>,
    commondir: Snap,
    config: ConfigState,
    config_worktree: ConfigState,
    hooks: Hooks,
    worktrees: BTreeMap<String, WorktreeState>,
    modules: BTreeMap<String, ModuleState>,
    alternates: Snap,
    protected: BTreeSet<String>,
}

/// The git surface of a set of git dirs at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurfaceManifest {
    repos: BTreeMap<PathBuf, RepoState>,
}

struct Capture<'a> {
    fs: &'a dyn FsView,
    lister: &'a dyn ConfigLister,
}

impl Capture<'_> {
    fn snap(&self, path: &Path) -> Snap {
        match self.fs.lstat(path) {
            Err(_) => Snap::Missing,
            Ok(FileKind::Dir) => Snap::Dir,
            Ok(FileKind::Other) => Snap::Other,
            Ok(FileKind::Symlink) => Snap::Link(self.fs.read_link(path).unwrap_or_default()),
            Ok(FileKind::File) => match self.fs.read_file(path, MAX_SURFACE_FILE) {
                Ok(bytes) => Snap::File(bytes),
                Err(_) => Snap::TooLarge,
            },
        }
    }

    fn config(&self, path: &Path) -> ConfigState {
        let snap = self.snap(path);
        let (keys, unreadable) = match &snap {
            Snap::Missing => (Vec::new(), false),
            Snap::File(_) | Snap::TooLarge => match self.lister.list(path) {
                Ok(listing) => (code_keys(&listing), false),
                Err(_) => (Vec::new(), true),
            },
            Snap::Dir | Snap::Link(_) | Snap::Other => (Vec::new(), true),
        };
        ConfigState { snap, keys, unreadable }
    }

    fn hooks(&self, dir: &Path) -> Hooks {
        match self.fs.lstat(dir) {
            Ok(FileKind::Dir) => Hooks::Entries(
                self.names(dir)
                    .into_iter()
                    .map(|name| {
                        let snap = self.snap(&dir.join(&name));
                        (name.to_string_lossy().into_owned(), snap)
                    })
                    .collect(),
            ),
            Ok(FileKind::Symlink) => Hooks::Link(self.fs.read_link(dir).unwrap_or_default()),
            _ => Hooks::Missing,
        }
    }

    fn names(&self, dir: &Path) -> Vec<OsString> {
        if self.fs.lstat(dir).ok() != Some(FileKind::Dir) {
            return Vec::new();
        }
        let mut names = self.fs.read_dir(dir).unwrap_or_default();
        names.sort();
        names
    }

    fn repo(&self, target: &GitDirTarget, protected: &[String]) -> RepoState {
        let dir = &target.git_dir;
        let worktrees = self
            .names(&dir.join("worktrees"))
            .into_iter()
            .map(|name| {
                let at = dir.join("worktrees").join(&name);
                let state = WorktreeState {
                    commondir: self.snap(&at.join("commondir")),
                    config_worktree: self.config(&at.join("config.worktree")),
                };
                (name.to_string_lossy().into_owned(), state)
            })
            .collect();
        let modules = self
            .names(&dir.join("modules"))
            .into_iter()
            .map(|name| {
                let at = dir.join("modules").join(&name);
                let state = ModuleState {
                    config: self.config(&at.join("config")),
                    hooks: self.hooks(&at.join("hooks")),
                };
                (name.to_string_lossy().into_owned(), state)
            })
            .collect();
        let protected = match &target.work_tree {
            Some(root) => protected
                .iter()
                .map(|name| name.trim_end_matches('/'))
                .filter(|name| self.fs.lstat(&root.join(name)).is_ok())
                .map(str::to_owned)
                .collect(),
            None => BTreeSet::new(),
        };
        RepoState {
            main: target.main,
            work_tree: target.work_tree.clone(),
            commondir: self.snap(&dir.join("commondir")),
            config: self.config(&dir.join("config")),
            config_worktree: self.config(&dir.join("config.worktree")),
            hooks: self.hooks(&dir.join("hooks")),
            worktrees,
            modules,
            alternates: self.snap(&dir.join("objects/info/alternates")),
            protected,
        }
    }
}

impl SurfaceManifest {
    /// Records the git surface of `targets`; `protected` are the names that count when
    /// a call creates them at a repository root.
    pub fn capture(
        targets: &[GitDirTarget],
        protected: &[String],
        fs: &dyn FsView,
        lister: &dyn ConfigLister,
    ) -> SurfaceManifest {
        let capture = Capture { fs, lister };
        let repos = targets
            .iter()
            .map(|target| (target.git_dir.clone(), capture.repo(target, protected)))
            .collect();
        SurfaceManifest { repos }
    }

    /// The git dirs it holds.
    pub fn git_dirs(&self) -> Vec<&Path> {
        self.repos.keys().map(PathBuf::as_path).collect()
    }
}

/// The changes between `before` and `after` that break a rule of the guard, in a
/// stable order. A git dir that only `after` holds is new: every rule applies to it.
pub fn check_surface(before: &SurfaceManifest, after: &SurfaceManifest) -> Vec<SurfaceChange> {
    let mut changes = Vec::new();
    let mut flag = |path: PathBuf, rule: SurfaceRule, key: Option<String>| {
        changes.push(SurfaceChange {
            path,
            rule: rule.as_str().to_owned(),
            key,
            quarantined: rule.quarantines(),
        });
    };
    for (dir, now) in &after.repos {
        let was = before.repos.get(dir);
        let changed_snap =
            |pick: fn(&RepoState) -> &Snap| was.is_none_or(|was| pick(was) != pick(now));
        if now.main {
            if now.commondir.exists() && changed_snap(|repo| &repo.commondir) {
                flag(dir.join("commondir"), SurfaceRule::CommondirInMainGitDir, None);
            }
            let appeared = was.is_none_or(|was| !was.config_worktree.snap.exists());
            if now.config_worktree.snap.exists() && appeared {
                flag(dir.join("config.worktree"), SurfaceRule::ConfigWorktreeAppeared, None);
            }
        } else {
            if now.commondir.exists() && changed_snap(|repo| &repo.commondir) {
                let own = now.work_tree.as_deref().unwrap_or(dir);
                if !points_within(&now.commondir, dir, own) {
                    flag(dir.join("commondir"), SurfaceRule::CommondirOutsideRepo, None);
                }
            }
            config_rule(
                &mut flag,
                &dir.join("config.worktree"),
                was.map(|was| &was.config_worktree),
                &now.config_worktree,
            );
        }
        config_rule(&mut flag, &dir.join("config"), was.map(|was| &was.config), &now.config);
        hooks_rule(
            &mut flag,
            &dir.join("hooks"),
            was.map(|was| &was.hooks),
            &now.hooks,
            SurfaceRule::HookPlanted,
        );
        for (name, tree) in &now.worktrees {
            let at = dir.join("worktrees").join(name);
            let old = was.and_then(|was| was.worktrees.get(name));
            if tree.commondir.exists()
                && old.is_none_or(|old| old.commondir != tree.commondir)
                && !points_to(&tree.commondir, &at, dir)
            {
                flag(at.join("commondir"), SurfaceRule::WorktreeCommondirElsewhere, None);
            }
            let appeared = old.is_none_or(|old| !old.config_worktree.snap.exists());
            if tree.config_worktree.snap.exists() && appeared {
                flag(at.join("config.worktree"), SurfaceRule::WorktreeConfigWorktreeAppeared, None);
            }
        }
        for (name, module) in &now.modules {
            let at = dir.join("modules").join(name);
            let old = was.and_then(|was| was.modules.get(name));
            if old.is_none_or(|old| old.config != module.config) && has_code(&module.config) {
                flag(at.join("config"), SurfaceRule::ModuleCodeKey, joined(&module.config.keys));
            }
            hooks_rule(
                &mut flag,
                &at.join("hooks"),
                old.map(|old| &old.hooks),
                &module.hooks,
                SurfaceRule::ModuleHook,
            );
        }
        if now.alternates.exists() && changed_snap(|repo| &repo.alternates) {
            flag(dir.join("objects/info/alternates"), SurfaceRule::AlternatesChanged, None);
        }
        if let Some(root) = &now.work_tree {
            for name in &now.protected {
                if was.is_none_or(|was| !was.protected.contains(name)) {
                    flag(root.join(name), SurfaceRule::ProtectedNameCreated, None);
                }
            }
        }
    }
    changes
}

fn has_code(config: &ConfigState) -> bool {
    config.unreadable || !config.keys.is_empty()
}

fn joined(keys: &[String]) -> Option<String> {
    (!keys.is_empty()).then(|| keys.join(", "))
}

fn config_rule(
    flag: &mut impl FnMut(PathBuf, SurfaceRule, Option<String>),
    path: &Path,
    was: Option<&ConfigState>,
    now: &ConfigState,
) {
    if was.is_some_and(|was| was == now) || !now.snap.exists() {
        return;
    }
    if now.unreadable {
        flag(path.to_path_buf(), SurfaceRule::ConfigUnreadable, None);
    } else if !now.keys.is_empty() {
        flag(path.to_path_buf(), SurfaceRule::ConfigCodeKey, joined(&now.keys));
    }
}

fn hooks_rule(
    flag: &mut impl FnMut(PathBuf, SurfaceRule, Option<String>),
    dir: &Path,
    was: Option<&Hooks>,
    now: &Hooks,
    rule: SurfaceRule,
) {
    match now {
        Hooks::Missing => {}
        Hooks::Link(_) => {
            if was != Some(now) {
                flag(dir.to_path_buf(), rule, None);
            }
        }
        Hooks::Entries(entries) => {
            let old = match was {
                Some(Hooks::Entries(old)) => Some(old),
                _ => None,
            };
            for (name, snap) in entries {
                let changed = old.is_none_or(|old| old.get(name) != Some(snap));
                if changed && !name.ends_with(".sample") {
                    flag(dir.join(name), rule, None);
                }
            }
        }
    }
}

/// True when the `commondir` text `snap`, read relative to `base`, names `target`.
fn points_to(snap: &Snap, base: &Path, target: &Path) -> bool {
    resolve_text(snap, base).is_some_and(|path| path == target)
}

/// True when the `commondir` text `snap`, read relative to `base`, lies in `root`.
fn points_within(snap: &Snap, base: &Path, root: &Path) -> bool {
    resolve_text(snap, base).is_some_and(|path| is_within(&path, root))
}

fn resolve_text(snap: &Snap, base: &Path) -> Option<PathBuf> {
    let text = snap.text()?;
    normalize(&base.join(text))
}

/// One entry of the turn-end report: `relative`, a changed file that matches
/// `sandbox.surface_files`, with what in its `content` runs code: the code keys of a
/// `.cargo/config.toml` or `.cargo/config`, `scripts` of a `package.json`.
pub fn report_file(relative: &Path, content: &[u8]) -> ReportedFile {
    let text = String::from_utf8_lossy(content);
    let name = relative.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let in_cargo = relative.parent().and_then(Path::file_name).is_some_and(|dir| dir == ".cargo");
    let detail = if in_cargo && (name == "config.toml" || name == "config") {
        let keys = cargo_code_keys(&text);
        (!keys.is_empty()).then(|| keys.join(", "))
    } else if name == "package.json" && text.contains("\"scripts\"") {
        Some("scripts".to_owned())
    } else {
        None
    };
    ReportedFile { path: relative.to_path_buf(), detail }
}

/// The limits of the scan for nested git dirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    /// The deepest level below a root that the scan visits.
    pub depth: usize,
    /// The most entries the scan looks at, across all roots.
    pub entries: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        ScanLimits { depth: 4, entries: 20_000 }
    }
}

/// Directories the scan never enters: builds make them, and they are large.
pub const SCAN_SKIP: &[&str] = &["target", "node_modules", ".venv", "dist", "build", ".git"];

/// The git dirs the guard records: the pinned ones, every `.git` directory in
/// `roots` down to the depth limit, and the `.git` of each directory in `also` (the
/// final cwd and its parents). The second value is true when the scan stopped at its
/// entry limit.
pub fn scan_git_dirs(
    roots: &[PathBuf],
    also: &[PathBuf],
    pinned: &[PathBuf],
    fs: &dyn FsView,
    limits: ScanLimits,
) -> (Vec<GitDirTarget>, bool) {
    let mut found: BTreeMap<PathBuf, GitDirTarget> = pinned
        .iter()
        .map(|dir| {
            let target = GitDirTarget {
                git_dir: dir.clone(),
                main: true,
                work_tree: dir.parent().map(Path::to_path_buf).filter(|_| dir.ends_with(".git")),
            };
            (dir.clone(), target)
        })
        .collect();
    let mut seen = 0_usize;
    let mut truncated = false;
    let add = |found: &mut BTreeMap<PathBuf, GitDirTarget>, dir: &Path| {
        let git_dir = dir.join(".git");
        if fs.lstat(&git_dir).ok() == Some(FileKind::Dir) && !found.contains_key(&git_dir) {
            let target = GitDirTarget {
                git_dir: git_dir.clone(),
                main: false,
                work_tree: Some(dir.to_path_buf()),
            };
            found.insert(git_dir, target);
        }
    };
    'roots: for root in roots {
        let mut level: Vec<PathBuf> = vec![root.clone()];
        for _ in 0..=limits.depth {
            let mut next = Vec::new();
            for dir in level {
                add(&mut found, &dir);
                // NOTE: the listing's own kinds: an lstat per entry made the scan of a
                // project of 8,000 entries cost 24 ms, and it runs twice per call.
                for (name, kind) in fs.read_dir_kinds(&dir).unwrap_or_default() {
                    seen += 1;
                    if seen > limits.entries {
                        truncated = true;
                        break 'roots;
                    }
                    if SCAN_SKIP.iter().any(|skip| name == *skip) {
                        continue;
                    }
                    let child = dir.join(&name);
                    let kind = kind.or_else(|| fs.lstat(&child).ok());
                    if kind == Some(FileKind::Dir) {
                        next.push(child);
                    }
                }
            }
            level = next;
        }
    }
    for dir in also {
        add(&mut found, dir);
    }
    (found.into_values().collect(), truncated)
}

#[cfg(test)]
mod tests;
