//! [`FsView`]: the only way this crate reads the file system, so every rule runs
//! against a fake in tests and against `openat2` in `efr-sbx` and `efrd`.

use std::ffi::OsString;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use crate::SandboxError;
use crate::paths::normalize;

/// What `lstat` found at a path. A link is not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link.
    Symlink,
    /// A socket, a device, a fifo.
    Other,
}

/// Read access to the file system. The implementation in `efr-sbx` opens every path
/// with `openat2(RESOLVE_NO_SYMLINKS)`, so a path that a sandboxed call swapped for a
/// link after the check cannot change what is bound.
pub trait FsView {
    /// The kind of `path`, without following a link at its end. A missing path is
    /// [`io::ErrorKind::NotFound`].
    fn lstat(&self, path: &Path) -> io::Result<FileKind>;

    /// The target of the symbolic link `path`, as the link holds it.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;

    /// The names in the directory `path`, without `.` and `..`, in any order.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<OsString>>;

    /// The content of the file `path`, at most `limit` bytes; a longer file is an
    /// error of kind [`io::ErrorKind::FileTooLarge`].
    fn read_file(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>>;

    /// An `O_PATH` descriptor of `path`, opened with no symbolic link anywhere on the
    /// way: the source of one `--bind-fd` or `--ro-bind-fd`.
    fn open_no_symlinks(&self, path: &Path) -> io::Result<OwnedFd>;

    /// A descriptor that reads as empty, such as `/dev/null` opened for reading: the
    /// data of one `--ro-bind-data`, which masks a file.
    fn open_empty(&self) -> io::Result<OwnedFd>;
}

/// The most symbolic links one resolution follows, as the kernel's `MAXSYMLINKS`.
pub const MAX_LINKS: usize = 40;

/// Where a path leads once every symbolic link on the way is followed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Resolved {
    /// The path with every link followed, in normal form. When a component is missing,
    /// the rest is appended as written.
    pub path: PathBuf,
    /// True when `path` exists.
    pub exists: bool,
    /// What `path` is, when it exists.
    pub kind: Option<FileKind>,
    /// Every symbolic link that the resolution went through, where it lies.
    pub links: Vec<PathBuf>,
}

/// Follows every symbolic link of `path`, an absolute path, like `realpath -m`.
pub fn resolve(fs: &dyn FsView, path: &Path) -> Result<Resolved, SandboxError> {
    let Some(start) = normalize(path) else {
        return Err(SandboxError::SpecPath { field: "path", path: path.to_path_buf() });
    };
    let mut done = PathBuf::from("/");
    let mut todo: Vec<OsString> = start.iter().skip(1).map(|part| part.to_os_string()).collect();
    todo.reverse();
    let mut links = Vec::new();
    let mut kind = Some(FileKind::Dir);
    while let Some(part) = todo.pop() {
        if part == ".." {
            done.pop();
            kind = Some(FileKind::Dir);
            continue;
        }
        if part == "." {
            continue;
        }
        let next = done.join(&part);
        match fs.lstat(&next) {
            Ok(FileKind::Symlink) => {
                if links.len() >= MAX_LINKS {
                    return Err(SandboxError::LinkLoop { path: path.to_path_buf() });
                }
                let target = fs
                    .read_link(&next)
                    .map_err(|source| SandboxError::Io { path: next.clone(), source })?;
                links.push(next);
                if target.is_absolute() {
                    done = PathBuf::from("/");
                }
                let mut parts: Vec<OsString> = target
                    .iter()
                    .filter(|part| *part != "/")
                    .map(|part| part.to_os_string())
                    .collect();
                parts.reverse();
                todo.extend(parts);
            }
            Ok(found) => {
                done = next;
                kind = Some(found);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut rest = next;
                while let Some(part) = todo.pop() {
                    rest.push(part);
                }
                let path = normalize(&rest).unwrap_or(rest);
                return Ok(Resolved { path, exists: false, kind: None, links });
            }
            Err(source) => return Err(SandboxError::Io { path: next, source }),
        }
    }
    Ok(Resolved { path: done, exists: true, kind, links })
}

#[cfg(test)]
mod tests;
