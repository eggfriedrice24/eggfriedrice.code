//! [`MountPlan::build`]: the rules of the module docs, applied to one spec.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, CacheMode, Grant};

use crate::SandboxError;
use crate::fs_view::{FileKind, FsView, Resolved, resolve};
use crate::git_config::parse_config;
use crate::layers::{CacheLayer, CacheLayers, STAGING_DIR, moved_mounts};
use crate::paths::{depth, expand_home, is_within, normalize, too_wide};
use crate::plan::{Mount, MountOp, MountOrigin, MountPlan, OpKind, PlanNote};
use crate::spec::{FloorKind, MaskKind, NetworkPlan, SandboxSpec, WriteRootKind};

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
}

impl MountPlan {
    /// Checks `spec` and turns it into the ordered mounts of one contained call,
    /// reading the file system only through `fs`.
    pub fn build(spec: &SandboxSpec, fs: &dyn FsView) -> Result<MountPlan, SandboxError> {
        spec.check()?;
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
        builder.assets()?;
        builder.sockets_and_devices()?;
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
        for cache in &self.spec.caches {
            let resolved = self.resolve(&cache.target)?;
            let in_root = self.writable(&resolved.path);
            if resolved.kind != Some(FileKind::Dir)
                || in_root
                || self.masked_by(&resolved.path).is_some()
            {
                self.notes.push(PlanNote::CacheSkipped(cache.target.clone()));
                continue;
            }
            let target = resolved.path;
            if self.spec.cache_mode == CacheMode::Overlay {
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
            let op = match self.spec.cache_mode {
                CacheMode::Tmp => {
                    MountOp::TmpOverlay { lower: target.clone(), target: target.clone() }
                }
                _ => MountOp::Overlay {
                    lower: target.clone(),
                    upper: cache.upper.clone(),
                    work: cache.work.clone(),
                    target: target.clone(),
                },
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
        for group in ["modules", "worktrees"] {
            let dir = git_dir.join(group);
            match self.lstat(&dir)? {
                Some(FileKind::Dir) => {}
                Some(FileKind::Symlink) => return Err(SandboxError::SymlinkedPin { path: dir }),
                _ => continue,
            }
            let mut names = self
                .fs
                .read_dir(&dir)
                .map_err(|source| SandboxError::Io { path: dir.clone(), source })?;
            names.sort();
            for name in names {
                let entry = dir.join(name);
                match self.lstat(&entry)? {
                    Some(FileKind::Dir) => self.git_surface(&entry)?,
                    Some(FileKind::Symlink) => {
                        return Err(SandboxError::SymlinkedPin { path: entry });
                    }
                    _ => {}
                }
            }
        }
        let config = git_dir.join("config");
        if self.lstat(&config)? == Some(FileKind::File) {
            let text = self
                .fs
                .read_file(&config, MAX_GIT_CONFIG)
                .map_err(|source| SandboxError::Io { path: config.clone(), source })?;
            let work_tree = git_dir.parent().unwrap_or(git_dir).to_path_buf();
            for (key, value) in parse_config(&String::from_utf8_lossy(&text)) {
                let base = if key == "core.hookspath" { &work_tree } else { git_dir };
                let is_include = key == "include.path"
                    || (key.starts_with("includeif.") && key.ends_with(".path"));
                if !is_include && key != "core.hookspath" {
                    continue;
                }
                let named = expand_home(Path::new(&value), &self.spec.runtime.home);
                let Some(path) = normalize(&base.join(named)) else { continue };
                if self.writable(&path) {
                    let kind = if key == "core.hookspath" {
                        FloorKind::GitHooks
                    } else {
                        FloorKind::GitConfig
                    };
                    self.floor(&path, kind)?;
                }
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
        if let Some(mask) = self.masked_by(&resolved.path) {
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

    /// The launcher, the child script, the shell snapshot and state, the line and the
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

    /// Socket, bus and device grants, and the network.
    fn sockets_and_devices(&mut self) -> Result<(), SandboxError> {
        let runtime = &self.spec.runtime;
        let mut sockets: Vec<PathBuf> = Vec::new();
        for grant in &self.spec.grants {
            match grant {
                Grant::Socket { path } => sockets.push(path.clone()),
                Grant::Bus { bus: BusKind::System } => {
                    sockets.push(runtime.system_bus.clone());
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
                    sockets.push(bus);
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
        for socket in sockets {
            let resolved = self.resolve(&socket)?;
            if resolved.kind != Some(FileKind::Other) {
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
        Ok(())
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
        let unshare_net = match &spec.network {
            NetworkPlan::Open => false,
            NetworkPlan::None | NetworkPlan::Proxy { .. } => {
                !spec.grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork))
            }
        };
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
        }
    }
}

/// The overlays of `mounts` for the launcher's helper, and in the `overlay` mode the
/// layer dir of each one, which bwrap binds into the staging dir.
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
        let (upper, work) = match upper {
            Some((upper, work)) => {
                // The plan checked that the two share a parent of their own.
                sources.push(upper.parent().unwrap_or(upper).to_path_buf());
                (named(upper, "upper"), named(work, "work"))
            }
            None => (dir.join("upper"), dir.join("work")),
        };
        let moved = moved_mounts(target, mounts.iter().map(|mount| mount.op.target()));
        layers.push(CacheLayer { target: target.clone(), dir, upper, work, moved });
    }
    if layers.is_empty() {
        return (None, sources);
    }
    let fresh = spec.cache_mode != CacheMode::Overlay;
    (Some(CacheLayers { staging, fresh, layers }), sources)
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
