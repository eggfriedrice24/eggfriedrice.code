//! `derive(cwd) -> Scope`, run again on every turn.
//!
//! The scope follows the shell: the user may `cd /etc` between two `,` lines, so the
//! daemon derives it from the `ShellContext` of each turn and never caches it. The
//! rules, in order:
//!
//! 1. inside a registered project root (the deepest one): `Project`, even for `$HOME`
//!    or `/`, because registering is the one explicit way to make them a project;
//! 2. `$HOME`, `/` or a directory above `$HOME`: `Machine`;
//! 3. inside a git work tree, after the guards of [`Git::discover`]: `Path(root)`;
//! 4. anything else: `Machine`.

use std::path::Path;

use efr_protocol::Scope;

use crate::git::Discovery;
use crate::home::normalize;
use crate::{Git, Home, Registry, Repo, ScopeError};

/// The scope of a turn and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivation {
    /// The scope.
    pub scope: Scope,
    /// Which rule gave it.
    pub basis: Basis,
    /// The git work tree around the directory, when there is one that is not guarded,
    /// for the live-state preamble (root and branch). It is set for a registered
    /// project too.
    pub repo: Option<Repo>,
}

/// Which rule of [`derive()`] gave the scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Basis {
    /// The directory is inside a registered project root.
    Registered,
    /// The directory is `$HOME`, `/` or above `$HOME`, or its work tree's root is one
    /// of them.
    HomeOrRoot,
    /// The directory is inside a git work tree.
    WorkTree,
    /// None of the above.
    Elsewhere,
}

/// The scope of a turn whose shell is in `cwd`.
///
/// `cwd` is matched against the registry both as given and with symbolic links
/// resolved, because the shell reports its logical directory. The registry is passed
/// in, so the caller decides how often to read the file; the daemon reads it every
/// turn. The file system is read on tokio's blocking pool (see [`Git`]), never on the
/// calling task. Fails when `cwd` is relative, when git cannot run, or when git or a look
/// at the file system does not finish in time; the caller then uses `Machine`, which
/// widens nothing.
pub async fn derive(
    cwd: &Path,
    home: &Home,
    registry: &Registry,
    git: &Git,
) -> Result<Derivation, ScopeError> {
    let Some(cwd) = normalize(cwd) else {
        return Err(ScopeError::NotAbsolute { path: cwd.to_path_buf() });
    };
    let target = cwd.clone();
    let resolved = git.probe(&cwd, move || std::fs::canonicalize(target).ok()).await?;
    let forms = || std::iter::once(cwd.as_path()).chain(resolved.as_deref());

    let project = forms().find_map(|form| registry.containing(form));
    let at_or_above_home = forms().any(|form| home.is_at_or_above(form));
    // git from $HOME or above can only find a guarded root, so it is not asked.
    let discovery =
        if at_or_above_home { Discovery::NotARepository } else { git.discover(&cwd, home).await? };

    let derivation = match (project, discovery) {
        (Some(project), discovery) => Derivation {
            scope: Scope::Project(project.id()),
            basis: Basis::Registered,
            repo: discovery.work_tree().cloned(),
        },
        (None, _) if at_or_above_home => {
            Derivation { scope: Scope::Machine, basis: Basis::HomeOrRoot, repo: None }
        }
        (None, Discovery::WorkTree(repo)) => Derivation {
            scope: Scope::Path(repo.root.clone()),
            basis: Basis::WorkTree,
            repo: Some(repo),
        },
        (None, Discovery::Guarded { .. }) => {
            Derivation { scope: Scope::Machine, basis: Basis::HomeOrRoot, repo: None }
        }
        (None, Discovery::NotARepository) => {
            Derivation { scope: Scope::Machine, basis: Basis::Elsewhere, repo: None }
        }
    };
    Ok(derivation)
}

#[cfg(test)]
mod tests;
