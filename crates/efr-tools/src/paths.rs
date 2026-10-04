//! Paths in tool calls: resolved lexically for the permission engine, then checked
//! against the file system before the tool touches them.
//!
//! The engine classifies a path as written, without the file system, so a call that
//! named `~/scratch/notes` could reach `~/.ssh` through a symbolic link. A file tool
//! therefore works only on a path whose real form is the one the engine judged; for
//! any other it fails and names the real path, and a second call with that path is
//! judged on its own.

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
    let mut existing = declared;
    let mut missing = Vec::new();
    let resolved = loop {
        match std::fs::canonicalize(existing) {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(name), Some(parent)) = (existing.file_name(), existing.parent()) else {
                    return Ok(());
                };
                missing.push(name);
                existing = parent;
            }
            Err(source) => return Err(ToolError::Read { path: declared.to_path_buf(), source }),
        }
    };
    let mut real = resolved;
    for name in missing.iter().rev() {
        real.push(name);
    }
    if real == declared {
        return Ok(());
    }
    if let Ok(below_home) = declared.strip_prefix(home.path())
        && home.canonical().join(below_home) == real
    {
        return Ok(());
    }
    Err(ToolError::ThroughSymlink { path: declared.to_path_buf(), real })
}

#[cfg(test)]
mod tests;
