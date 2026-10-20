//! The deletes that full mode guards: a delete or a move of a tree that the user must
//! approve at their own machine, whatever the rules say (efr's full mode, DF4). The
//! detector that finds them in a line is `exits::critical`; these are its types.

use std::fmt;
use std::path::{Path, PathBuf};

/// One delete or move of a line that full mode guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriticalDelete {
    /// The simple command, its words joined by spaces.
    pub part: String,
    /// What it deletes.
    pub target: CriticalTarget,
}

/// The target of a [`CriticalDelete`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CriticalTarget {
    /// A path that efr reads, and why a delete of it is critical.
    Path {
        /// The path, absolute.
        path: PathBuf,
        /// Why a delete of it is critical.
        why: CriticalWhy,
    },
    /// A word whose path efr cannot read, as the line wrote it.
    Unreadable {
        /// The word.
        word: String,
    },
}

/// Why a delete of a path is critical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CriticalWhy {
    /// The path is `/`.
    Root,
    /// The path is the home directory.
    Home,
    /// The path holds the home directory.
    AboveHome,
    /// A system directory outside the home directory, such as `/etc` or `/usr/lib`.
    SystemDir,
    /// The path holds two or more projects or write roots.
    HoldsProjects,
    /// The path holds a file that runs code later, such as a shell startup file.
    HoldsFloor,
    /// The path holds efr's config or its own files.
    HoldsEfr,
    /// The path is or holds a secret.
    HoldsSecret,
    /// The path is the root of a registered project or of the turn's project.
    ProjectRoot,
}

impl CriticalWhy {
    /// Why, in a few words for the user, such as `a system directory`.
    pub const fn describe(self) -> &'static str {
        match self {
            CriticalWhy::Root => "the root directory",
            CriticalWhy::Home => "your home directory",
            CriticalWhy::AboveHome => "it holds your home directory",
            CriticalWhy::SystemDir => "a system directory",
            CriticalWhy::HoldsProjects => "it holds more than one project",
            CriticalWhy::HoldsFloor => "it holds files that run code later",
            CriticalWhy::HoldsEfr => "it holds efr's config or files",
            CriticalWhy::HoldsSecret => "it holds a secret",
            CriticalWhy::ProjectRoot => "a project",
        }
    }
}

impl CriticalDelete {
    /// The target and why, for the user, with a path below `home` as `~/...`, such as
    /// `~ (your home directory)` or `"$X"/* (efr cannot read the target)`.
    pub fn describe(&self, home: &Path) -> String {
        match &self.target {
            CriticalTarget::Path { path, why } => {
                format!("{} ({})", shown(path, home), why.describe())
            }
            CriticalTarget::Unreadable { word } => format!("{word} (efr cannot read the target)"),
        }
    }
}

impl fmt::Display for CriticalTarget {
    /// The target as the engine reads it, with the path in full.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CriticalTarget::Path { path, why } => {
                write!(f, "{} ({})", path.display(), why.describe())
            }
            CriticalTarget::Unreadable { word } => {
                write!(f, "{word} (efr cannot read the target)")
            }
        }
    }
}

/// `path` with `home` as `~`.
fn shown(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests;
