//! [`MountPlan::build`]: the rules of the module docs, applied to one spec.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, CacheMode, DesktopKind, Grant};

use crate::SandboxError;
use crate::desktop::{desktop_socket_path, is_desktop_socket};
use crate::fs_view::{FileKind, FsView, Memo, Resolved, resolve};
use crate::inner::InnerForward;
use crate::layers::{CacheLayer, CacheLayers, STAGING_DIR, moved_mounts};
use crate::modules::{self, ConfigPath, config_paths, module_work_tree};
use crate::mounts::{MOUNTS_FILE, SIZES_FILE};
use crate::paths::{depth, is_within, normalize, too_wide};
use crate::plan::{
    Mount, MountOp, MountOrigin, MountPlan, NO_PROXY, NO_PROXY_VARIABLES, OpKind, PROXY_URL,
    PROXY_VARIABLES, PlanNote, SERVICE_SOCKETS, SYSTEM_BUS_SOCKET,
};
use crate::spec::{DesktopSocket, FloorKind, MaskKind, NetworkPlan, SandboxSpec, WriteRootKind};
use crate::surface::{ScanLimits, scan_git_dirs};

/// The most bytes of a git config that the plan reads for its include paths.
const MAX_GIT_CONFIG: usize = 1024 * 1024;

/// A mask, resolved.
struct PlannedMask {
    path: PathBuf,
    kind: MaskKind,
}

/// A writable directory or file, resolved.
struct Writable {
    path: PathBuf,
    kind: WriteRootKind,
    dir: bool,
}

struct Builder<'a> {
    spec: &'a SandboxSpec,
    fs: &'a dyn FsView,
    mounts: Vec<Mount>,
    notes: Vec<PlanNote>,
    masks: Vec<PlannedMask>,
    roots: Vec<Writable>,
    widenings: Vec<Writable>,
    caches: Vec<PathBuf>,
    layer_dirs: Vec<PathBuf>,
    resolve_unix: Vec<PathBuf>,
    devices: Vec<PathBuf>,
    env: Vec<(String, String)>,
    proxy_socket: Option<PathBuf>,
    forward: Option<InnerForward>,
}

impl MountPlan {
    /// Checks `spec` and turns it into the ordered mounts of one contained call,
    /// reading the file system only through `fs`.
    pub fn build(spec: &SandboxSpec, fs: &dyn FsView) -> Result<MountPlan, SandboxError> {
        spec.check()?;
        // A grant of a newer efr: this build cannot know what it widens, so the call
        // fails closed.
        if spec.grants.iter().any(|grant| matches!(grant, Grant::Unknown)) {
            return Err(SandboxError::UnknownGrant);
        }
        // NOTE: efrd adds the network view only beside `Write` and `Unmask` grants
        // (efr-conversation's `view_fits`). The plan checks it again, so no spec puts
        // another grant into the host's network namespace.
        let net_view = spec.grants.iter().any(|grant| matches!(grant, Grant::NetView));
        if net_view
            && !spec.grants.iter().all(|grant| {
                matches!(grant, Grant::NetView | Grant::Write { .. } | Grant::Unmask { .. })
            })
        {
            return Err(SandboxError::NetViewMixed);
        }
        // NOTE: one plan resolves each shared dir of its hundred paths once, not each
        // time; the plan cost the launcher 2 ms per call before.
        let memo = Memo::new(fs);
        let fs: &dyn FsView = &memo;
        let path_dirs = path_entries(&spec.shell_path)?;
        let mut builder = Builder {
            spec,
            fs,
            mounts: Vec::new(),
            notes: Vec::new(),
            masks: Vec::new(),
            roots: Vec::new(),
            widenings: Vec::new(),
            caches: Vec::new(),
            layer_dirs: Vec::new(),
            resolve_unix: Vec::new(),
            devices: Vec::new(),
            env: Vec::new(),
            proxy_socket: None,
            forward: None,
        };
        builder.masks()?;
        builder.roots()?;
        builder.check_roots()?;
        builder.caches()?;
        builder.private_tmp()?;
        builder.pins()?;
        for floor in &spec.floors {
            builder.floor(&floor.path, floor.kind)?;
        }
        for dir in &path_dirs {
            builder.floor(dir, FloorKind::PathDir)?;
        }
        builder.job_floors()?;
        builder.assets()?;
        builder.sockets_and_devices()?;
        builder.proxy()?;
        builder.forward()?;
        builder.bus_filter()?;
        Ok(builder.finish())
    }
}

/// The entries of the hidden shell's `PATH`; an empty or relative entry refuses the
/// call, because it names a directory that depends on the working directory.
fn path_entries(path: &str) -> Result<Vec<PathBuf>, SandboxError> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    path.split(':')
        .map(|entry| {
            normalize(Path::new(entry))
                .ok_or_else(|| SandboxError::RelativePathEntry { entry: entry.to_owned() })
        })
        .collect()
}

