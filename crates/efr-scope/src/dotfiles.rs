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
/// `~/.local/share`. The disk is read on tokio's blocking pool under the git timeout.
/// Fails when `home` cannot be listed, when git cannot run, or when git or a look at
/// the disk does not finish in time.
pub async fn detect_dotfiles(
    home: &Home,
    data_home: Option<&Path>,
    git: &Git,
) -> Result<Vec<Dotfiles>, ScopeError> {
    let scan = {
        let (owned, data_home) = (home.clone(), data_home.map(Path::to_path_buf));
        git.probe(home.path(), move || scan(&owned, data_home.as_deref())).await??
    };
    let mut found = Vec::new();
    if scan.dot_git && is_home_work_tree(home, git).await? {
        found.push(Dotfiles::HomeWorkTree { git_dir: home.path().join(".git") });
    }
    found.extend(scan.yadm.into_iter().map(|git_dir| Dotfiles::Yadm { git_dir }));
    for git_dir in scan.bare_candidates {
        if worktree_is_home(&git_dir, home, git).await? {
            found.push(Dotfiles::BareRepo { git_dir });
        }
    }
    Ok(found)
}

/// What [`detect_dotfiles`] needs from the disk, read in one blocking call.
#[derive(Debug)]
struct Scan {
    /// Something is at `~/.git`.
    dot_git: bool,
    /// The yadm repositories that exist.
    yadm: Vec<PathBuf>,
    /// The dot directories of `$HOME` shaped like a git directory, by name.
    bare_candidates: Vec<PathBuf>,
}

fn scan(home: &Home, data_home: Option<&Path>) -> Result<Scan, ScopeError> {
    let dot_git = home.path().join(".git").symlink_metadata().is_ok();
    let default_data_home = home.path().join(".local/share");
    let mut yadm = vec![
        data_home.unwrap_or(&default_data_home).join("yadm/repo.git"),
        home.path().join(".config/yadm/repo.git"),
        home.path().join(".yadm/repo.git"),
    ];
    yadm.dedup();
    yadm.retain(|git_dir| looks_like_git_dir(git_dir));
    let mut bare_candidates = dot_dirs(home)?;
    bare_candidates.retain(|git_dir| looks_like_git_dir(git_dir));
    Ok(Scan { dot_git, yadm, bare_candidates })
}

async fn is_home_work_tree(home: &Home, git: &Git) -> Result<bool, ScopeError> {
    match git.run(home.path(), home, ["rev-parse", "--show-toplevel"]).await? {
        Some(root) => is_home(PathBuf::from(OsString::from_vec(root)), home, git).await,
        None => Ok(false),
    }
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
    is_home(worktree, home, git).await
}

/// True when `path` names the home directory, lexically or after resolving links. Links
/// are resolved on the blocking pool, and only when the lexical form does not match.
async fn is_home(path: PathBuf, home: &Home, git: &Git) -> Result<bool, ScopeError> {
    let names_home = |candidate: &Path| candidate == home.path() || candidate == home.canonical();
    if normalize(&path).is_some_and(|normal| names_home(&normal)) {
        return Ok(true);
    }
    let target = path.clone();
    let canonical = git.probe(&path, move || std::fs::canonicalize(target).ok()).await?;
    Ok(canonical.is_some_and(|canonical| names_home(&canonical)))
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
