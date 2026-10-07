//! One call's `SandboxSpec`, built from plain paths: the turn's project and the
//! registered projects that the line names, the git dirs of a worktree project from
//! efrd's registration record, the user's extra roots and caches, the engine's secrets
//! and the sandbox-only masks, the floors (efr's config and its link targets, shell
//! startup files, autostart, tool configs, dotfile link targets, efr's binaries, the
//! protected names in each root, the user's additions) and the call's grants.
//!
//! The launcher builds the mount plan from it and adds what it derives itself:
//! efr's own roots as masks, scratch and the private tmp as write roots, the git
//! surface of each project root and the `PATH` dirs in a write root as floors.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use efr_config::{SandboxSettings, WriteProjects};
use efr_permissions::{PROTECTED_NAMES, persistence_floors, sandbox_masks};
use efr_protocol::{CacheMode, CallId, ConversationId, Grant, Launch};
use efr_sandbox::{
    CacheOverlay, EnvPlan, Floor, FloorKind, Mask, MaskKind, NetworkPlan, RecordLimits,
    RuntimePaths, SCAN_SKIP, SPEC_VERSION, SandboxSpec, SpecLaunch, WorktreeCheck, WriteRoot,
    WriteRootKind, expand_home, is_within, name_matches, too_wide,
};
use efr_stdx::paths::Dirs;

use crate::sandbox::links::link_targets;
use crate::sandbox::projects::{self, WORKTREE_CHANGED, WORKTREE_NO_RECORD};

/// The directory below efr's state root that holds each conversation's sandbox dir.
pub(crate) const SANDBOX_DIR: &str = "sandbox";

/// The directory below efr's runtime root that holds each conversation's shell dir.
pub(crate) const SHELL_DIR: &str = "sbx";

/// The names of the zsh startup files that `$ZDOTDIR` holds.
const ZSH_STARTUP: &[&str] = &[".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"];

/// How deep below a write root the scan for project `.env` files looks.
const ENV_DEPTH: usize = 3;

/// The most entries that the `.env` scan reads per root.
const ENV_ENTRIES: usize = 20_000;

/// What the machine and the hidden shells look like, read once at start.
#[derive(Debug, Clone, Default)]
pub(crate) struct HostFacts {
    /// The hidden shell's `PATH`.
    pub(crate) path: String,
    /// The hidden shell's zsh.
    pub(crate) zsh: Option<PathBuf>,
    /// `$XDG_RUNTIME_DIR`, which the sandbox replaces with an empty tmpfs.
    pub(crate) user_runtime: PathBuf,
    /// The session bus socket.
    pub(crate) session_bus: Option<PathBuf>,
    /// `$XAUTHORITY`, masked.
    pub(crate) xauthority: Option<PathBuf>,
    /// `$HISTFILE`, masked.
    pub(crate) histfile: Option<PathBuf>,
    /// `$ZDOTDIR`, whose startup files are floors.
    pub(crate) zdotdir: Option<PathBuf>,
    /// efr's own programs: efrd, efr and the launcher's source.
    pub(crate) binaries: Vec<PathBuf>,
}

impl HostFacts {
    /// The facts of the hidden shells' environment `env`, with `zsh` their program.
    pub(crate) fn from_env(env: &BTreeMap<String, String>, zsh: Option<PathBuf>) -> HostFacts {
        let absolute =
            |name: &str| env.get(name).map(PathBuf::from).filter(|path| path.is_absolute());
        let user_runtime = absolute("XDG_RUNTIME_DIR").unwrap_or_else(|| {
            PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw()))
        });
        let session_bus = env
            .get("DBUS_SESSION_BUS_ADDRESS")
            .and_then(|address| address.split(';').find_map(|part| part.strip_prefix("unix:path=")))
            .map(|path| PathBuf::from(path.split(',').next().unwrap_or(path)))
            .filter(|path| path.is_absolute())
            .or_else(|| Some(user_runtime.join("bus")).filter(|bus| bus.exists()));
        HostFacts {
            path: env.get("PATH").cloned().unwrap_or_default(),
            zsh,
            user_runtime,
            session_bus,
            xauthority: absolute("XAUTHORITY"),
            histfile: absolute("HISTFILE"),
            zdotdir: absolute("ZDOTDIR"),
            binaries: Vec::new(),
        }
    }
}

