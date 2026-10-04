//! The five path classes and the locations that define them.
//!
//! The engine judges a call by the paths that it touches, never by the shell's working
//! directory: a shell in `~` must not make every file in `~` writable. Classification
//! is lexical. `.` and `..` are resolved by name and nothing is read from the disk, so
//! the caller (the tool) resolves symbolic links before it declares a path.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use efr_protocol::ProjectId;
use serde::{Deserialize, Serialize};

use crate::PermissionsError;

/// Where a path lives, which sets how freely a call may use it.
///
/// | Class | Examples | Read | Write |
/// |---|---|---|---|
/// | `Scratch` | the conversation's `$SCRATCH` | free | free |
/// | `UserConfig` | `~/.config`, `~/.zshrc`, other dot entries in `~` | free | approval |
/// | `UserData` | `~/Documents`, `~/p`, `~/.local/share`, `~/.cache` | free | approval, or free inside the turn's registered project |
/// | `System` | everything outside `~`: `/etc`, `/usr`, `/srv` | free | approval |
/// | `Secrets` | `~/.ssh`, `~/.gnupg`, password stores, the daemon's `secrets/` | denied | denied |
///
/// The enum is deliberately exhaustive: a new class must make every consumer decide
/// what it means, instead of falling into a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathClass {
    /// The conversation's own `$SCRATCH` directory.
    Scratch,
    /// The user's configuration: dot entries in `~` and the configured config roots.
    UserConfig,
    /// The user's files: everything else in `~`.
    UserData,
    /// Everything outside `~`.
    System,
    /// Keys, password stores and credentials. The default policy denies them.
    Secrets,
}

impl PathClass {
    /// Every class, from the freest to the most guarded.
    pub const ALL: [PathClass; 5] = [
        PathClass::Scratch,
        PathClass::UserConfig,
        PathClass::UserData,
        PathClass::System,
        PathClass::Secrets,
    ];

    /// The name of the class in a policy file, such as `user_config`.
    pub const fn as_str(self) -> &'static str {
        match self {
            PathClass::Scratch => "scratch",
            PathClass::UserConfig => "user_config",
            PathClass::UserData => "user_data",
            PathClass::System => "system",
            PathClass::Secrets => "secrets",
        }
    }
}

/// Words for messages to the user and the model, such as `user config`.
impl fmt::Display for PathClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PathClass::Scratch => "scratch",
            PathClass::UserConfig => "user config",
            PathClass::UserData => "user data",
            PathClass::System => "system",
            PathClass::Secrets => "secrets",
        })
    }
}

/// Secret locations relative to the home directory. Checked before every other class,
/// so `~/.local/share/keyrings` is a secret although `~/.local/share` is user data.
const HOME_SECRETS: &[&str] =
    &[".ssh", ".gnupg", ".password-store", ".local/share/keyrings", ".netrc"];

/// Secret locations outside the home directory.
const SYSTEM_SECRETS: &[&str] = &["/etc/shadow", "/etc/gshadow"];

/// Dot entries of the home directory that hold data, not configuration.
const HOME_DATA: &[&str] = &[".local/share", ".local/state", ".cache"];

/// The machine facts that classification needs: the home directory, extra secret and
/// configuration roots, and the roots of the registered projects.
///
/// The daemon builds this from its directories and the project registry, and builds a
/// new [`Engine`](crate::Engine) when the registry changes. Every path is stored in its
/// lexical normal form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locations {
    home: PathBuf,
    secret_roots: Vec<PathBuf>,
    user_config_roots: Vec<PathBuf>,
    projects: BTreeMap<ProjectId, PathBuf>,
}

impl Locations {
    /// Locations for a user whose home directory is `home`.
    ///
    /// Fails when `home` is relative, or when it is `/`, which would make every path on
    /// the machine user data.
    pub fn new(home: impl Into<PathBuf>) -> Result<Self, PermissionsError> {
        let home = absolute(home.into())?;
        if home.parent().is_none() {
            return Err(PermissionsError::HomeIsRoot);
        }
        Ok(Locations {
            home,
            secret_roots: Vec::new(),
            user_config_roots: Vec::new(),
            projects: BTreeMap::new(),
        })
    }

