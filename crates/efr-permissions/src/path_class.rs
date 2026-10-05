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
/// | `UserConfig` | `~/.config`, `~/.zshrc`, other dot entries in `~`, a repository's `.git` in `~` or `$SCRATCH` | free | approval |
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
    /// The user's configuration: dot entries in `~`, the configured config roots, and
    /// the `.git` of a repository that would otherwise be user data or scratch, whose
    /// config names programs that git runs.
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

/// Entries of a process directory under `/proc` that expose its environment, its
/// memory, its open files or its view of the file system. Reading through them would
/// bypass every other class: `/proc/self/root/home/u/.ssh/id_ed25519` is the key, and
/// `/proc/self/environ` holds the tokens that `env` would print.
const PROC_SECRETS: &[&str] = &["environ", "root", "cwd", "fd", "map_files", "mem"];

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
    /// The secret roots that no rule opens; each is in `secret_roots` too.
    sealed_roots: Vec<PathBuf>,
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
            sealed_roots: Vec::new(),
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
        let Locations {
            home,
            home_aliases,
            secret_roots,
            sealed_roots,
            user_config_roots,
            projects,
        } = &mut self;
        let roots = secret_roots.iter_mut().chain(sealed_roots).chain(user_config_roots);
        for root in roots.chain(projects.values_mut()) {
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

    /// Adds a directory or file whose contents are secret and that no rule opens, not
    /// even the user's: the daemon's own `secrets/`, whose tokens would let whoever
    /// reads them act as the user at the model provider.
    pub fn with_sealed_root(mut self, root: impl Into<PathBuf>) -> Result<Self, PermissionsError> {
        let root = self.absolute_root(root.into())?;
        self.secret_roots.push(root.clone());
        self.sealed_roots.push(root);
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
        let class = if scratch.is_some_and(|scratch| path.starts_with(scratch)) {
            PathClass::Scratch
        } else if self.user_config_roots.iter().any(|root| path.starts_with(root)) {
            PathClass::UserConfig
        } else {
            match path.strip_prefix(&self.home) {
                Ok(relative) => classify_in_home(relative),
                Err(_) => PathClass::System,
            }
        };
        // NOTE: `git status`, `git diff` and `git log` run the programs that a
        // repository's own config names (core.fsmonitor, filter drivers, diff.external),
        // so writing a `.git` is changing what an allowed command runs. Inside the
        // turn's project or `$SCRATCH` it would otherwise be free to write.
        match class {
            PathClass::Scratch | PathClass::UserData if in_repository_metadata(path) => {
                PathClass::UserConfig
            }
            other => other,
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

    /// True when `path`, in normal form, is a sealed root or lies below one, in any
    /// form of the home directory.
    pub(crate) fn is_sealed(&self, path: &Path) -> bool {
        let path = self.rehome(path);
        self.sealed_roots.iter().any(|root| path.starts_with(root))
    }

    fn is_secret(&self, path: &Path) -> bool {
        HOME_SECRETS.iter().any(|secret| path.starts_with(self.home.join(secret)))
            || SYSTEM_SECRETS.iter().any(|secret| path.starts_with(secret))
            || self.secret_roots.iter().any(|root| path.starts_with(root))
            || is_proc_secret(path)
    }

    /// The secrets strictly below `dir`, a path in normal form, so that a call which
    /// reads everything below `dir` can be judged by them. Under `/proc` a process's
    /// secret entries stand for every process.
    pub(crate) fn secrets_below(&self, dir: &Path) -> Vec<PathBuf> {
        let dir = self.rehome(dir);
        let dir = dir.as_ref();
        let mut below: Vec<PathBuf> = HOME_SECRETS
            .iter()
            .map(|secret| self.home.join(secret))
            .chain(SYSTEM_SECRETS.iter().map(PathBuf::from))
            .chain(self.secret_roots.iter().cloned())
            .filter(|root| root.starts_with(dir) && root != dir)
            .collect();
        below.extend(proc_secret_below(dir));
        below
    }
}

/// True for the secret entries of a process or thread directory under `/proc`.
fn is_proc_secret(path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix("/proc") else {
        return false;
    };
    let names: Vec<&std::ffi::OsStr> = rest
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let secret = |name: &std::ffi::OsStr| PROC_SECRETS.iter().any(|entry| name == *entry);
    match names.as_slice() {
        [_, entry, ..] if secret(entry) => true,
        [_, task, _, entry, ..] => *task == "task" && secret(entry),
        _ => false,
    }
}

/// A secret entry of `/proc` below `dir` that a recursive read of `dir` would reach.
fn proc_secret_below(dir: &Path) -> Option<PathBuf> {
    if dir == Path::new("/") || dir == Path::new("/proc") {
        return Some(PathBuf::from("/proc/self/environ"));
    }
    let rest = dir.strip_prefix("/proc").ok()?;
    let names: Vec<String> = rest
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    let is_process = |name: &str| {
        name == "self" || name == "thread-self" || name.bytes().all(|byte| byte.is_ascii_digit())
    };
    match names.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [process] if is_process(process) => Some(dir.join("environ")),
        [process, "task"] if is_process(process) => Some(dir.join("*").join("environ")),
        [process, "task", _] if is_process(process) => Some(dir.join("environ")),
        _ => None,
    }
}

/// True when `path` is a `.git` file or directory, or lies below one.
fn in_repository_metadata(path: &Path) -> bool {
    path.components().any(|component| component == Component::Normal(".git".as_ref()))
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
