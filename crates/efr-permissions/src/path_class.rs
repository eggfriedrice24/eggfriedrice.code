//! The five path classes and the locations that define them.
//!
//! The engine judges a call by the paths that it touches, never by the shell's working
//! directory: a shell in `~` must not make every file in `~` writable. Classification
//! is lexical. `.` and `..` are resolved by name and nothing is read from the disk, so
//! the caller (the tool) resolves symbolic links before it declares a path. A home
//! directory reached through a link therefore arrives in its resolved form, and
//! [`Locations`] knows every form of it.

use std::borrow::Cow;
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
/// | `Secrets` | `~/.ssh`, `~/.gnupg`, password stores, credential files such as `~/.aws/credentials`, the daemon's `secrets/` | denied | denied |
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
/// Besides keys and password stores, the files where common tools keep credentials:
/// read freely as user config, they would go to the model provider with the turn.
const HOME_SECRETS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".password-store",
    ".local/share/keyrings",
    ".netrc",
    // Cloud, container and cluster credentials.
    ".aws/credentials",
    ".aws/sso/cache",
    ".azure",
    ".config/gcloud",
    ".docker/config.json",
    ".kube/config",
    // Tokens of developer tools and package registries.
    ".codex/auth.json",
    ".git-credentials",
    ".config/gh/hosts.yml",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".gem/credentials",
    ".vault-token",
    ".terraform.d/credentials.tfrc.json",
];

/// Secret locations outside the home directory.
const SYSTEM_SECRETS: &[&str] = &["/etc/shadow", "/etc/gshadow"];

/// Dot entries of the home directory that hold data, not configuration.
const HOME_DATA: &[&str] = &[".local/share", ".local/state", ".cache"];