impl Builder<'_> {
    fn resolve(&self, path: &Path) -> Result<Resolved, SandboxError> {
        resolve(self.fs, path)
    }

    fn push(&mut self, op: MountOp, kind: OpKind, origin: MountOrigin) {
        self.mounts.push(Mount { op, kind, origin });
    }

    /// True when `path` lies in a place that a call may write: a write root, a
    /// widening or a cache overlay. Only there does a floor need a mount.
    fn writable(&self, path: &Path) -> bool {
        self.roots.iter().chain(&self.widenings).any(|root| is_within(path, &root.path))
            || self.caches.iter().any(|cache| is_within(path, cache))
    }

    fn masked_by(&self, path: &Path) -> Option<&PlannedMask> {
        self.masks.iter().find(|mask| is_within(path, &mask.path))
    }

    /// The mask that hides `path` inside the sandbox. Scratch lies below efr's masked
    /// data root and comes back as a write root, so a path in scratch is not hidden by
    /// that mask; `$SCRATCH/jobs` is such a floor.
    fn hides(&self, path: &Path) -> Option<&PlannedMask> {
        self.masks.iter().find(|mask| {
            is_within(path, &mask.path)
                && !self.roots.iter().any(|root| {
                    root.kind == WriteRootKind::Scratch
                        && mask.kind == MaskKind::EfrState
                        && is_within(path, &root.path)
                        && is_within(&root.path, &mask.path)
                })
        })
    }

    fn mask(
        &mut self,
        path: &Path,
        kind: MaskKind,
        origin: MountOrigin,
        perms: u32,
    ) -> Result<(), SandboxError> {
        let resolved = self.resolve(path)?;
        if !resolved.exists {
            self.notes.push(PlanNote::MissingMask(path.to_path_buf()));
            return Ok(());
        }
        let op = match resolved.kind {
            Some(FileKind::Dir) => MountOp::Tmpfs { target: resolved.path.clone(), perms },
            _ => MountOp::DevNull { target: resolved.path.clone() },
        };
        self.push(op, OpKind::Mask, origin);
        self.masks.push(PlannedMask { path: resolved.path, kind });
        Ok(())
    }

    /// The runtime dirs, efr's own roots and the spec's masks, minus the unmask
    /// grants.
    fn masks(&mut self) -> Result<(), SandboxError> {
        let runtime = &self.spec.runtime;
        self.mask(&runtime.user_runtime, MaskKind::EfrState, MountOrigin::Runtime, 0o700)?;
        if !is_within(&runtime.runtime, &runtime.user_runtime) {
            self.mask(&runtime.runtime, MaskKind::EfrState, MountOrigin::Runtime, 0o700)?;
        }
        for root in [&runtime.data, &runtime.state] {
            self.mask(root, MaskKind::EfrState, MountOrigin::EfrState, 0o500)?;
        }
        let unmasked: Vec<&Path> = self
            .spec
            .grants
            .iter()
            .filter_map(|grant| match grant {
                Grant::Unmask { path } => Some(path.as_path()),
                _ => None,
            })
            .collect();
        let mut used = vec![false; unmasked.len()];
        for mask in &self.spec.masks {
            if let Some(at) = unmasked.iter().position(|path| *path == mask.path) {
                if mask.kind == MaskKind::EngineSecret {
                    return Err(SandboxError::UnmaskSecret { path: mask.path.clone() });
                }
                used[at] = true;
                continue;
            }
            self.mask(&mask.path, mask.kind, MountOrigin::Mask(mask.kind), 0o500)?;
        }
        for (path, used) in unmasked.iter().zip(used) {
            if !used {
                self.notes.push(PlanNote::UnmaskUnused(path.to_path_buf()));
            }
        }
        Ok(())
    }

    /// The write roots of the spec, scratch and the write grants, resolved and checked
    /// against the home directory.
    fn roots(&mut self) -> Result<(), SandboxError> {
        let home = &self.spec.runtime.home;
        let scratch = crate::spec::WriteRoot {
            path: self.spec.runtime.scratch.clone(),
            kind: WriteRootKind::Scratch,
        };
        for root in self.spec.write_roots.iter().chain([&scratch]) {
            let resolved = self.resolve(&root.path)?;
            if too_wide(&root.path, home) || too_wide(&resolved.path, home) {
                return Err(SandboxError::RootTooWide { path: root.path.clone() });
            }
            if resolved.kind != Some(FileKind::Dir) {
                self.notes.push(PlanNote::MissingRoot(root.path.clone()));
                continue;
            }
            if !self.roots.iter().any(|known| known.path == resolved.path) {
                self.roots.push(Writable { path: resolved.path, kind: root.kind, dir: true });
            }
        }
        for grant in &self.spec.grants {
            let Grant::Write { path } = grant else { continue };
            let resolved = self.resolve(path)?;
            if too_wide(path, home) || too_wide(&resolved.path, home) {
                return Err(SandboxError::RootTooWide { path: path.clone() });
            }
            if !matches!(resolved.kind, Some(FileKind::Dir | FileKind::File)) {
                return Err(SandboxError::GrantTarget { path: path.clone() });
            }
            let dir = resolved.kind == Some(FileKind::Dir);
            self.widenings.push(Writable { path: resolved.path, kind: WriteRootKind::Grant, dir });
        }
        Ok(())
    }

    /// Refuses a write root or a widening at or inside a mask or a floor. Scratch lies
    /// below efr's data root on purpose: the root is masked and scratch comes back.
    fn check_roots(&mut self) -> Result<(), SandboxError> {
        let data = self.resolve(&self.spec.runtime.data)?.path;
        for root in self.roots.iter().chain(&self.widenings) {
            for mask in &self.masks {
                let scratch_in_data = root.kind == WriteRootKind::Scratch
                    && mask.kind == MaskKind::EfrState
                    && mask.path == data;
                if is_within(&root.path, &mask.path) && !scratch_in_data {
                    return Err(SandboxError::RootUnderMask {
                        root: root.path.clone(),
                        mask: mask.path.clone(),
                    });
                }
            }
            for floor in &self.spec.floors {
                let resolved = self.resolve(&floor.path)?.path;
                if is_within(&root.path, &floor.path) || is_within(&root.path, &resolved) {
                    return Err(SandboxError::RootUnderFloor {
                        root: root.path.clone(),
                        floor: floor.path.clone(),
                    });
                }
            }
        }
        let roots: Vec<(PathBuf, WriteRootKind)> =
            self.roots.iter().map(|root| (root.path.clone(), root.kind)).collect();
        for (path, kind) in roots {
            let op = MountOp::Bind { source: path.clone(), target: path, writable: true };
            self.push(op, OpKind::WriteRoot, MountOrigin::WriteRoot(kind));
        }
        let widenings: Vec<PathBuf> = self.widenings.iter().map(|root| root.path.clone()).collect();
        for path in widenings {
            let op = MountOp::Bind { source: path.clone(), target: path, writable: true };
            self.push(op, OpKind::Widening, MountOrigin::WriteRoot(WriteRootKind::Grant));
        }
        Ok(())
    }

    /// The cache overlays and their read-only pins.
    fn caches(&mut self) -> Result<(), SandboxError> {
        if self.spec.cache_mode == CacheMode::Readonly {
            return Ok(());
        }
        // NOTE: the outermost cache comes first, whatever the order of the list or the
        // links on the way: a cache inside another one gets no overlay of its own, or
        // the helper would have to move an overlay that it has not mounted yet.
        let mut caches = Vec::with_capacity(self.spec.caches.len());
        for cache in &self.spec.caches {
            caches.push((self.resolve(&cache.target)?, cache));
        }
        caches.sort_by_key(|(resolved, _)| depth(&resolved.path));
        for (resolved, cache) in caches {
            if self.caches.iter().any(|known| is_within(&resolved.path, known)) {
                // Its pins still lie in a writable place: the outer overlay.
                self.notes.push(PlanNote::CacheSkipped(cache.target.clone()));
                for pin in &cache.pins {
                    self.floor(pin, FloorKind::ToolConfig)?;
                }
                continue;
            }
            let in_root = self.writable(&resolved.path);
            // NOTE: the helper finds the lower layer inside the sandbox, where these
            // dirs are private ones, not the host's.
            let private = ["/tmp", "/var/tmp", "/dev"]
                .iter()
                .any(|dir| is_within(&resolved.path, Path::new(dir)));
            if resolved.kind != Some(FileKind::Dir)
                || in_root
                || private
                || self.masked_by(&resolved.path).is_some()
            {
                self.notes.push(PlanNote::CacheSkipped(cache.target.clone()));
                continue;
            }
            let target = resolved.path;
            // NOTE: a kept cache of the `tmp` mode (`sandbox.keep_downloads`) mounts on
            // its private layer as in the `overlay` mode; the other caches stay `tmp`.
            let keep = cache.keeps_layer(self.spec.cache_mode);
            if keep {
                // NOTE: the helper binds the layer dir and finds upper and work in it; a
                // dir that two overlays share would be one upper for both.
                let layer = cache.upper.parent();
                let shared = self.layer_dirs.iter().any(|known| Some(known.as_path()) == layer);
                match layer {
                    Some(layer) if cache.work.parent() == Some(layer) && !shared => {
                        self.layer_dirs.push(layer.to_path_buf());
                    }
                    _ => return Err(SandboxError::CacheLayer { cache: cache.target.clone() }),
                }
            }
            let op = if keep {
                MountOp::Overlay {
                    lower: target.clone(),
                    upper: cache.upper.clone(),
                    work: cache.work.clone(),
                    target: target.clone(),
                }
            } else {
                MountOp::TmpOverlay { lower: target.clone(), target: target.clone() }
            };
            self.push(op, OpKind::Overlay, MountOrigin::Cache);
            self.caches.push(target);
            for pin in &cache.pins {
                self.floor(pin, FloorKind::ToolConfig)?;
            }
        }
        Ok(())
    }

    /// The private `/tmp` and `/var/tmp` (two binds, two descriptors) and `/dev/shm`.
    fn private_tmp(&mut self) -> Result<(), SandboxError> {
        // The private tmp lies in efr's state root, which a sandboxed call cannot
        // change, so a link on the way (a linked ~/.local) is followed.
        let resolved = self.resolve(&self.spec.runtime.private_tmp())?;
        if resolved.kind != Some(FileKind::Dir) {
            return Err(SandboxError::MissingAsset { path: self.spec.runtime.private_tmp() });
        }
        let source = resolved.path;
        for target in ["/tmp", "/var/tmp"] {
            let op =
                MountOp::Bind { source: source.clone(), target: target.into(), writable: true };
            self.push(op, OpKind::WriteRoot, MountOrigin::PrivateTmp);
        }
        let op = MountOp::Tmpfs { target: "/dev/shm".into(), perms: 0o1777 };
        self.push(op, OpKind::WriteRoot, MountOrigin::SharedMemory);
        Ok(())
    }

    /// The top `.git` of each project root and the registered git dirs: pinned, with
    /// their config, hooks, includes and hooks path read-only. A `.git` file is a floor.
    fn pins(&mut self) -> Result<(), SandboxError> {
        let mut pins: Vec<PathBuf> = Vec::new();
        let projects: Vec<PathBuf> = self
            .roots
            .iter()
            .filter(|root| {
                matches!(root.kind, WriteRootKind::TurnProject | WriteRootKind::NamedProject)
            })
            .map(|root| root.path.clone())
            .collect();
        for root in projects {
            let dot_git = root.join(".git");
            match self.lstat(&dot_git)? {
                Some(FileKind::Dir) => pins.push(dot_git),
                Some(FileKind::File) => self.floor(&dot_git, FloorKind::GitFile)?,
                Some(FileKind::Symlink) => {
                    return Err(SandboxError::SymlinkedPin { path: dot_git });
                }
                Some(FileKind::Other) | None => {}
            }
        }
        for dir in &self.spec.git_dirs {
            let resolved = self.resolve(dir)?;
            if resolved.links.iter().any(|link| self.writable(link)) {
                return Err(SandboxError::SymlinkedPin { path: dir.clone() });
            }
            // NOTE: a pin is a writable bind, so a git dir outside every write root
            // would widen the call; efrd adds the registered git dirs as roots.
            let in_root = self.roots.iter().any(|root| is_within(&resolved.path, &root.path));
            if resolved.kind == Some(FileKind::Dir) && in_root {
                pins.push(resolved.path);
            } else {
                self.notes.push(PlanNote::MissingRoot(dir.clone()));
            }
        }
        pins.dedup();
        for pin in pins {
            self.pin(&pin)?;
        }
        Ok(())
    }

    fn lstat(&self, path: &Path) -> Result<Option<FileKind>, SandboxError> {
        match self.fs.lstat(path) {
            Ok(kind) => Ok(Some(kind)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(SandboxError::Io { path: path.to_path_buf(), source }),
        }
    }

    fn pin(&mut self, git_dir: &Path) -> Result<(), SandboxError> {
        let op = MountOp::Bind {
            source: git_dir.to_path_buf(),
            target: git_dir.to_path_buf(),
            writable: true,
        };
        self.push(op, OpKind::WriteRoot, MountOrigin::Pin);
        self.git_surface(git_dir)?;
        let worktrees = git_dir.join("worktrees");
        match self.lstat(&worktrees)? {
            Some(FileKind::Dir) => {
                let mut names = self
                    .fs
                    .read_dir(&worktrees)
                    .map_err(|source| SandboxError::Io { path: worktrees.clone(), source })?;
                names.sort();
                for name in names {
                    let entry = worktrees.join(name);
                    match self.lstat(&entry)? {
                        Some(FileKind::Dir) => self.git_surface(&entry)?,
                        Some(FileKind::Symlink) => {
                            return Err(SandboxError::SymlinkedPin { path: entry });
                        }
                        _ => {}
                    }
                }
            }
            Some(FileKind::Symlink) => {
                return Err(SandboxError::SymlinkedPin { path: worktrees });
            }
            _ => {}
        }
        let work_tree = git_dir.parent().unwrap_or(git_dir).to_path_buf();
        let mut trees: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        let text = self.config_text(git_dir)?;
        self.config_floors(git_dir, &text, Some(&work_tree))?;
        trees.push((git_dir.to_path_buf(), Some(work_tree)));
        // NOTE: git names a module's git dir after its path (`modules/vendor/foo`) and
        // nests the modules of a module (`modules/a/modules/b`), so the pins follow
        // every depth. A tree deeper than the walk's limit keeps the pins of what the
        // walk found: the guard then records `modules too deep`, and efrd's check of
        // an outside git line says that it could not check the repository.
        let walk = modules::walk(git_dir, self.fs);
        if let Some(path) = walk.links.into_iter().next() {
            return Err(SandboxError::SymlinkedPin { path });
        }
        if let Some((path, source)) = walk.unreadable {
            return Err(SandboxError::Io { path, source });
        }
        for module in walk.git_dirs {
            self.git_surface(&module)?;
            let text = self.config_text(&module)?;
            let tree = module_work_tree(&module, &text, &trees, self.fs)?;
            self.config_floors(&module, &text, tree.as_deref())?;
            trees.push((module, tree));
        }
        Ok(())
    }

    /// The text of `git_dir/config`, empty when it is not a file.
    fn config_text(&self, git_dir: &Path) -> Result<String, SandboxError> {
        let config = git_dir.join("config");
        if self.lstat(&config)? != Some(FileKind::File) {
            return Ok(String::new());
        }
        let bytes = self
            .fs
            .read_file(&config, MAX_GIT_CONFIG)
            .map_err(|source| SandboxError::Io { path: config.clone(), source })?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The include files and the hooks path that the config `text` of `git_dir` names,
    /// read-only where a call can write them.
    fn config_floors(
        &mut self,
        git_dir: &Path,
        text: &str,
        work_tree: Option<&Path>,
    ) -> Result<(), SandboxError> {
        let home = self.spec.runtime.home.clone();
        for named in config_paths(text, git_dir, work_tree, Some(&home)) {
            let (path, kind) = match named {
                ConfigPath::Include(path) => (path, FloorKind::GitConfig),
                ConfigPath::HooksPath(path) => (path, FloorKind::GitHooks),
            };
            if self.writable(&path) {
                self.floor(&path, kind)?;
            }
        }
        Ok(())
    }

    /// `config`, `config.worktree` and `hooks` of one git dir, read-only.
    fn git_surface(&mut self, git_dir: &Path) -> Result<(), SandboxError> {
        for (name, kind) in [
            ("config", FloorKind::GitConfig),
            ("config.worktree", FloorKind::GitConfig),
            ("hooks", FloorKind::GitHooks),
        ] {
            let path = git_dir.join(name);
            if self.lstat(&path)? == Some(FileKind::Symlink) {
                return Err(SandboxError::SymlinkedFloor { link: path.clone(), path });
            }
            self.floor(&path, kind)?;
        }
        Ok(())
    }

    /// One floor: resolved, refused when a link on the way lies where a call can write,
    /// skipped when missing or outside every writable place, dropped below a mask.
    fn floor(&mut self, path: &Path, kind: FloorKind) -> Result<(), SandboxError> {
        let resolved = self.resolve(path)?;
        if let Some(link) = resolved.links.iter().find(|link| self.writable(link)) {
            return Err(SandboxError::SymlinkedFloor {
                path: path.to_path_buf(),
                link: link.clone(),
            });
        }
        if !resolved.exists {
            self.notes.push(PlanNote::MissingFloor(path.to_path_buf()));
            return Ok(());
        }
        if !self.writable(&resolved.path) {
            return Ok(());
        }
        if let Some(mask) = self.hides(&resolved.path) {
            let note =
                PlanNote::FloorUnderMask { floor: path.to_path_buf(), mask: mask.path.clone() };
            self.notes.push(note);
            return Ok(());
        }
        let op =
            MountOp::Bind { source: resolved.path.clone(), target: resolved.path, writable: false };
        self.push(op, OpKind::Floor, MountOrigin::Floor(kind));
        Ok(())
    }

    /// The floors of jobs. `$SCRATCH/jobs`, where efrd writes the job outputs, is
    /// read-only for every call once efrd made it as a dir; a link or a file there is
    /// not efrd's, and efrd moves it away before it makes the dir. A job also gets every
    /// git dir that it finds at its start read-only whole: the pinned ones and each one
    /// that the surface guard's scan finds in the guard roots, so a job that runs for
    /// hours cannot plant a hook or a config in a known repository.
    fn job_floors(&mut self) -> Result<(), SandboxError> {
        let outputs = self.spec.runtime.job_outputs();
        if self.lstat(&outputs)? == Some(FileKind::Dir) {
            self.floor(&outputs, FloorKind::JobOutput)?;
        }
        if self.spec.job.is_none() {
            return Ok(());
        }
        let pins: Vec<PathBuf> = self
            .mounts
            .iter()
            .filter(|mount| mount.origin == MountOrigin::Pin)
            .map(|mount| mount.op.target().to_path_buf())
            .collect();
        let (targets, _) =
            scan_git_dirs(&self.spec.guard_roots, &[], &pins, self.fs, ScanLimits::default());
        for target in targets {
            self.floor(&target.git_dir, FloorKind::JobGitDir)?;
        }
        Ok(())
    }

    /// The launcher, the child script, the shell snapshot and state, the line, the
    /// snapshot of the host's mounts (only for a line of the mount view) and the
    /// editor stub, read-only inside the masked runtime dirs.
    fn assets(&mut self) -> Result<(), SandboxError> {
        let runtime = &self.spec.runtime;
        let inside = runtime.inside_dir();
        let assets: Vec<(PathBuf, PathBuf, bool)> = vec![
            (runtime.launcher.clone(), runtime.inside_launcher(), true),
            (runtime.child_script.clone(), runtime.inside_child_script(), true),
            (runtime.shell_dir.join("snapshot.zsh"), inside.join("snapshot.zsh"), false),
            (runtime.shell_dir.join("snapshot.zsh.zwc"), inside.join("snapshot.zsh.zwc"), false),
            (runtime.shell_dir.join("state.zsh"), inside.join("state.zsh"), false),
            (runtime.call_dir.join(crate::result::LINE_FILE), inside.join("line"), true),
            (runtime.call_dir.join(MOUNTS_FILE), inside.join(MOUNTS_FILE), false),
            (runtime.call_dir.join(SIZES_FILE), inside.join(SIZES_FILE), false),
            (runtime.editor.clone(), runtime.editor.clone(), false),
        ];
        for (source, target, required) in assets {
            // The launcher's files lie in efr's runtime root, out of the sandbox's
            // reach, so a link on the way is followed.
            let resolved = self.resolve(&source)?;
            match resolved.kind {
                Some(FileKind::File) => {
                    let op = MountOp::Bind { source: resolved.path, target, writable: false };
                    self.push(op, OpKind::Floor, MountOrigin::Asset);
                }
                _ if required => return Err(SandboxError::MissingAsset { path: source }),
                _ => {}
            }
        }
        Ok(())
    }

    /// The bridge's view of a proxy plan: the call's socket bound read-only at
    /// `$XRT/efr-proxy.sock`, and the variables that point every tool at the bridge.
    ///
    /// The child gets no `RESOLVE_UNIX` rule for the socket: only the bridge, in a
    /// Landlock domain of its own, connects to it. The bind is read-only, so the child
    /// cannot `chmod` the host's inode (Landlock has no right for that); `connect()`
    /// needs no writable mount. A grant of the whole network refuses the plan: the call
    /// would keep the host's network namespace (`SandboxSpec::check` refuses it too).
    fn proxy(&mut self) -> Result<(), SandboxError> {
        let spec = self.spec;
        let NetworkPlan::Proxy { socket } = &spec.network else { return Ok(()) };
        if spec.grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork)) {
            return Err(SandboxError::ProxyWithOpenNetwork);
        }
        if *socket != spec.runtime.proxy_socket() {
            return Err(SandboxError::ProxySocket { path: socket.clone() });
        }
        // efrd's call dir lies in efr's runtime root, out of the sandbox's reach, so a
        // link on the way is followed, as for the launcher's other files.
        let resolved = self.resolve(socket)?;
        if resolved.kind != Some(FileKind::Other) {
            return Err(SandboxError::ProxySocket { path: socket.clone() });
        }
        let target = spec.runtime.inside_proxy_socket();
        let op = MountOp::Bind { source: resolved.path, target: target.clone(), writable: false };
        self.push(op, OpKind::Floor, MountOrigin::Asset);
        self.proxy_socket = Some(target);
        for name in PROXY_VARIABLES {
            self.env.push(((*name).to_owned(), PROXY_URL.to_owned()));
        }
        for name in NO_PROXY_VARIABLES {
            self.env.push(((*name).to_owned(), NO_PROXY.to_owned()));
        }
        self.env.push(("NODE_USE_ENV_PROXY".to_owned(), "1".to_owned()));
        Ok(())
    }

    /// The forwarder's view of a job with port forwards: the job's socket bound
    /// read-only at `$XRT/efr-forward.sock`, as the proxy socket.
    ///
    /// The child gets no `RESOLVE_UNIX` rule for the socket: only the forwarder, in a
    /// Landlock domain of its own, connects to it. The spec check refused forwards for a
    /// job that keeps the host's network namespace; the plan checks the socket again.
    fn forward(&mut self) -> Result<(), SandboxError> {
        let spec = self.spec;
        let Some(plan) = spec.job.as_ref().and_then(|job| job.forward.as_ref()) else {
            return Ok(());
        };
        if plan.socket != spec.runtime.forward_socket() {
            return Err(SandboxError::ForwardSocket { path: plan.socket.clone() });
        }
        let resolved = self.resolve(&plan.socket)?;
        if resolved.kind != Some(FileKind::Other) {
            return Err(SandboxError::ForwardSocket { path: plan.socket.clone() });
        }
        let target = spec.runtime.inside_forward_socket();
        let op = MountOp::Bind { source: resolved.path, target: target.clone(), writable: false };
        self.push(op, OpKind::Floor, MountOrigin::Asset);
        self.forward = Some(InnerForward::new(plan, target));
        Ok(())
    }

    /// The call's filtered buses (phase E): for each `Grant::BusFilter`, the call's own
    /// socket `$CALL/bus/<bus>.sock`, which efrd's bus proxy serves, bound read-only
    /// where a D-Bus client finds the bus with no variable
    /// ([`RuntimePaths::inside_bus_socket`](crate::RuntimePaths::inside_bus_socket)),
    /// a `RESOLVE_UNIX` rule for the child on exactly that place, and the bus's
    /// address variable.
    ///
    /// The session bus lands in the `$XRT` tmpfs. For the system bus the plan first
    /// puts an empty tmpfs on `/run/dbus`, so the bind has a place and the host's real
    /// socket there is out of view; with no `/run/dbus` the plan fails with
    /// [`SandboxError::NoSystemBus`]. The bind is read-only like the proxy socket's:
    /// `connect()` needs no writable mount. The socket must be a socket, else the call
    /// fails closed; efrd drops a view that it cannot serve before it writes the spec.
    fn bus_filter(&mut self) -> Result<(), SandboxError> {
        let spec = self.spec;
        let mut done: Vec<BusKind> = Vec::new();
        for grant in &spec.grants {
            let Grant::BusFilter { bus } = grant else { continue };
            if done.contains(bus) {
                continue;
            }
            done.push(*bus);
            let variable = match bus {
                BusKind::System => "DBUS_SYSTEM_BUS_ADDRESS",
                BusKind::Session => "DBUS_SESSION_BUS_ADDRESS",
                _ => return Err(SandboxError::UnknownGrant),
            };
            let source = spec.runtime.bus_filter_socket(*bus);
            // efrd's call dir lies in efr's runtime root, out of the sandbox's reach, so
            // a link on the way is followed, as for the launcher's other files.
            let resolved = self.resolve(&source)?;
            if resolved.kind != Some(FileKind::Other) {
                return Err(SandboxError::GrantTarget { path: source });
            }
            let target = spec.runtime.inside_bus_socket(*bus);
            if *bus == BusKind::System {
                let Some(dir) = target.parent() else { return Err(SandboxError::NoSystemBus) };
                if self.lstat(dir)? != Some(FileKind::Dir) {
                    return Err(SandboxError::NoSystemBus);
                }
                let op = MountOp::Tmpfs { target: dir.to_path_buf(), perms: 0o755 };
                self.push(op, OpKind::Mask, MountOrigin::Runtime);
            }
            let op =
                MountOp::Bind { source: resolved.path, target: target.clone(), writable: false };
            self.push(op, OpKind::Floor, MountOrigin::Asset);
            self.env.push((variable.to_owned(), format!("unix:path={}", target.display())));
            self.resolve_unix.push(target);
        }
        Ok(())
    }

    /// Socket, bus, desktop and device grants, and the network.
    fn sockets_and_devices(&mut self) -> Result<(), SandboxError> {
        let spec = self.spec;
        let runtime = &spec.runtime;
        // Each socket with true for a socket grant, false for a bus grant.
        let mut sockets: Vec<(PathBuf, bool)> = Vec::new();
        let mut desktops: Vec<&DesktopSocket> = Vec::new();
        for grant in &spec.grants {
            match grant {
                Grant::Socket { path } => sockets.push((path.clone(), true)),
                Grant::Desktop { desktop } => {
                    let socket =
                        runtime.desktops.iter().find(|socket| socket.desktop == *desktop).ok_or(
                            SandboxError::NoDesktop { desktop: *desktop, needs_cookie: false },
                        )?;
                    desktops.push(socket);
                }
                Grant::Bus { bus: BusKind::System } => {
                    sockets.push((runtime.system_bus.clone(), false));
                    self.env.push((
                        "DBUS_SYSTEM_BUS_ADDRESS".to_owned(),
                        format!("unix:path={}", runtime.system_bus.display()),
                    ));
                }
                Grant::Bus { bus: BusKind::Session } => {
                    let bus = runtime.session_bus.clone().ok_or(SandboxError::NoSessionBus)?;
                    self.env.push((
                        "DBUS_SESSION_BUS_ADDRESS".to_owned(),
                        format!("unix:path={}", bus.display()),
                    ));
                    sockets.push((bus, false));
                }
                Grant::Device { path } => {
                    let node = self.resolve(path)?;
                    if !is_within(&node.path, Path::new("/dev"))
                        || !matches!(node.kind, Some(FileKind::Other | FileKind::File))
                    {
                        return Err(SandboxError::GrantTarget { path: path.clone() });
                    }
                    self.push(
                        MountOp::DevBind { node: node.path.clone() },
                        OpKind::Widening,
                        MountOrigin::Device,
                    );
                    self.devices.push(node.path);
                }
                _ => {}
            }
        }
        for (socket, granted) in sockets {
            let resolved = self.resolve(&socket)?;
            if granted && self.service_socket(&socket, &resolved.path)? {
                return Err(SandboxError::SocketGrantRefused { path: socket });
            }
            if resolved.kind != Some(FileKind::Other) || self.hidden_socket(&resolved.path)? {
                return Err(SandboxError::GrantTarget { path: socket });
            }
            let op = MountOp::Bind {
                source: resolved.path.clone(),
                target: resolved.path.clone(),
                writable: true,
            };
            self.push(op, OpKind::Widening, MountOrigin::Socket);
            self.resolve_unix.push(resolved.path);
        }
        for socket in desktops {
            self.desktop(socket)?;
        }
        Ok(())
    }

    /// The one control socket of a desktop grant, bound writable at the path that its
    /// variable names, and that variable. The socket must be the one that
    /// [`desktop_socket_path`] computes from the value, still a socket with the place
    /// and the name of that desktop's socket after every link ([`is_desktop_socket`]),
    /// and no hidden one, so the grant never reaches another socket of the user, such
    /// as Hyprland's event socket or the session bus.
    fn desktop(&mut self, socket: &DesktopSocket) -> Result<(), SandboxError> {
        let refused = || SandboxError::GrantTarget { path: socket.path.clone() };
        let runtime = &self.spec.runtime;
        let expected = desktop_socket_path(socket.desktop, &socket.value, &runtime.user_runtime);
        if expected.as_deref() != Some(socket.path.as_path()) {
            return Err(refused());
        }
        let user_runtime = self.resolve(&runtime.user_runtime)?.path;
        let resolved = self.resolve(&socket.path)?;
        // NOTE: the path after every link must still be a socket of this desktop, so a
        // link named `wayland-1` that points to the session bus binds nothing.
        if resolved.kind != Some(FileKind::Other)
            || !is_desktop_socket(socket.desktop, &resolved.path, &user_runtime)
            || self.hidden_socket(&resolved.path)?
        {
            return Err(refused());
        }
        let op =
            MountOp::Bind { source: resolved.path, target: socket.path.clone(), writable: true };
        self.push(op, OpKind::Widening, MountOrigin::Socket);
        self.resolve_unix.push(socket.path.clone());
        self.env.push((socket.desktop.variable().to_owned(), socket.value.clone()));
        Ok(())
    }

    /// True when a socket at `path`, resolved, lies where no grant may reach: below a
    /// mask other than the user's runtime dir, such as an engine secret
    /// (`~/.gnupg/S.gpg-agent`), or in efr's own runtime, data or state root, such as
    /// efrd's socket. The engine refuses these before any question; this check keeps a
    /// spec that names one from binding it all the same.
    fn hidden_socket(&self, path: &Path) -> Result<bool, SandboxError> {
        let runtime = &self.spec.runtime;
        for root in [&runtime.runtime, &runtime.data, &runtime.state] {
            if is_within(path, &self.resolve(root)?.path) {
                return Ok(true);
            }
        }
        Ok(self.masked_by(path).is_some_and(|mask| mask.kind != MaskKind::EfrState))
    }

    /// True when a socket grant of `named`, which resolves to `resolved`, reaches what
    /// only another grant may bind, or nothing may: the system or the session bus, the
    /// socket of a service manager or a container engine, a socket below
    /// `$XRT/systemd/`, or the control socket of any desktop. The engine classifies a
    /// socket by its text only, so this check on the path after every link is what
    /// keeps a link in a write root (`~/p/app.sock -> $XRT/bus`) from the real bus. It
    /// is not part of [`hidden_socket`](Self::hidden_socket), because a desktop grant
    /// must still bind its own socket.
    fn service_socket(&self, named: &Path, resolved: &Path) -> Result<bool, SandboxError> {
        let runtime = &self.spec.runtime;
        let user_runtime = self.resolve(&runtime.user_runtime)?.path;
        let mut buses = vec![
            runtime.system_bus.clone(),
            PathBuf::from(SYSTEM_BUS_SOCKET),
            runtime.user_runtime.join("bus"),
        ];
        buses.extend(runtime.session_bus.clone());
        for bus in &buses {
            if self.resolve(bus)?.path == resolved {
                return Ok(true);
            }
        }
        let service = |path: &Path| SERVICE_SOCKETS.iter().any(|socket| path.ends_with(socket));
        Ok(service(named)
            || service(resolved)
            || is_within(resolved, &user_runtime.join("systemd"))
            || DesktopKind::ALL
                .iter()
                .any(|desktop| is_desktop_socket(*desktop, resolved, &user_runtime)))
    }

    fn finish(self) -> MountPlan {
        let Builder {
            spec,
            mut mounts,
            mut notes,
            roots,
            widenings,
            mut caches,
            resolve_unix,
            devices,
            env,
            proxy_socket,
            forward,
            ..
        } = self;
        mounts.sort_by_key(|mount| (depth(mount.op.target()), mount.kind));
        let mut unique: Vec<Mount> = Vec::with_capacity(mounts.len());
        for mount in mounts {
            if !unique.iter().any(|known| known.op == mount.op) {
                unique.push(mount);
            }
        }
        // A cache with another mount at its own path (a floor, a PATH dir) stays as
        // that mount makes it: the mount covers the whole overlay.
        let covered: Vec<PathBuf> = unique
            .iter()
            .filter(|mount| mount.origin == MountOrigin::Cache)
            .map(|mount| mount.op.target().to_path_buf())
            .filter(|cache| {
                unique.iter().any(|other| {
                    other.origin != MountOrigin::Cache && other.op.target() == cache.as_path()
                })
            })
            .collect();
        for cache in &covered {
            unique.retain(|mount| {
                !(mount.origin == MountOrigin::Cache && mount.op.target() == cache.as_path())
            });
            caches.retain(|known| known != cache);
            notes.push(PlanNote::CacheSkipped(cache.clone()));
        }
        let (layers, layer_sources) = cache_layers(spec, &unique);
        let mut dirs: Vec<PathBuf> = roots.iter().map(|root| root.path.clone()).collect();
        dirs.extend(caches);
        dirs.extend(["/tmp", "/var/tmp", "/dev/shm"].map(PathBuf::from));
        let mut files = Vec::new();
        for widening in widenings {
            if widening.dir {
                dirs.push(widening.path);
            } else {
                files.push(widening.path);
            }
        }
        let write_dirs = outermost(dirs);
        // NOTE: one predicate decides both: the host's network namespace comes only
        // with an open network or with the network view's profile, never with the
        // phase 1 profile alone. A network view with a proxy plan keeps the private
        // namespace.
        let open_grant = spec.grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork));
        let net_view = spec.grants.iter().any(|grant| matches!(grant, Grant::NetView))
            && !open_grant
            && matches!(spec.network, NetworkPlan::None);
        let unshare_net = !(matches!(spec.network, NetworkPlan::Open) || open_grant || net_view);
        // NOTE: the bridge starts only in the call's own network namespace; `proxy()`
        // refused the one plan that keeps the host's.
        let proxy_socket = proxy_socket.filter(|_| unshare_net);
        // NOTE: the same for the forwarder: in the host's namespace it would serve the
        // host's own ports; the spec check refused that plan.
        let forward = forward.filter(|_| unshare_net);
        let runtime = &spec.runtime;
        let child_argv: Vec<OsString> = vec![
            runtime.zsh.clone().into_os_string(),
            OsString::from("-f"),
            runtime.inside_child_script().into_os_string(),
        ];
        MountPlan {
            mounts: unique,
            notes,
            unshare_net,
            net_view,
            write_dirs,
            write_files: files,
            resolve_unix,
            devices,
            env,
            inside_launcher: runtime.inside_launcher(),
            child_argv,
            private_tmp: runtime.private_tmp(),
            layers,
            layer_sources,
            proxy_socket,
            forward,
        }
    }
}

