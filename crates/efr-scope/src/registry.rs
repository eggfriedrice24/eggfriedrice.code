//! The explicit project registry, `$XDG_CONFIG_HOME/efr/projects.toml`.
//!
//! A project exists only because the user listed it here. A `.git` directory alone
//! never makes a project, because `$HOME` may be a dotfiles work tree and a scratch
//! directory may sit inside one. The file is small and is read again every turn, so an
//! edit takes effect at the next `,` line without a restart:
//!
//! ```toml
//! [[project]]
//! id = "0192f0c1-7a00-7000-8000-000000000001"
//! root = "/home/me/p/eggfriedrice.code"
//! name = "eggfriedrice.code"
//! ```

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use efr_protocol::ProjectId;
use serde::{Deserialize, Serialize};

use crate::ScopeError;
use crate::home::normalize;

mod edit;

pub use edit::RegistryEdit;

/// The registry's file name inside the efr config directory.
pub const REGISTRY_FILE: &str = "projects.toml";

/// One registered project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    id: ProjectId,
    root: PathBuf,
    name: Option<String>,
}

impl Project {
    /// The project's id, which `Scope::Project` carries.
    pub fn id(&self) -> ProjectId {
        self.id
    }

    /// The project's root directory: absolute, in lexical normal form, valid UTF-8.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A name for people, when the file gives one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// A rule of the registry that a file or a new project breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryProblem {
    /// A project root is a relative path.
    RootNotAbsolute {
        /// The root.
        root: PathBuf,
    },
    /// A project root is not valid UTF-8, which a TOML file cannot hold.
    RootNotUnicode {
        /// The root.
        root: PathBuf,
    },
    /// Two projects have the same id.
    DuplicateId {
        /// The id.
        id: ProjectId,
    },
    /// Two projects have the same root.
    DuplicateRoot {
        /// The root, in normal form.
        root: PathBuf,
    },
}

impl fmt::Display for RegistryProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryProblem::RootNotAbsolute { root } => {
                write!(f, "the root {} is not an absolute path", root.display())
            }
            RegistryProblem::RootNotUnicode { root } => {
                write!(f, "the root {} is not valid UTF-8", root.display())
            }
            RegistryProblem::DuplicateId { id } => write!(f, "the id {id} is registered twice"),
            RegistryProblem::DuplicateRoot { root } => {
                write!(f, "the root {} is registered twice", root.display())
            }
        }
    }
}

/// The registered projects, in the order of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    projects: Vec<Project>,
}

/// The file's shape. Unknown keys are refused, as in `config.toml`, so a misspelt key
/// is an error instead of a silently missing project.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    #[serde(default, rename = "project")]
    projects: Vec<ProjectEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectEntry {
    id: ProjectId,
    root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

impl Registry {
    /// A registry with no projects.
    pub fn empty() -> Self {
        Registry::default()
    }

    /// The registry file in the efr config directory `config_dir`.
    pub fn path_in(config_dir: &Path) -> PathBuf {
        config_dir.join(REGISTRY_FILE)
    }

    /// Reads the registry file at `path`. A missing file is an empty registry.
    ///
    /// This blocks on the file system; async callers read it in `spawn_blocking`.
    pub fn load(path: &Path) -> Result<Self, ScopeError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Registry::from_toml(&text, path),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Registry::empty()),
            Err(source) => Err(ScopeError::ReadRegistry { path: path.to_path_buf(), source }),
        }
    }

    /// Parses the text of a registry file. `path` names the file in errors.
    pub(crate) fn from_toml(text: &str, path: &Path) -> Result<Self, ScopeError> {
        let file: RegistryFile = toml::from_str(text)
            .map_err(|source| ScopeError::ParseRegistry { path: path.to_path_buf(), source })?;
        let mut registry = Registry::empty();
        for entry in file.projects {
            registry.insert(entry.id, entry.root, entry.name).map_err(|problem| {
                ScopeError::InvalidRegistry { path: path.to_path_buf(), problem }
            })?;
        }
        Ok(registry)
    }

    /// The registry as the text of a registry file.
    pub fn to_toml(&self) -> Result<String, ScopeError> {
        let file = RegistryFile {
            projects: self
                .projects
                .iter()
                .map(|project| ProjectEntry {
                    id: project.id,
                    root: project.root.clone(),
                    name: project.name.clone(),
                })
                .collect(),
        };
        toml::to_string(&file).map_err(|source| ScopeError::SerializeRegistry { source })
    }

    /// Writes the registry to `path` atomically (mode 0600), creating the parent
    /// directory when it is missing.
    ///
    /// This blocks on the file system; async callers write it in `spawn_blocking`.
    pub fn save(&self, path: &Path) -> Result<(), ScopeError> {
        let text = self.to_toml()?;
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|source| ScopeError::CreateDir { path: parent.to_path_buf(), source })?;
        }
        efr_stdx::fs::write_atomic(path, text.as_bytes())
            .map_err(|source| ScopeError::WriteRegistry { path: path.to_path_buf(), source })
    }

    /// Adds a project. The caller mints `id`, normally as
    /// `ProjectId::from_uuid(efr_stdx::id::uuid_v7(clock, rng))`.
    ///
    /// Registering `$HOME` or `/` is allowed: it is the one way to make either a
    /// project. Fails when `root` is relative or not UTF-8, or when the id or the root
    /// is registered already.
    pub fn register(
        &mut self,
        id: ProjectId,
        root: impl Into<PathBuf>,
        name: Option<String>,
    ) -> Result<&Project, ScopeError> {
        self.insert(id, root.into(), name).map_err(|problem| ScopeError::InvalidProject { problem })
    }

    /// Removes the project `id` and returns it.
    pub fn remove(&mut self, id: &ProjectId) -> Option<Project> {
        let index = self.projects.iter().position(|project| project.id == *id)?;
        Some(self.projects.remove(index))
    }

    /// The project `id`.
    pub fn get(&self, id: &ProjectId) -> Option<&Project> {
        self.projects.iter().find(|project| project.id == *id)
    }

    /// Every project, in the order of the file.
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    /// True when no project is registered.
    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }

    /// The project whose root holds `path`, the deepest root when roots are nested.
    /// `None` for a relative path. The comparison is lexical and by whole components,
    /// so `/p/app` does not hold `/p/application`.
    pub fn containing(&self, path: &Path) -> Option<&Project> {
        let path = normalize(path)?;
        self.projects
            .iter()
            .filter(|project| path.starts_with(&project.root))
            .max_by_key(|project| project.root.components().count())
    }

    fn insert(
        &mut self,
        id: ProjectId,
        root: PathBuf,
        name: Option<String>,
    ) -> Result<&Project, RegistryProblem> {
        if root.to_str().is_none() {
            return Err(RegistryProblem::RootNotUnicode { root });
        }
        let Some(root) = normalize(&root) else {
            return Err(RegistryProblem::RootNotAbsolute { root });
        };
        if self.get(&id).is_some() {
            return Err(RegistryProblem::DuplicateId { id });
        }
        if self.projects.iter().any(|project| project.root == root) {
            return Err(RegistryProblem::DuplicateRoot { root });
        }
        self.projects.push(Project { id, root, name });
        match self.projects.last() {
            Some(project) => Ok(project),
            None => unreachable!("a project was pushed on the line above"),
        }
    }
}

#[cfg(test)]
mod tests;
