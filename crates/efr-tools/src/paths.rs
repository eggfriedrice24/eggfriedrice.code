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

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStrExt;
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

/// The real forms of the declared paths of one call
/// ([`ToolRequirements::with_real_paths`](crate::ToolRequirements::with_real_paths)).
///
/// It gives what [`reached`] gives for each path, but it looks up each directory on
/// the way once for the whole call, and a path below a directory that is missing or
/// that the file system refuses needs no look-up: the file system resolves `a/b`
/// through `a` first, so `a/b` fails where `a` fails, with the same error. The `cd`s
/// of one line declare thousands of paths below a few dirs that do not exist, and each
/// missing level cost a full `canonicalize` of its own.
pub(crate) struct RealForms<'h> {
    home: &'h Home,
    /// What the file system gives for each path that was looked up or known from its
    /// parent, by the exact bytes of the path: `a/b/` is not `a/b` when `b` is a file.
    known: HashMap<OsString, Canon>,
    /// The look-ups that reached the file system.
    lookups: usize,
}

/// What `canonicalize` gives for one path.
#[derive(Debug, Clone)]
enum Canon {
    /// The path exists, with every link resolved.
    Real(PathBuf),
    /// A part of the path does not exist.
    Missing,
    /// Any other error, such as a directory that cannot be searched or a loop of links.
    Fails,
}

/// The longest path whose look-up the look-up of its parent can stand for. A path with
/// a NUL byte, or a longer one, can fail on its own before any part of it is resolved.
const INFERRED_MAX_BYTES: usize = 4095;

impl<'h> RealForms<'h> {
    pub(crate) fn new(home: &'h Home) -> Self {
        RealForms { home, known: HashMap::new(), lookups: 0 }
    }

    /// The path that `declared`, an absolute path, reaches through a symbolic link:
    /// `None` when it reaches itself, when only the form of the home directory differs,
    /// or when the file system cannot tell (a directory that cannot be searched, a loop
    /// of links), which the shell running as the same user cannot get through either.
    /// It blocks; async callers run it in `spawn_blocking`.
    pub(crate) fn real_form(&mut self, declared: &Path) -> Option<PathBuf> {
        let real = self.reached(declared)?;
        (!same_place(self.home, declared, &real)).then_some(real)
    }

    /// The look-ups that reached the file system so far.
    #[cfg_attr(not(test), expect(dead_code, reason = "only the tests and the bench read it"))]
    pub(crate) fn lookups(&self) -> usize {
        self.lookups
    }

    /// What [`reached`] gives for `declared`, with `None` for an error and for a path
    /// of which no part exists.
    fn reached(&mut self, declared: &Path) -> Option<PathBuf> {
        let mut existing = declared;
        let mut missing = Vec::new();
        let resolved = loop {
            match self.canon(existing) {
                Canon::Real(resolved) => break resolved,
                Canon::Missing => {
                    let (Some(name), Some(parent)) = (existing.file_name(), existing.parent())
                    else {
                        return None;
                    };
                    missing.push(name);
                    existing = parent;
                }
                Canon::Fails => return None,
            }
        };
        let mut real = resolved;
        for name in missing.iter().rev() {
            real.push(name);
        }
        Some(real)
    }

    /// What `canonicalize` gives for `path`. It goes from the nearest parent that is
    /// known down to `path`, and keeps each level.
    fn canon(&mut self, path: &Path) -> Canon {
        let mut chain = Vec::new();
        let mut above = None;
        let mut at = Some(path);
        while let Some(level) = at {
            if let Some(known) = self.known.get(level.as_os_str()) {
                above = Some(known.clone());
                break;
            }
            chain.push(level);
            at = level.parent();
        }
        for level in chain.into_iter().rev() {
            let canon = match above {
                Some(Canon::Missing) if inferable(level) => Canon::Missing,
                Some(Canon::Fails) if inferable(level) => Canon::Fails,
                Some(Canon::Real(parent)) => self.below(&parent, level),
                _ => self.lookup(level),
            };
            self.known.insert(level.as_os_str().to_owned(), canon.clone());
            above = Some(canon);
        }
        above.unwrap_or(Canon::Fails)
    }

    /// What `canonicalize` gives for `path`, whose parent resolves to `parent`. When
    /// `path` is its parent and one more name, written plainly, and that name is no
    /// link, one `lstat` of the name in `parent` tells: the parent resolves as before,
    /// and a name that is no link resolves to itself. Anything else takes the full
    /// look-up.
    fn below(&mut self, parent: &Path, path: &Path) -> Canon {
        let (Some(name), Some(written)) = (path.file_name(), path.parent()) else {
            return self.lookup(path);
        };
        let bytes = path.as_os_str().as_bytes();
        let start = written.as_os_str().as_bytes();
        // NOTE: `a/b/` and `a//b` have the parent `a` too, and `a/b/` is no file.
        let joined = match start {
            b"/" => [start, name.as_bytes()].concat(),
            _ => [start, b"/", name.as_bytes()].concat(),
        };
        if bytes != joined.as_slice() || !inferable(path) {
            return self.lookup(path);
        }
        let real = parent.join(name);
        self.lookups += 1;
        match std::fs::symlink_metadata(&real) {
            Ok(meta) if !meta.file_type().is_symlink() => Canon::Real(real),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Canon::Missing,
            Ok(_) | Err(_) => self.lookup(path),
        }
    }

    /// `canonicalize` of `path`.
    fn lookup(&mut self, path: &Path) -> Canon {
        self.lookups += 1;
        match std::fs::canonicalize(path) {
            Ok(real) => Canon::Real(real),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Canon::Missing,
            Err(_) => Canon::Fails,
        }
    }
}

/// True when the look-up of `path` fails where the look-up of its parent fails: it has
/// no NUL byte, and it is short enough that no check of its length comes first.
fn inferable(path: &Path) -> bool {
    let bytes = path.as_os_str().as_bytes();
    bytes.len() <= INFERRED_MAX_BYTES && !bytes.contains(&0)
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
