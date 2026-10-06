//! Lexical path helpers: normal form, containment, depth and `~`.
//!
//! Nothing here touches the file system; [`crate::FsView`] does that.

use std::path::{Component, Path, PathBuf};

/// `path` in normal form: absolute, without `.` and with `..` applied. `None` for a
/// relative path. A `..` at the root stays at the root, as the kernel does.
pub fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    Some(out)
}

/// True when `path` is already in the normal form of [`normalize`].
pub fn is_normal(path: &Path) -> bool {
    normalize(path).is_some_and(|normal| normal.as_os_str() == path.as_os_str())
}

/// True when `path` is `root` or lies below it, compared by components.
pub fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// The number of normal components: 0 for `/`, 2 for `/home/u`.
pub fn depth(path: &Path) -> usize {
    path.components().filter(|component| matches!(component, Component::Normal(_))).count()
}

/// `path` with a leading `~` component replaced by `home`; other paths unchanged.
pub fn expand_home(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) if rest.as_os_str().is_empty() => home.to_path_buf(),
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// True when `path` is `/`, `home` or a directory above `home`: never a write root.
pub fn too_wide(path: &Path, home: &Path) -> bool {
    path.parent().is_none() || is_within(home, path)
}

#[cfg(test)]
mod tests;
