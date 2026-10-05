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

use efr_config::Settings;
use efr_permissions::{Engine, Locations};
use efr_scope::{Home, Registry};

use crate::DaemonError;

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
        build(&self.home, &self.secrets, &protected, settings, &projects)
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
    Ok(Engine::with_rules(locations, settings.permissions.rules.clone()))
}

#[cfg(test)]
mod tests;
