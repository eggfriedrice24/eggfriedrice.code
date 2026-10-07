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

use efr_config::{Settings, WriteProjects};
use efr_permissions::{AutoSupport, Engine, Locations, PermissionsError};
use efr_sandbox::{expand_home, is_within, too_wide};
use efr_scope::{Home, Registry};

use crate::DaemonError;
use crate::sandbox::HostFacts;
use crate::sandbox::links::link_targets;

/// The places that every contained call may write besides the projects: the private
/// `/tmp`, `/var/tmp` and `/dev/shm`.
const PRIVATE_ROOTS: &[&str] = &["/tmp", "/var/tmp", "/dev/shm"];

/// The zsh startup files that `$ZDOTDIR` holds.
const ZSH_STARTUP: &[&str] = &[".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"];

/// What the engine needs from the hidden shells and the file system for the `auto`
/// sandbox: the shells' `PATH`, `$ZDOTDIR`, `$XAUTHORITY` and `$HISTFILE`, and the
/// dotfile link targets that lie in a write root.
#[derive(Debug, Clone, Default)]
pub(crate) struct SandboxFacts {
    pub(crate) host: HostFacts,
    pub(crate) link_targets: Vec<PathBuf>,
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
        let links = tokio::task::spawn_blocking(move || link_targets(&home, &roots))
            .await
            .unwrap_or_default();
        let facts = SandboxFacts { host: self.host.clone(), link_targets: links };
        build(&self.home, &self.secrets, &protected, settings, &projects, &facts)
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
    // NOTE: phase 1 has no proxy, no bus proxy and no undo; later phases set them here.
    Ok(Engine::with_rules(locations, settings.permissions.rules.clone())
        .with_support(AutoSupport::default()))
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
    let envelope: Vec<PathBuf> = PRIVATE_ROOTS
        .iter()
        .map(PathBuf::from)
        .chain(expand(&sandbox.caches))
        .chain(roots.iter().cloned())
        .collect();
    for root in envelope {
        locations = add(locations, root, "envelope root", |l, p| l.with_envelope_root(p));
    }
    for dir in expand(&sandbox.synced_dirs) {
        locations = add(locations, dir, "synced folder", |l, p| l.with_synced_root(p));
    }
    let mut floors = expand(&sandbox.protect);
    if let Some(zdotdir) = &facts.host.zdotdir {
        floors.extend(ZSH_STARTUP.iter().map(|name| zdotdir.join(name)));
    }
    floors.extend(
        facts
            .host
            .path
            .split(':')
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute() && roots.iter().any(|root| is_within(dir, root))),
    );
    floors.extend(facts.link_targets.iter().cloned());
    for floor in floors {
        locations = add(locations, floor, "floor", |l, p| l.with_floor_root(p));
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
