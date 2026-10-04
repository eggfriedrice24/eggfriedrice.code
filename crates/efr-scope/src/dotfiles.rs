//! The dotfiles layouts that make `$HOME` a git work tree.
//!
//! Plain git discovery sees only the first of these, and only from `$HOME` itself:
//!
//! - a `~/.git` directory or file: `$HOME` is an ordinary work tree;
//! - yadm: a repository at `$XDG_DATA_HOME/yadm/repo.git`, or at the older
//!   `~/.config/yadm/repo.git` or `~/.yadm/repo.git`;
//! - a bare repository among the dot directories of `$HOME` whose `core.worktree` is
//!   `$HOME`, used through an alias such as `git --git-dir=$HOME/.dotfiles`.
//!
//! A bare repository used only through `--work-tree=$HOME` on the command line leaves
//! nothing on disk that says so, and is not detected. The daemon detects the layouts
//! every turn, because they come and go, and records them in machine memory.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

use crate::home::normalize;
use crate::{Git, Home, ScopeError};

/// A dotfiles layout found in `$HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Dotfiles {
    /// `$HOME` is a work tree with its own `~/.git`.
    HomeWorkTree {
        /// `~/.git`.
        git_dir: PathBuf,
    },
    /// A yadm repository.
    Yadm {
        /// The repository, such as `~/.local/share/yadm/repo.git`.
        git_dir: PathBuf,
    },
    /// A bare repository whose `core.worktree` is `$HOME`.
    BareRepo {
        /// The repository, such as `~/.dotfiles`.
        git_dir: PathBuf,
    },
}

impl Dotfiles {
    /// The layout's git directory.
    pub fn git_dir(&self) -> &Path {
        match self {
            Dotfiles::HomeWorkTree { git_dir }
            | Dotfiles::Yadm { git_dir }
            | Dotfiles::BareRepo { git_dir } => git_dir,
        }
    }
}

/// Every dotfiles layout in `home`: a home work tree first, then yadm repositories,
/// then bare repositories by name.
///
/// `data_home` is `$XDG_DATA_HOME` when the caller knows it; `None` means
/// `~/.local/share`. Fails when `home` cannot be listed or git cannot run.
pub async fn detect_dotfiles(
    home: &Home,
    data_home: Option<&Path>,
    git: &Git,
) -> Result<Vec<Dotfiles>, ScopeError> {
    let mut found = Vec::new();

    let dot_git = home.path().join(".git");
    if dot_git.symlink_metadata().is_ok() && is_home_work_tree(home, git).await? {
        found.push(Dotfiles::HomeWorkTree { git_dir: dot_git });
    }

    let default_data_home = home.path().join(".local/share");
    let mut yadm = vec![
        data_home.unwrap_or(&default_data_home).join("yadm/repo.git"),
        home.path().join(".config/yadm/repo.git"),
        home.path().join(".yadm/repo.git"),
    ];
    yadm.dedup();
    for git_dir in yadm {
        if looks_like_git_dir(&git_dir) {
            found.push(Dotfiles::Yadm { git_dir });
        }
    }

    for git_dir in dot_dirs(home)? {
        if looks_like_git_dir(&git_dir) && worktree_is_home(&git_dir, home, git).await? {
            found.push(Dotfiles::BareRepo { git_dir });
        }
    }
    Ok(found)
}

async fn is_home_work_tree(home: &Home, git: &Git) -> Result<bool, ScopeError> {
    let root = git.run(home.path(), home, ["rev-parse", "--show-toplevel"]).await?;
    Ok(root.is_some_and(|root| is_home(&PathBuf::from(OsString::from_vec(root)), home)))
}

/// Whether the bare repository `git_dir` names `$HOME` as its work tree.
async fn worktree_is_home(git_dir: &Path, home: &Home, git: &Git) -> Result<bool, ScopeError> {
    let args = [
        OsStr::new("--git-dir"),
        git_dir.as_os_str(),
        OsStr::new("config"),
        OsStr::new("--local"),
        OsStr::new("--get"),
        OsStr::new("core.worktree"),
    ];
    let Some(value) = git.run(home.path(), home, args).await? else {
        return Ok(false);
    };
    let value = PathBuf::from(OsString::from_vec(value));
    // git reads a relative core.worktree against the git directory, and a leading `~`
    // only in some versions; both spellings name $HOME in practice.
    let worktree = match value.strip_prefix("~") {
        Ok(rest) => home.path().join(rest),
        Err(_) => git_dir.join(value),
    };
    Ok(is_home(&worktree, home))
}

/// True when `path` names the home directory, lexically or after resolving links.
fn is_home(path: &Path, home: &Home) -> bool {
    let normal = normalize(path);
    let canonical = std::fs::canonicalize(path).ok();
    [normal, canonical]
        .into_iter()
        .flatten()
        .any(|path| path == home.path() || path == home.canonical())
}

/// The directories directly in `$HOME` whose names start with a dot, except `.git`,
/// sorted by name.
fn dot_dirs(home: &Home) -> Result<Vec<PathBuf>, ScopeError> {
    let read_error = |source| ScopeError::ReadDir { path: home.path().to_path_buf(), source };
    let mut dirs = Vec::new();
    for entry in std::fs::read_dir(home.path()).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        let name = entry.file_name();
        if name.as_encoded_bytes().starts_with(b".") && name != ".git" && entry.path().is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// The shape of a git directory: a `HEAD` file and `objects` and `refs` directories.
fn looks_like_git_dir(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

#[cfg(test)]
mod tests;
