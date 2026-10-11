//! The permission engine, built in one place from the settings, the project registry
//! and efr's config directory.
//!
//! The engine holds the home directory and its resolved form, the daemon's own secrets
//! (sealed, so no rule opens them), efr's config directory and the real files behind
//! its symbolic links (write-sealed, so no tool writes them), the secret paths of the
//! settings (`~/` below the home directory) and the registered projects. It decides by
//! the built-in policy of each turn's permission mode followed by the user's rules. A
//! tool call reads the latest engine from the watch channel in `State`; whatever
//! changes the rules, the secret paths, the projects or the links in the config
//! directory builds a new one here and sends it there.

use std::path::{Path, PathBuf};

use efr_config::{AskDestructive, Settings, WriteProjects};
use efr_permissions::{
    AutoSupport, Engine, FloorGroup, Locations, PermissionsError, ZSH_STARTUP_FILES,
};
use efr_sandbox::{expand_home, is_within, too_wide};
use efr_scope::{Home, Registry};

use crate::DaemonError;
use crate::sandbox::links::dotfile_links;
use crate::sandbox::{HostFacts, launcher};
use crate::state::SCRATCH_DIR;

/// The places that every contained call may write besides the projects: the private
/// `/tmp`, `/var/tmp` and `/dev/shm`.
const PRIVATE_ROOTS: &[&str] = &["/tmp", "/var/tmp", "/dev/shm"];

/// The name of efr's trash below the data directory: a tool may move files there, and
/// efr can bring them back.
pub(crate) const TRASH_DIR: &str = "trash";

/// What the engine needs from the hidden shells and the file system for the `auto`
/// sandbox: the shells' `PATH`, `$ZDOTDIR`, `$XAUTHORITY` and `$HISTFILE`, the
/// dotfile link targets that lie in a write root, and efr's own roots and programs.
#[derive(Debug, Clone, Default)]
pub(crate) struct SandboxFacts {
    pub(crate) host: HostFacts,
    /// Each dotfile link whose target lies in a write root, with that target.
    pub(crate) links: Vec<(PathBuf, PathBuf)>,
    pub(crate) efr: EfrPaths,
}

/// efr's own roots and programs, which no tool writes in any mode, and which a shell
/// write in `auto` reaches only as a refused `config` exit. The runtime root holds the
/// hidden shells' `$ZDOTDIR` and the launcher's copy; the data root holds the database,
/// whose prompts decide whether a user named an exit, and the backups of floor files.
/// `$SCRATCH` and efr's trash below the data root stay open.
#[derive(Debug, Clone, Default)]
pub(crate) struct EfrPaths {
    pub(crate) runtime: Option<PathBuf>,
    pub(crate) data: Option<PathBuf>,
    pub(crate) state: Option<PathBuf>,
    /// efr's programs besides the ones in [`HostFacts::binaries`]: the launcher's
    /// source and its copy in the runtime root.
    pub(crate) binaries: Vec<PathBuf>,
}

/// What the engine is built from besides the settings. It does not change while the
/// daemon runs.
#[derive(Debug, Clone)]
pub(crate) struct EngineParts {
    /// The user's home directory.
    pub(crate) home: Home,
    /// The daemon's own `secrets/` directory.
    pub(crate) secrets: PathBuf,
    /// The project registry file.
    pub(crate) registry: PathBuf,
    /// efr's config directory, which holds `config.toml` and the registry.
    pub(crate) config: PathBuf,
    /// The hidden shells' environment as the sandbox reads it.
    pub(crate) host: HostFacts,
    /// efr's runtime root.
    pub(crate) runtime: PathBuf,
    /// efr's data root.
    pub(crate) data: PathBuf,
    /// efr's state root.
    pub(crate) state: PathBuf,
    /// The launcher that efrd copies into the runtime root, when it found one.
    pub(crate) launcher: Option<PathBuf>,
}

