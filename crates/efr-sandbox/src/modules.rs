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
use crate::git_config::parse_config;
use crate::paths::{expand_home, normalize};

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

/// The most bytes of a `.gitmodules` that [`module_work_tree`] reads.
const MAX_GITMODULES: usize = 1024 * 1024;

/// A path that a git config names, where git reads more config or runs hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigPath {
    /// The file of an `include.path` or an `includeIf.<condition>.path`.
    Include(PathBuf),
    /// The dir of `core.hooksPath`.
    HooksPath(PathBuf),
}

/// The include files and the hooks path that the config `text` of `git_dir` names, in
/// the order of the text. An include path is relative to the git dir; a relative hooks
/// path is relative to `work_tree`, the dir where git runs the hooks, and with no known
/// work tree only an absolute one counts. A path that starts with `~` counts only with
/// a `home`.
pub(crate) fn config_paths(
    text: &str,
    git_dir: &Path,
    work_tree: Option<&Path>,
    home: Option<&Path>,
) -> Vec<ConfigPath> {
    let mut found = Vec::new();
    for (key, value) in parse_config(text) {
        let hooks = key == "core.hookspath";
        let include =
            key == "include.path" || (key.starts_with("includeif.") && key.ends_with(".path"));
        if !include && !hooks {
            continue;
        }
        let named = match home {
            Some(home) => expand_home(Path::new(&value), home),
            None if value.starts_with('~') => continue,
            None => PathBuf::from(&value),
        };
        let base = match (hooks, work_tree) {
            (false, _) => git_dir,
            (true, Some(tree)) => tree,
            (true, None) if named.is_absolute() => git_dir,
            (true, None) => continue,
        };
        let Some(path) = normalize(&base.join(named)) else { continue };
        found.push(if hooks { ConfigPath::HooksPath(path) } else { ConfigPath::Include(path) });
    }
    found
}

/// The work tree of the module git dir `module`, whose config is `text`: its
/// `core.worktree`, which git writes for each module, else the path that the
/// `.gitmodules` of its superproject gives its name. `trees` holds each git dir above
/// it with its work tree.
pub(crate) fn module_work_tree(
    module: &Path,
    text: &str,
    trees: &[(PathBuf, Option<PathBuf>)],
    fs: &dyn FsView,
) -> Result<Option<PathBuf>, SandboxError> {
    if let Some((_, value)) = parse_config(text).into_iter().find(|(key, _)| key == "core.worktree")
    {
        return Ok(normalize(&module.join(value)));
    }
    // NOTE: the deepest superproject first: `modules/a/modules/b` is the module `b` of
    // the module `a` before it is a module named `a/modules/b`. A `.gitmodules` lies
    // in a work tree that a call can write, so it decides only where efr looks for a
    // module's relative hooks path when the module's own config does not say.
    let mut parents: Vec<&(PathBuf, Option<PathBuf>)> =
        trees.iter().filter(|(git_dir, _)| module.starts_with(git_dir.join("modules"))).collect();
    parents.sort_by_key(|(git_dir, _)| std::cmp::Reverse(git_dir.as_os_str().len()));
    for (git_dir, tree) in parents {
        let Some(tree) = tree else { continue };
        let Ok(name) = module.strip_prefix(git_dir.join("modules")) else { continue };
        let gitmodules = tree.join(".gitmodules");
        match fs.lstat(&gitmodules) {
            Ok(FileKind::File) => {}
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(SandboxError::Io { path: gitmodules, source }),
        }
        let bytes = fs
            .read_file(&gitmodules, MAX_GITMODULES)
            .map_err(|source| SandboxError::Io { path: gitmodules.clone(), source })?;
        let key = format!("submodule.{}.path", name.display());
        let found = parse_config(&String::from_utf8_lossy(&bytes))
            .into_iter()
            .find(|(known, _)| *known == key);
        if let Some((_, path)) = found {
            return Ok(normalize(&tree.join(path)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
