//! The scope of a turn, derived again from the shell's working directory every turn.

use std::fmt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use efr_protocol::Scope;
use efr_scope::{Basis, Derivation, Git, Home, Registry};

/// Turns the working directory of a turn into its scope.
///
/// The turn asks once per turn and never caches the answer, because the user may
/// `cd /etc` between two prompts. Resolving cannot fail: when git or the registry
/// cannot be read, the scope is `Machine`, which widens nothing.
#[async_trait]
pub trait ScopeResolver: Send + Sync + fmt::Debug {
    /// The scope of a turn whose shell is in `cwd`, with the git work tree around it
    /// for the live-state preamble.
    async fn resolve(&self, cwd: &Path) -> Derivation;
}

/// The production resolver: `efr_scope::derive` over the project registry file, read
/// again on every turn, and guarded git discovery.
#[derive(Debug, Clone)]
pub struct GitScopeResolver {
    home: Home,
    git: Git,
    registry: Option<PathBuf>,
}

impl GitScopeResolver {
    /// A resolver for the user whose home is `home`, running `git`, with no registered
    /// projects until [`with_registry`](Self::with_registry) names the file.
    pub fn new(home: Home, git: Git) -> Self {
        GitScopeResolver { home, git, registry: None }
    }

    /// Reads the registered projects from `path` (`projects.toml`) on every turn. A
    /// missing file is an empty registry.
    #[must_use]
    pub fn with_registry(mut self, path: impl Into<PathBuf>) -> Self {
        self.registry = Some(path.into());
        self
    }

    async fn registry(&self) -> Registry {
        let Some(path) = self.registry.clone() else {
            return Registry::empty();
        };
        match tokio::task::spawn_blocking(move || Registry::load(&path)).await {
            Ok(Ok(registry)) => registry,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "the project registry could not be read; no project is registered this turn");
                Registry::empty()
            }
            Err(_) => {
                tracing::warn!(
                    "reading the project registry panicked; no project is registered this turn"
                );
                Registry::empty()
            }
        }
    }
}

#[async_trait]
impl ScopeResolver for GitScopeResolver {
    async fn resolve(&self, cwd: &Path) -> Derivation {
        let registry = self.registry().await;
        match efr_scope::derive(cwd, &self.home, &registry, &self.git).await {
            Ok(derivation) => derivation,
            Err(error) => {
                tracing::warn!(error = %error, "the scope could not be derived; using the machine scope");
                machine()
            }
        }
    }
}

/// The scope that widens nothing.
pub(crate) fn machine() -> Derivation {
    Derivation { scope: Scope::Machine, basis: Basis::Elsewhere, repo: None }
}

#[cfg(test)]
mod tests;