impl EngineParts {
    /// Reads the project registry and the links in the config directory, and builds
    /// the engine for `settings`.
    pub(crate) async fn engine(&self, settings: &Settings) -> Result<Engine, DaemonError> {
        let projects = load_registry(&self.registry).await;
        let config = self.config.clone();
        let protected = tokio::task::spawn_blocking(move || protected_config(&config))
            .await
            .unwrap_or_else(|_| vec![self.config.clone()]);
        let roots = write_roots(self.home.path(), settings, &projects);
        let home = self.home.path().to_path_buf();
        let links = tokio::task::spawn_blocking(move || dotfile_links(&home, &roots))
            .await
            .unwrap_or_default();
        let facts = SandboxFacts { host: self.host.clone(), links, efr: self.efr() };
        build(&self.home, &self.secrets, &protected, settings, &projects, &facts)
    }

    /// efr's own roots and programs.
    fn efr(&self) -> EfrPaths {
        let mut binaries: Vec<PathBuf> = self.launcher.iter().cloned().collect();
        binaries.push(self.runtime.join(launcher::BIN_DIR).join(launcher::LAUNCHER));
        EfrPaths {
            runtime: Some(self.runtime.clone()),
            data: Some(self.data.clone()),
            state: Some(self.state.clone()),
            binaries,
        }
    }
}

/// The project registry at `path`, read off the async workers. A registry that cannot
/// be read registers no project and costs a warning.
pub(crate) async fn load_registry(path: &Path) -> Registry {
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || Registry::load(&path)).await {
        Ok(Ok(registry)) => registry,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "the project registry could not be read; no project is registered");
            Registry::empty()
        }
        Err(_) => Registry::empty(),
    }
}

/// The places that no tool may write: the config directory `dir`, its resolved form
/// when it is a symbolic link, and what each symbolic link directly in it reaches,
/// such as `~/dotfiles/efr/config.toml` behind `config.toml`. A link whose target is
/// missing gives the target it names, because a write through it would create that
/// file. It blocks; async callers run it in `spawn_blocking`.
///
/// NOTE: a directory that cannot be read gives itself alone. The links are read again
/// each time the engine is built, so a link the user adds later counts from then on.
pub(crate) fn protected_config(dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![dir.to_path_buf()];
    let mut add = |root: PathBuf| {
        if !roots.contains(&root) {
            roots.push(root);
        }
    };
    if let Ok(real) = std::fs::canonicalize(dir) {
        add(real);
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return roots;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_link = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_symlink());
        if !is_link {
            continue;
        }
        match std::fs::canonicalize(&path) {
            Ok(real) => add(real),
            // NOTE: `join` keeps an absolute target as it is and puts a relative one
            // below the directory that holds the link, as the file system reads it.
            Err(_) => {
                if let Ok(target) = std::fs::read_link(&path) {
                    add(dir.join(target));
                }
            }
        }
    }
    roots
}

