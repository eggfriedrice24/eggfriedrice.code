//! Paths in tool calls: resolved lexically for the permission engine, then checked
//! against the file system before the tool touches them.
//!
//! The engine classifies a path as written, without the file system, so a call that
//! named `~/scratch/notes` could reach `~/.ssh` through a symbolic link. A file tool
//! therefore works only on a path whose real form is the one the engine judged; for
//! any other it fails and names the real path, and a second call with that path is
//! judged on its own. A shell command cannot be held to that, because `/bin`,
//! `/etc/resolv.conf` and many more are links, so
//! [`ToolRequirements::with_real_paths`](crate::ToolRequirements::with_real_paths)
//! declares the real form beside the written one and the engine judges both.

use std::io;
use std::path::{Component, Path, PathBuf};

use efr_scope::Home;

use crate::{ToolContext, ToolError};

/// The absolute, lexically normal form of `raw`: `~` and `~/...` under the home
/// directory, a relative path under the context's working directory, `.` and `..`
/// folded without looking at the file system.
pub(crate) fn resolve(ctx: &ToolContext, raw: &str) -> Result<PathBuf, ToolError> {
    if raw.is_empty() {
        return Err(ToolError::EmptyPath);
    }
    let path = if raw == "~" {
        ctx.home.path().to_path_buf()
    } else if let Some(below) = raw.strip_prefix("~/") {
        ctx.home.path().join(below)
    } else {
        ctx.cwd.join(raw)
    };
    Ok(normalize(&path))
}

/// `.` dropped, `..` removing the component before it (never above `/`), repeated
/// separators collapsed.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normal.push("/"),
            Component::Normal(name) => normal.push(name),
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir | Component::Prefix(_) => {}
        }
    }
    normal
}

/// Checks that `declared` reaches the file system as written: the part of it that
/// exists, with symbolic links resolved, plus the part that does not exist yet, must
/// be `declared` again. A home reached through a link (`/home` to `/var/home`) is the
/// one exception, because the engine knows both forms of it. It blocks; async callers
/// run it in `spawn_blocking`.
pub(crate) fn check_real(home: &Home, declared: &Path) -> Result<(), ToolError> {
    match reached(declared) {
        Ok(Some(real)) if !same_place(home, declared, &real) => {
            Err(ToolError::ThroughSymlink { path: declared.to_path_buf(), real })
        }
        Ok(_) => Ok(()),
        Err(source) => Err(ToolError::Read { path: declared.to_path_buf(), source }),
    }
}

/// The path that `declared`, an absolute path, reaches through a symbolic link: `None`
/// when it reaches itself, when only the form of the home directory differs, or when
/// the file system cannot tell (a directory that cannot be searched, a loop of
/// links), which the shell running as the same user cannot get through either. It
/// blocks; async callers run it in `spawn_blocking`.
pub(crate) fn real_form(home: &Home, declared: &Path) -> Option<PathBuf> {
    match reached(declared) {
        Ok(Some(real)) if !same_place(home, declared, &real) => Some(real),
        Ok(_) | Err(_) => None,
    }
}

/// What `declared` reaches: the part of it that exists, with symbolic links resolved,
/// plus the part that does not exist yet. `Ok(None)` when no part of it exists.
fn reached(declared: &Path) -> io::Result<Option<PathBuf>> {
    let mut existing = declared;
    let mut missing = Vec::new();
    let resolved = loop {
        match std::fs::canonicalize(existing) {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(name), Some(parent)) = (existing.file_name(), existing.parent()) else {
                    return Ok(None);
                };
                missing.push(name);
                existing = parent;
            }
            Err(error) => return Err(error),
        }
    };
    let mut real = resolved;
    for name in missing.iter().rev() {
        real.push(name);
    }
    Ok(Some(real))
}

/// True when `real` is `declared`, or `declared` with the home directory in its
/// resolved form.
fn same_place(home: &Home, declared: &Path, real: &Path) -> bool {
    real == declared
        || declared
            .strip_prefix(home.path())
            .is_ok_and(|below_home| home.canonical().join(below_home) == real)
}

#[cfg(test)]
mod tests;