/// Everything one call's spec is built from.
pub(crate) struct PlanInput<'a> {
    pub(crate) conversation: ConversationId,
    pub(crate) call: CallId,
    pub(crate) launch: &'a Launch,
    /// The root of the turn's project, when the turn runs in one.
    pub(crate) turn_project: Option<&'a Path>,
    /// Every registered project root.
    pub(crate) projects: &'a [PathBuf],
    /// The paths that the call names: its declared paths and the directory it starts
    /// in, which pick the named projects.
    pub(crate) named_paths: &'a [PathBuf],
    /// The conversation's `$SCRATCH`.
    pub(crate) scratch: &'a Path,
    pub(crate) settings: &'a SandboxSettings,
    /// The engine's secrets: masks that no grant opens.
    pub(crate) secrets: &'a [PathBuf],
    /// efr's config directory and the real files behind its links.
    pub(crate) protected_config: &'a [PathBuf],
    pub(crate) host: &'a HostFacts,
    pub(crate) dirs: &'a Dirs,
    pub(crate) home: &'a Path,
    /// The bubblewrap program the probe found.
    pub(crate) bwrap: &'a Path,
    /// The cache mode the probe found to work.
    pub(crate) cache_mode: CacheMode,
    /// The launcher's copy in efr's runtime root.
    pub(crate) launcher: &'a Path,
}

/// A built spec, and what the tool result says about it.
#[derive(Debug)]
pub(crate) struct Planned {
    pub(crate) spec: SandboxSpec,
    /// Notes for the model, such as a worktree whose `.git` file changed.
    pub(crate) notes: Vec<&'static str>,
}

/// The write roots of the projects of a call: the turn's project, then the named or
/// every registered project, as `sandbox.write_projects` says; never a root at or above
/// the home directory.
pub(crate) fn project_roots(input: &PlanInput<'_>) -> Vec<WriteRoot> {
    let home = input.home;
    let mut roots: Vec<WriteRoot> = Vec::new();
    let mut add = |path: &Path, kind: WriteRootKind| {
        if !too_wide(path, home) && !roots.iter().any(|root| root.path == path) {
            roots.push(WriteRoot { path: path.to_path_buf(), kind });
        }
    };
    if let Some(project) = input.turn_project {
        add(project, WriteRootKind::TurnProject);
    }
    for project in input.projects {
        let named = match input.settings.write_projects {
            WriteProjects::Turn => false,
            WriteProjects::All => true,
            // NOTE: a variant added later writes no more than the named projects.
            _ => input.named_paths.iter().any(|path| is_within(path, project)),
        };
        if named {
            add(project, WriteRootKind::NamedProject);
        }
    }
    roots
}