/// The engine for `settings` and `projects`, with `secrets` sealed and `protected`
/// (from [`protected_config`]) write-sealed.
///
/// NOTE: the user's rules belong to the engine, the machine policy, and not to
/// `ConversationConfig::policy`: a conversation's rules may never open a secret or a
/// system path, and the user's explicit rules must be able to. The mode is not part of
/// the engine: each turn passes its own, and the engine holds the policy of every mode.
pub(crate) fn build(
    home: &Home,
    secrets: &Path,
    protected: &[PathBuf],
    settings: &Settings,
    projects: &Registry,
    sandbox: &SandboxFacts,
) -> Result<Engine, DaemonError> {
    let invalid = |source| DaemonError::Locations { source };
    let mut locations = Locations::new(home.path()).map_err(invalid)?;
    // NOTE: an alias the engine refuses (one inside or above the home directory) only
    // costs the resolved form; paths under the home directory itself still classify.
    locations = match locations.clone().with_home_alias(home.canonical()) {
        Ok(aliased) => aliased,
        Err(error) => {
            tracing::warn!(error = %error, alias = %home.canonical().display(), "the resolved home directory is not used as an alias");
            locations
        }
    };
    // NOTE: the daemon's own tokens would let the model act as the user at the
    // provider, so no rule of the user's may open them, not even `class = "secrets"`.
    locations = locations.with_sealed_root(secrets).map_err(invalid)?;
    // NOTE: the config holds the permission rules and the registry defines the project
    // that the auto mode trusts, so a tool that could write them could grant itself
    // anything. Only the user changes them.
    for root in protected {
        locations = locations.with_write_sealed_root(root).map_err(invalid)?;
    }
    for path in &settings.permissions.secret_paths {
        let root = match path.strip_prefix("~") {
            Ok(below) => home.path().join(below),
            Err(_) => path.clone(),
        };
        locations = locations.with_secret_root(root).map_err(invalid)?;
    }
    for project in projects.projects() {
        locations = locations.with_project(project.id(), project.root()).map_err(invalid)?;
    }
    locations = with_sandbox(locations, home.path(), settings, projects, sandbox);
    locations = with_efr(locations, sandbox)?;
    // NOTE: no proxy (phase D), no bus proxy (phase E) yet, and the mount view of phase D
    // sets `mount_view`. efrd keeps the home store while snapshots are on (`home.rs`),
    // and a covered destructive part runs with no review only while undo is on too and
    // `sandbox.ask_destructive` lets it (plan PB17). With snapshots off, a named floor
    // file edit goes back to the user: stricter, never looser.
    let floor_backup = settings.snapshot.enabled;
    let undo = floor_backup
        && settings.undo.enabled
        && settings.sandbox.ask_destructive == AskDestructive::UnlessUndoable;
    let support = AutoSupport { floor_backup, undo, mount_view: false, ..AutoSupport::default() };
    Ok(Engine::with_rules(locations, settings.permissions.rules.clone()).with_support(support))
}

/// `locations` with efr's own roots and programs (plan PD36): the runtime root, the
/// data root except `$SCRATCH` and efr's trash, the state root, efrd, `efr` and the
/// launcher with its copy. A write there is refused in every mode.
///
/// NOTE: before phase A the engine sealed only the daemon's `secrets/`, and a write to
/// the database or the hidden shells' `$ZDOTDIR` was a `write` exit that only the user
/// approved. The classifier now judges such writes, so without this floor only the
/// naming rule would stand between the model and efr's own trust anchors.
fn with_efr(mut locations: Locations, sandbox: &SandboxFacts) -> Result<Locations, DaemonError> {
    let invalid = |source| DaemonError::Locations { source };
    let efr = &sandbox.efr;
    if let Some(runtime) = &efr.runtime {
        locations = locations.with_efr_root(runtime, []).map_err(invalid)?;
    }
    if let Some(data) = &efr.data {
        let open = [data.join(SCRATCH_DIR), data.join(TRASH_DIR)];
        locations = locations.with_efr_root(data, open).map_err(invalid)?;
        locations = locations.with_trash_root(data.join(TRASH_DIR)).map_err(invalid)?;
    }
    if let Some(state) = &efr.state {
        locations = locations.with_efr_root(state, []).map_err(invalid)?;
    }
    for binary in sandbox.host.binaries.iter().chain(&efr.binaries) {
        locations = locations.with_efr_binary(binary).map_err(invalid)?;
    }
    Ok(locations)
}

/// The write roots of the `auto` sandbox besides scratch and the private tmp: the
/// registered projects that `sandbox.write_projects` lets a call write and the roots of
/// `sandbox.write_roots`, never one at or above the home directory.
pub(crate) fn write_roots(home: &Path, settings: &Settings, projects: &Registry) -> Vec<PathBuf> {
    let sandbox = &settings.sandbox;
    let mut roots: Vec<PathBuf> = Vec::new();
    // NOTE: with `turn`, only the turn's own project is a root, and the engine knows
    // the turn's project from its scope; it needs no envelope root for it.
    if sandbox.write_projects != WriteProjects::Turn {
        roots.extend(projects.projects().iter().map(|project| project.root().to_path_buf()));
    }
    roots.extend(sandbox.write_roots.iter().map(|root| expand_home(root, home)));
    roots.retain(|root| root.is_absolute() && !too_wide(root, home));
    roots
}