/// The overlays of `mounts` for the launcher's helper, and the layer dir of each one
/// that keeps its private layer (every one in the `overlay` mode, the kept caches in
/// the `tmp` mode), which bwrap binds into the staging dir, in the order of the plan.
fn cache_layers(spec: &SandboxSpec, mounts: &[Mount]) -> (Option<CacheLayers>, Vec<PathBuf>) {
    let staging = spec.runtime.inside_dir().join(STAGING_DIR);
    let mut layers = Vec::new();
    let mut sources = Vec::new();
    for mount in mounts {
        let (target, upper) = match &mount.op {
            MountOp::TmpOverlay { target, .. } => (target, None),
            MountOp::Overlay { target, upper, work, .. } => (target, Some((upper, work))),
            _ => continue,
        };
        let dir = staging.join(layers.len().to_string());
        let named = |path: &Path, fallback: &str| {
            dir.join(path.file_name().map_or_else(|| fallback.into(), ToOwned::to_owned))
        };
        let fresh = upper.is_none();
        let (upper, work) = match upper {
            Some((upper, work)) => {
                // The plan checked that the two share a parent of their own.
                sources.push(upper.parent().unwrap_or(upper).to_path_buf());
                (named(upper, "upper"), named(work, "work"))
            }
            None => (dir.join("upper"), dir.join("work")),
        };
        // A cache's own overlay is never a mount that bwrap made, so it never moves.
        let inside = mounts.iter().filter(|mount| mount.origin != MountOrigin::Cache);
        let moved = moved_mounts(target, inside.map(|mount| mount.op.target()));
        layers.push(CacheLayer { target: target.clone(), dir, upper, work, moved, fresh });
    }
    if layers.is_empty() {
        return (None, sources);
    }
    (Some(CacheLayers { staging, layers }), sources)
}

/// `paths` without one that lies in another, in their order.
fn outermost(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        if out.iter().any(|known| is_within(&path, known)) {
            continue;
        }
        out.retain(|known| !is_within(known, &path));
        out.push(path);
    }
    out
}
