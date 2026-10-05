//! The permission engine, built in one place from the settings and the project
//! registry.
//!
//! The engine holds the home directory and its resolved form, the daemon's own secrets
//! (sealed, so no rule opens them), the secret paths of the settings (`~/` below the
//! home directory) and the registered projects, and decides by the built-in rules
//! followed by the user's. A tool call reads the latest engine from the watch channel in
//! `State`; whatever changes the rules, the secret paths, the projects or (later) the
//! mode builds a new one here and sends it there.

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
}

impl EngineParts {
    /// Reads the project registry and builds the engine for `settings`.
    pub(crate) async fn engine(&self, settings: &Settings) -> Result<Engine, DaemonError> {
        let projects = load_registry(&self.registry).await;
        build(&self.home, &self.secrets, settings, &projects)
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

/// The engine for `settings` and `projects`, with `secrets` sealed.
///
/// NOTE: the user's rules belong to the engine, the machine policy, and not to
/// `ConversationConfig::policy`: a conversation's rules may never open a secret or a
/// system path, and the user's explicit rules must be able to.
pub(crate) fn build(
    home: &Home,
    secrets: &Path,
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
    // NOTE: `permissions.mode` is not applied yet; every turn decides by the cautious
    // rules, which are the built-in ones.
    Ok(Engine::new(locations, settings.permissions.policy()))
}

#[cfg(test)]
mod tests;