/// `locations` with what the `auto` sandbox adds: its envelope roots (write roots,
/// caches, the private tmp), the synced folders, the floors that efrd knows (the
/// user's, `$ZDOTDIR`'s startup files, `PATH` dirs in a write root, dotfile link
/// targets) and the masks (the user's, `$XAUTHORITY`, `$HISTFILE`). A path the engine
/// refuses only costs a warning.
fn with_sandbox(
    mut locations: Locations,
    home: &Path,
    settings: &Settings,
    projects: &Registry,
    facts: &SandboxFacts,
) -> Locations {
    let sandbox = &settings.sandbox;
    let roots = write_roots(home, settings, projects);
    let expand = |paths: &[PathBuf]| -> Vec<PathBuf> {
        paths.iter().map(|path| expand_home(path, home)).filter(|path| path.is_absolute()).collect()
    };
    let add = |locations: Locations,
               path: PathBuf,
               what: &str,
               step: fn(Locations, PathBuf) -> Result<Locations, PermissionsError>| {
        let kept = locations.clone();
        match step(locations, path.clone()) {
            Ok(next) => next,
            Err(error) => {
                tracing::warn!(error = %error, path = %path.display(), what, "the engine leaves out a sandbox path");
                kept
            }
        }
    };
    // NOTE: the private tmp and the cache overlays are write roots whose writes never
    // reach the real files, so a program there was not written in the sandbox. A tool
    // fills a cache again, so a delete there needs no user message that names it.
    for root in PRIVATE_ROOTS.iter().map(PathBuf::from) {
        locations = add(locations, root, "private root", |l, p| l.with_private_root(p));
    }
    for root in expand(&sandbox.caches) {
        locations = add(locations, root, "cache", |l, p| l.with_cache_root(p));
    }
    locations = locations.with_rebuildable_names(sandbox.rebuildable.iter().cloned());
    for root in roots.iter().cloned() {
        locations = add(locations, root, "envelope root", |l, p| l.with_envelope_root(p));
    }
    for dir in expand(&sandbox.synced_dirs) {
        locations = add(locations, dir, "synced folder", |l, p| l.with_synced_root(p));
    }
    // NOTE: a floor in a group may take a named file edit through the classifier once
    // efrd keeps backups; `sandbox.protect` and the floors of no known group stay with
    // the user. A dotfile link target takes the group of its link.
    for floor in expand(&sandbox.protect) {
        locations = add(locations, floor, "floor", |l, p| l.with_floor_root(p));
    }
    for (link, target) in &facts.links {
        let kept = locations.clone();
        locations = match locations.with_link_floor_root(target, link) {
            Ok(next) => next,
            Err(error) => {
                tracing::warn!(error = %error, path = %target.display(), what = "floor", "the engine leaves out a sandbox path");
                kept
            }
        };
    }
    // NOTE: a credential agent signs and logs in as the user, so a socket of one that
    // the model names asks the user, whatever the reviewer would say.
    for socket in &facts.host.agent_sockets {
        locations =
            add(locations, socket.clone(), "credential socket", |l, p| l.with_credential_socket(p));
    }
    if let Some(zdotdir) = &facts.host.zdotdir {
        for file in ZSH_STARTUP_FILES.iter().map(|name| zdotdir.join(name)) {
            locations =
                add(locations, file, "floor", |l, p| l.with_floor_group_root(p, FloorGroup::Zsh));
        }
    }
    let path_dirs = facts
        .host
        .path
        .split(':')
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && roots.iter().any(|root| is_within(dir, root)));
    for dir in path_dirs {
        locations =
            add(locations, dir, "floor", |l, p| l.with_floor_group_root(p, FloorGroup::Programs));
    }
    let mut masks = expand(&sandbox.mask);
    masks.extend(facts.host.xauthority.iter().cloned());
    masks.extend(facts.host.histfile.iter().cloned());
    for mask in masks {
        locations = add(locations, mask, "mask", |l, p| l.with_sandbox_mask(p));
    }
    locations
}

#[cfg(test)]
mod tests;
