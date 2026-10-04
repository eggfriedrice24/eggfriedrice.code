//! The home directory, and the lexical path form that every comparison uses.

use std::path::{Component, Path, PathBuf};

use crate::ScopeError;

/// The user's home directory, in its lexical normal form and with symbolic links
/// resolved.
///
/// It is always passed in, never read from the environment, so tests run against a
/// temporary home and never see the real one. Both forms are kept because git reports
/// resolved paths, and on some systems `~` itself is a link (`/home` to `/var/home`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    path: PathBuf,
    canonical: PathBuf,
}

impl Home {
    /// The home directory at `path`.
    ///
    /// Fails when `path` is relative or is `/`. When `path` cannot be resolved (it does
    /// not exist yet), the resolved form is the normal form.
    ///
    /// This resolves links on the file system and so blocks; the daemon builds the home
    /// once at startup, and an async caller builds it in `spawn_blocking`.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ScopeError> {
        let given = path.into();
        let Some(path) = normalize(&given) else {
            return Err(ScopeError::NotAbsolute { path: given });
        };
        if path.parent().is_none() {
            return Err(ScopeError::HomeIsRoot);
        }
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        Ok(Home { path, canonical })
    }

    /// The home directory as given, in normal form.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The home directory with symbolic links resolved.
    pub fn canonical(&self) -> &Path {
        &self.canonical
    }

    /// True when `dir` is the home directory, `/` or a directory above home, in either
    /// form: the directories that must never become a project root on their own.
    pub fn is_at_or_above(&self, dir: &Path) -> bool {
        self.path.starts_with(dir) || self.canonical.starts_with(dir)
    }

    /// A home with a given resolved form, so tests can describe a linked home without
    /// making the link.
    #[cfg(test)]
    pub(crate) fn linked(path: &str, canonical: &str) -> Self {
        Home { path: PathBuf::from(path), canonical: PathBuf::from(canonical) }
    }
}

/// The lexical normal form of an absolute path: `.` dropped, `..` removing the
/// component before it (never going above `/`), repeated separators collapsed. `None`
/// for a relative path.
pub(crate) fn normalize(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let mut normal = PathBuf::from("/");
    for component in components {
        match component {
            Component::Normal(name) => normal.push(name),
            Component::ParentDir => {
                normal.pop();
            }
            // A root or prefix cannot follow the first component of a Unix path.
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Some(normal)
}

#[cfg(test)]
mod tests;