    /// Adds a directory or file whose contents are secret, such as the daemon's
    /// `secrets/` directory.
    pub fn with_secret_root(mut self, root: impl Into<PathBuf>) -> Result<Self, PermissionsError> {
        self.secret_roots.push(absolute(root.into())?);
        Ok(self)
    }

    /// Adds a directory that counts as user configuration wherever it is, such as a
    /// dotfiles repository or a config directory outside `~`.
    pub fn with_user_config_root(
        mut self,
        root: impl Into<PathBuf>,
    ) -> Result<Self, PermissionsError> {
        self.user_config_roots.push(absolute(root.into())?);
        Ok(self)
    }

    /// Records the root of the registered project `id`, so that a turn whose scope is
    /// that project may write the project's user data without approval.
    pub fn with_project(
        mut self,
        id: ProjectId,
        root: impl Into<PathBuf>,
    ) -> Result<Self, PermissionsError> {
        self.projects.insert(id, absolute(root.into())?);
        Ok(self)
    }

    /// The home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The root of the registered project `id`, if it was added.
    pub fn project_root(&self, id: &ProjectId) -> Option<&Path> {
        self.projects.get(id).map(PathBuf::as_path)
    }

    /// The class of `path` for a conversation whose `$SCRATCH` is `scratch`, or `None`
    /// when `path` is relative.
    pub fn classify(&self, path: &Path, scratch: &Path) -> Option<PathClass> {
        let path = normalize(path)?;
        Some(self.classify_normal(&path, self.scratch_root(scratch).as_deref()))
    }

    /// The class of a path that is already in normal form, with the scratch root from
    /// [`Locations::scratch_root`].
    pub(crate) fn classify_normal(&self, path: &Path, scratch: Option<&Path>) -> PathClass {
        if self.is_secret(path) {
            return PathClass::Secrets;
        }
        if scratch.is_some_and(|scratch| path.starts_with(scratch)) {
            return PathClass::Scratch;
        }
        if self.user_config_roots.iter().any(|root| path.starts_with(root)) {
            return PathClass::UserConfig;
        }
        match path.strip_prefix(&self.home) {
            Ok(relative) => classify_in_home(relative),
            Err(_) => PathClass::System,
        }
    }

    /// The conversation's scratch directory in normal form, when it may count as
    /// scratch at all.
    ///
    /// NOTE: a scratch path of `/`, `~` or a directory above `~` would turn every user
    /// file into free scratch, so such a path makes nothing scratch.
    pub(crate) fn scratch_root(&self, scratch: &Path) -> Option<PathBuf> {
        normalize(scratch).filter(|scratch| !self.home.starts_with(scratch))
    }

    /// The root of the registered project `id`, when it may widen what a turn can do.
    ///
    /// NOTE: `~`, `/` and the directories above `~` are never treated as a project here,
    /// even when the registry lists one of them: that would make every user file
    /// writable without approval.
    pub(crate) fn widening_project_root(&self, id: &ProjectId) -> Option<&Path> {
        self.project_root(id).filter(|root| !self.home.starts_with(root))
    }

    fn is_secret(&self, path: &Path) -> bool {
        HOME_SECRETS.iter().any(|secret| path.starts_with(self.home.join(secret)))
            || SYSTEM_SECRETS.iter().any(|secret| path.starts_with(secret))
            || self.secret_roots.iter().any(|root| path.starts_with(root))
    }
}

/// The class of a path inside the home directory, given relative to it.
fn classify_in_home(relative: &Path) -> PathClass {
    if HOME_DATA.iter().any(|data| relative.starts_with(data)) {
        return PathClass::UserData;
    }
    match relative.components().next() {
        Some(Component::Normal(first)) if first.as_encoded_bytes().starts_with(b".") => {
            PathClass::UserConfig
        }
        _ => PathClass::UserData,
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

fn absolute(path: PathBuf) -> Result<PathBuf, PermissionsError> {
    normalize(&path).ok_or(PermissionsError::NotAbsolute { path })
}

#[cfg(test)]
mod tests;