/// The spec of one call. It reads the file system (worktree records, link targets,
/// project `.env` files), so it blocks.
pub(crate) fn build(input: &PlanInput<'_>) -> Planned {
    let home = input.home;
    let settings = input.settings;
    let mut notes = Vec::new();
    let mut write_roots = project_roots(input);
    let projects: Vec<PathBuf> = write_roots.iter().map(|root| root.path.clone()).collect();
    let mut git_dirs = Vec::new();
    for project in &projects {
        match projects::check(input.dirs.state(), project) {
            WorktreeCheck::Matches(record) => {
                for (path, kind) in [
                    (record.git_dir, WriteRootKind::GitDir),
                    (record.common_dir, WriteRootKind::GitCommonDir),
                ] {
                    if !too_wide(&path, home) && !git_dirs.contains(&path) {
                        git_dirs.push(path.clone());
                        write_roots.push(WriteRoot { path, kind });
                    }
                }
            }
            WorktreeCheck::Changed => notes.push(WORKTREE_CHANGED),
            WorktreeCheck::NoRecord => notes.push(WORKTREE_NO_RECORD),
            _ => {}
        }
    }
    for root in &settings.write_roots {
        let path = expand_home(root, home);
        if path.is_absolute() && !too_wide(&path, home) {
            write_roots.push(WriteRoot { path, kind: WriteRootKind::UserConfigured });
        }
    }
    let writable: Vec<PathBuf> = write_roots.iter().map(|root| root.path.clone()).collect();

    let runtime = runtime_paths(input);
    let caches = settings
        .caches
        .iter()
        .map(|cache| expand_home(cache, home))
        .filter(|cache| cache.is_absolute())
        .map(|cache| CacheOverlay::new(&cache, &runtime.sandbox_dir, home))
        .collect();

    let masks = masks(input, &projects);
    let floors = floors(input, &projects, &writable);
    let mut guard_roots = writable.clone();
    guard_roots.retain(|root| !git_dirs.contains(root));
    guard_roots.push(input.scratch.to_path_buf());
    let grants = input.launch.grants().to_vec();
    let network =
        if grants.contains(&Grant::OpenNetwork) { NetworkPlan::Open } else { NetworkPlan::None };
    let launch = match input.launch {
        Launch::Unsandboxed => SpecLaunch::Unsandboxed,
        _ => SpecLaunch::Contained,
    };
    let spec = SandboxSpec {
        version: SPEC_VERSION,
        conversation: input.conversation,
        call: input.call,
        launch,
        write_roots,
        caches,
        cache_mode: input.cache_mode,
        masks,
        floors,
        git_dirs,
        guard_roots,
        protected_names: PROTECTED_NAMES.iter().map(|name| (*name).to_owned()).collect(),
        grants,
        network,
        env: EnvPlan {
            deny: settings.env_deny.clone(),
            keep: settings.env_keep.clone(),
            promote: settings.promote_env.clone(),
            export_deny: settings.export_deny.clone(),
            offline_hints: settings.offline_hints,
        },
        shell_path: input.host.path.clone(),
        limits: RecordLimits::default(),
        runtime,
    };
    Planned { spec, notes }
}

/// Where things are for the call.
pub(crate) fn runtime_paths(input: &PlanInput<'_>) -> RuntimePaths {
    let dirs = input.dirs;
    let conversation = input.conversation.to_string();
    let shell_dir = dirs.runtime().join(SHELL_DIR).join(&conversation);
    let zsh_dir = crate::shells::integration_dir(dirs.runtime());
    RuntimePaths {
        home: input.home.to_path_buf(),
        user_runtime: input.host.user_runtime.clone(),
        runtime: dirs.runtime().to_path_buf(),
        data: dirs.data().to_path_buf(),
        state: dirs.state().to_path_buf(),
        config: dirs.config().to_path_buf(),
        sandbox_dir: dirs.state().join(SANDBOX_DIR).join(&conversation),
        call_dir: shell_dir.join(input.call.to_string()),
        shell_dir,
        scratch: input.scratch.to_path_buf(),
        launcher: input.launcher.to_path_buf(),
        child_script: zsh_dir.join("efr-child.zsh"),
        editor: zsh_dir.join("efr-editor"),
        zsh: input.host.zsh.clone().unwrap_or_else(|| PathBuf::from("/usr/bin/zsh")),
        bwrap: input.bwrap.to_path_buf(),
        session_bus: input.host.session_bus.clone(),
        system_bus: PathBuf::from("/run/dbus/system_bus_socket"),
    }
}

/// The masks: the engine's secrets, the sandbox-only masks, the user's, and the
/// project `.env` files of `sandbox.mask_globs`.
fn masks(input: &PlanInput<'_>, projects: &[PathBuf]) -> Vec<Mask> {
    let home = input.home;
    let mut masks: Vec<Mask> = Vec::new();
    let mut add = |path: PathBuf, kind: MaskKind| {
        if path.is_absolute() && !masks.iter().any(|mask| mask.path == path) {
            masks.push(Mask { path, kind });
        }
    };
    for secret in input.secrets {
        add(secret.clone(), MaskKind::EngineSecret);
    }
    for mask in sandbox_masks(home) {
        add(mask, MaskKind::SandboxMask);
    }
    for mask in [&input.host.xauthority, &input.host.histfile].into_iter().flatten() {
        add(mask.clone(), MaskKind::SandboxMask);
    }
    for mask in &input.settings.mask {
        add(expand_home(mask, home), MaskKind::User);
    }
    for project in projects {
        for file in env_files(project, &input.settings.mask_globs) {
            add(file, MaskKind::ProjectEnv);
        }
    }
    masks
}