/// The machine facts that classification needs: the home directory in every form it
/// has, extra secret and configuration roots, and the roots of the registered projects.
///
/// The daemon builds this from its directories and the project registry, and builds a
/// new [`Engine`](crate::Engine) when the registry changes. Every path is stored in its
/// lexical normal form.
///
/// Tools resolve symbolic links before they declare a path, so when `/home` links to
/// `/var/home`, a tool declares `/var/home/u/.ssh/id_ed25519` and never
/// `/home/u/.ssh/id_ed25519`. The daemon therefore passes `efr_scope::Home::path()` to
/// [`Locations::new`] and `efr_scope::Home::canonical()` to
/// [`Locations::with_home_alias`]. A path under an alias is classified as the same path
/// under the home directory, and a root under an alias is stored that way, so every
/// check compares one form. A root outside the home directory whose resolved form
/// differs is added in both forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locations {
    home: PathBuf,
    home_aliases: Vec<PathBuf>,
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
            home_aliases: Vec::new(),
            secret_roots: Vec::new(),
            user_config_roots: Vec::new(),
            projects: BTreeMap::new(),
        })
    }

    /// Adds another form of the home directory, such as its form with symbolic links
    /// resolved, so that a path a tool resolved through the link is classified like the
    /// same path under the home directory.
    ///
    /// An alias equal to the home directory changes nothing. Fails when `alias` is
    /// relative or `/`, or when it lies inside or above the home directory or another
    /// alias: then one form of a path would sit inside another, and `/home` as an alias
    /// of `/home/u` would make every user's files on the machine this user's.
    pub fn with_home_alias(mut self, alias: impl Into<PathBuf>) -> Result<Self, PermissionsError> {
        let alias = absolute(alias.into())?;
        if alias.parent().is_none() {
            return Err(PermissionsError::HomeIsRoot);
        }
        if self.home_forms().any(|form| *form == alias) {
            return Ok(self);
        }
        if self.home_forms().any(|form| form.starts_with(&alias) || alias.starts_with(form)) {
            return Err(PermissionsError::HomeAliasOverlaps { alias });
        }
        self.home_aliases.push(alias);
        // NOTE: a root added before the alias may be in the alias's form. Rewriting the
        // roots now keeps the order of the builder calls from mattering.
        let Locations { home, home_aliases, secret_roots, user_config_roots, projects } = &mut self;
        for root in secret_roots.iter_mut().chain(user_config_roots).chain(projects.values_mut()) {
            if let Cow::Owned(rehomed) = rehome(home, home_aliases, root) {
                *root = rehomed;
            }
        }
        Ok(self)
    }

    /// Adds a directory or file whose contents are secret, such as the daemon's
    /// `secrets/` directory.
    pub fn with_secret_root(mut self, root: impl Into<PathBuf>) -> Result<Self, PermissionsError> {
        let root = self.absolute_root(root.into())?;
        self.secret_roots.push(root);
        Ok(self)
    }

    /// Adds a directory that counts as user configuration wherever it is, such as a
    /// dotfiles repository or a config directory outside `~`.
    pub fn with_user_config_root(
        mut self,
        root: impl Into<PathBuf>,
    ) -> Result<Self, PermissionsError> {
        let root = self.absolute_root(root.into())?;
        self.user_config_roots.push(root);
        Ok(self)
    }

    /// Records the root of the registered project `id`, so that a turn whose scope is
    /// that project may write the project's user data without approval.
    pub fn with_project(
        mut self,
        id: ProjectId,
        root: impl Into<PathBuf>,
    ) -> Result<Self, PermissionsError> {
        let root = self.absolute_root(root.into())?;
        self.projects.insert(id, root);
        Ok(self)
    }

    /// The home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The other forms of the home directory, from [`Locations::with_home_alias`].
    pub fn home_aliases(&self) -> &[PathBuf] {
        &self.home_aliases
    }

    /// The root of the registered project `id`, if it was added. A root under a home
    /// alias is reported under the home directory.
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
        let path = self.rehome(path);
        let path = path.as_ref();
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

    /// The conversation's scratch directory in normal form, under the home directory
    /// when it lies under an alias, when it may count as scratch at all.
    ///
    /// NOTE: a scratch path of `/`, `~` or a directory above `~`, in any form of `~`,
    /// would turn every user file into free scratch, so such a path makes nothing
    /// scratch.
    pub(crate) fn scratch_root(&self, scratch: &Path) -> Option<PathBuf> {
        normalize(scratch)
            .map(|scratch| self.rehome(&scratch).into_owned())
            .filter(|scratch| !self.is_at_or_above_home(scratch))
    }

    /// The root of the registered project `id`, when it may widen what a turn can do.
    ///
    /// NOTE: `~`, `/` and the directories above `~`, in any form of `~`, are never
    /// treated as a project here, even when the registry lists one of them: that would
    /// make every user file writable without approval.
    pub(crate) fn widening_project_root(&self, id: &ProjectId) -> Option<&Path> {
        self.project_root(id).filter(|root| !self.is_at_or_above_home(root))
    }

    /// `path` with a leading home alias replaced by the home directory.
    fn rehome<'p>(&self, path: &'p Path) -> Cow<'p, Path> {
        rehome(&self.home, &self.home_aliases, path)
    }

    /// The home directory and its aliases.
    fn home_forms(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.home).chain(&self.home_aliases)
    }

    /// True when `dir` is a form of the home directory, `/`, or a directory above a form
    /// of the home directory.
    fn is_at_or_above_home(&self, dir: &Path) -> bool {
        self.home_forms().any(|form| form.starts_with(dir))
    }

    /// An absolute root in normal form, under the home directory when it lies under an
    /// alias.
    fn absolute_root(&self, root: PathBuf) -> Result<PathBuf, PermissionsError> {
        let root = absolute(root)?;
        Ok(self.rehome(&root).into_owned())
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

/// `path` with a leading alias of `home` replaced by `home`; `path` itself when it lies
/// under no alias. At most one alias can match, because
/// [`Locations::with_home_alias`] refuses aliases inside one another.
pub(crate) fn rehome<'p>(home: &Path, aliases: &[PathBuf], path: &'p Path) -> Cow<'p, Path> {
    for alias in aliases {
        if let Ok(rest) = path.strip_prefix(alias) {
            // `join` with an empty path would add a trailing separator.
            let rehomed =
                if rest.as_os_str().is_empty() { home.to_path_buf() } else { home.join(rest) };
            return Cow::Owned(rehomed);
        }
    }
    Cow::Borrowed(path)
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
