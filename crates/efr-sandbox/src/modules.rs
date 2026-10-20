//! The git dirs of the submodules below a git dir's `modules/`, at any depth.
//!
//! git keeps the git dir of the submodule `vendor/foo` at `.git/modules/vendor/foo`,
//! and that of a submodule of a submodule at `.git/modules/a/modules/b`. The plan pins
//! each one and the surface guard records each one, so the walk finds every one, not
//! only the direct children of `modules/`. efrd's check of the git code that an
//! outside git line runs uses the same walk.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::SandboxError;
use crate::fs_view::{FileKind, FsView};

/// The deepest dir below `modules/` that the walk lists: `modules/a/modules/b` has the
/// depth 3. A deeper dir stops the walk with [`SandboxError::ModulesTooDeep`].
pub const MAX_MODULE_DEPTH: usize = 8;

/// The dirs of a git dir that the walk never enters: they hold git's objects, its refs
/// and the large files of git-lfs, never a git dir that git uses for a submodule, and
/// they are large.
const SKIP: &[&str] = &["objects", "refs", "logs", "lfs"];

/// What the walk below one `modules/` found.
#[derive(Debug, Default)]
pub(crate) struct ModuleWalk {
    /// Each dir that holds `HEAD` or `config`, sorted.
    pub(crate) git_dirs: Vec<PathBuf>,
    /// Each symbolic link that the walk found where it looks for dirs, sorted.
    pub(crate) links: Vec<PathBuf>,
    /// The first dir deeper than [`MAX_MODULE_DEPTH`] that the walk did not list.
    pub(crate) too_deep: Option<PathBuf>,
    /// The first dir that the walk could not list, with the error.
    pub(crate) unreadable: Option<(PathBuf, io::Error)>,
}

/// Every module git dir below `git_dir/modules`, at any depth, sorted: each dir that
/// holds `HEAD` or `config`. The walk does not enter the `objects`, `refs`, `logs` and
/// `lfs` of a git dir. No `modules/` gives an empty list.
///
/// A symbolic link where the walk looks for dirs is [`SandboxError::SymlinkedPin`]: git
/// would follow it to a git dir that efr cannot pin. A dir deeper than
/// [`MAX_MODULE_DEPTH`] is [`SandboxError::ModulesTooDeep`], and a dir that cannot be
/// listed is [`SandboxError::Io`]; then the caller knows that the list is not complete.
pub fn module_git_dirs(git_dir: &Path, fs: &dyn FsView) -> Result<Vec<PathBuf>, SandboxError> {
    let walk = walk(git_dir, fs);
    if let Some(path) = walk.links.into_iter().next() {
        return Err(SandboxError::SymlinkedPin { path });
    }
    if let Some((path, source)) = walk.unreadable {
        return Err(SandboxError::Io { path, source });
    }
    if let Some(at) = walk.too_deep {
        return Err(SandboxError::ModulesTooDeep { at });
    }
    Ok(walk.git_dirs)
}

/// The walk of [`module_git_dirs`], which goes on past a link, a dir that is too deep
/// and a dir that it cannot list, and says what it found.
pub(crate) fn walk(git_dir: &Path, fs: &dyn FsView) -> ModuleWalk {
    let mut found = ModuleWalk::default();
    let modules = git_dir.join("modules");
    match fs.lstat(&modules) {
        Ok(FileKind::Dir) => {}
        Ok(FileKind::Symlink) => {
            found.links.push(modules);
            return found;
        }
        Ok(_) => return found,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return found,
        Err(error) => {
            found.unreadable = Some((modules, error));
            return found;
        }
    }
    let mut todo: Vec<(PathBuf, usize)> = vec![(modules, 0)];
    while let Some((dir, depth)) = todo.pop() {
        let mut entries = match fs.read_dir_kinds(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                if found.unreadable.is_none() {
                    found.unreadable = Some((dir, error));
                }
                continue;
            }
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let git = depth > 0 && entries.iter().any(|(name, _)| name == "HEAD" || name == "config");
        if git {
            found.git_dirs.push(dir.clone());
        }
        for (name, kind) in entries {
            if git && SKIP.iter().any(|skip| name == *skip) {
                continue;
            }
            let child = dir.join(&name);
            match kind_of(fs, &child, kind) {
                Some(FileKind::Dir) if depth >= MAX_MODULE_DEPTH => {
                    if found.too_deep.is_none() {
                        found.too_deep = Some(child);
                    }
                }
                Some(FileKind::Dir) => todo.push((child, depth + 1)),
                Some(FileKind::Symlink) if !git || is_module_name(&name) => {
                    found.links.push(child);
                }
                _ => {}
            }
        }
    }
    found.git_dirs.sort();
    found.links.sort();
    found
}

/// True for the one name in a git dir where git looks for module git dirs; a link with
/// another name in a git dir, such as one in `hooks`, is the guard's business, not the
/// walk's.
fn is_module_name(name: &OsString) -> bool {
    name == "modules"
}

fn kind_of(fs: &dyn FsView, path: &Path, kind: Option<FileKind>) -> Option<FileKind> {
    kind.or_else(|| fs.lstat(path).ok())
}

#[cfg(test)]
mod tests;