/// The floors that efrd knows: efr's config and its link targets, the persistence
/// paths of the home directory, `$ZDOTDIR`'s startup files, dotfile link targets in a
/// write root, efr's binaries, the protected names in each project root, and
/// `sandbox.protect`.
fn floors(input: &PlanInput<'_>, projects: &[PathBuf], writable: &[PathBuf]) -> Vec<Floor> {
    let home = input.home;
    let mut floors: Vec<Floor> = Vec::new();
    let mut add = |path: PathBuf, kind: FloorKind| {
        if path.is_absolute() && !floors.iter().any(|floor| floor.path == path) {
            floors.push(Floor { path, kind });
        }
    };
    for config in input.protected_config {
        add(config.clone(), FloorKind::Config);
    }
    for path in persistence_floors(home) {
        let kind = floor_kind(path.strip_prefix(home).unwrap_or(&path));
        add(path, kind);
    }
    if let Some(zdotdir) = &input.host.zdotdir {
        for name in ZSH_STARTUP {
            add(zdotdir.join(name), FloorKind::ShellStartup);
        }
    }
    for target in link_targets(home, writable) {
        add(target, FloorKind::LinkTarget);
    }
    for binary in &input.host.binaries {
        add(binary.clone(), FloorKind::EfrBinary);
    }
    for project in projects {
        for name in PROTECTED_NAMES {
            add(project.join(name.trim_end_matches('/')), FloorKind::ProtectedName);
        }
    }
    for path in &input.settings.protect {
        add(expand_home(path, home), FloorKind::User);
    }
    floors
}

/// What a persistence floor of the home directory is, from its path below the home.
pub(crate) fn floor_kind(relative: &Path) -> FloorKind {
    let name = relative.to_string_lossy();
    let startup = name.starts_with(".z")
        || name.starts_with(".bash")
        || matches!(
            name.as_ref(),
            ".profile"
                | ".config/fish"
                | ".config/environment.d"
                | ".pam_environment"
                | ".xprofile"
                | ".xinitrc"
        );
    let autostart = matches!(
        name.as_ref(),
        ".config/systemd"
            | ".config/autostart"
            | ".local/share/systemd"
            | ".local/share/applications"
            | ".config/hypr"
            | ".config/sway"
            | ".config/i3"
    );
    if startup {
        FloorKind::ShellStartup
    } else if autostart {
        FloorKind::Autostart
    } else {
        FloorKind::ToolConfig
    }
}

/// The files below `root`, to [`ENV_DEPTH`] levels and outside the build dirs, whose
/// names `globs` mask: a glob with a leading `!` keeps a name readable.
pub(crate) fn env_files(root: &Path, globs: &[String]) -> Vec<PathBuf> {
    let (keep, mask): (Vec<&str>, Vec<&str>) =
        globs.iter().map(String::as_str).partition(|glob| glob.starts_with('!'));
    let keep: Vec<&str> = keep.iter().map(|glob| &glob[1..]).collect();
    let mut found = Vec::new();
    let mut todo = vec![(root.to_path_buf(), 0_usize)];
    let mut seen = 0_usize;
    while let Some((dir, depth)) = todo.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > ENV_ENTRIES {
                return found;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth + 1 < ENV_DEPTH && !SCAN_SKIP.contains(&name) {
                    todo.push((entry.path(), depth + 1));
                }
                continue;
            }
            let masked = mask.iter().any(|glob| name_matches(glob, name))
                && !keep.iter().any(|glob| name_matches(glob, name));
            if masked {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests;
