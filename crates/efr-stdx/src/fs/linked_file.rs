//! A file read for a change and written back only when nobody changed it meanwhile.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::write_atomic;
use crate::StdxError;

/// A file that efr changes for the user, such as `config.toml` or the project registry,
/// read whole and written back whole.
///
/// When the file is a symbolic link (into a dotfiles repository, say), the file behind
/// it is read and written, and the link stays. A link to nothing is refused, because a
/// new file would appear somewhere the user may not expect. A missing file reads as
/// `None` and a write creates it, with its directory.
///
/// The functions block. Async code calls them in `spawn_blocking`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedFile {
    /// The file as named, which may be a symbolic link.
    path: PathBuf,
    /// The file that is read and written: the end of the link, or `path` itself.
    target: PathBuf,
    /// The contents as read; `None` when the file does not exist.
    text: Option<String>,
}

impl LinkedFile {
    /// Reads the file at `path`, through a symbolic link. Fails with
    /// [`StdxError::DanglingLink`] for a link to nothing and [`StdxError::ReadFile`]
    /// when the file or the link cannot be read.
    pub fn open(path: &Path) -> Result<LinkedFile, StdxError> {
        let read_error = |source| StdxError::ReadFile { path: path.to_path_buf(), source };
        let target = match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
                Ok(target) => target,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    let target = fs::read_link(path).map_err(read_error)?;
                    return Err(StdxError::DanglingLink { path: path.to_path_buf(), target });
                }
                Err(source) => return Err(read_error(source)),
            },
            Ok(_) => path.to_path_buf(),
            Err(source) if source.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
            Err(source) => return Err(read_error(source)),
        };
        let text = read(&target)?;
        Ok(LinkedFile { path: path.to_path_buf(), target, text })
    }

    /// The file as named.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file that a write replaces: the end of the symbolic link, or the file itself.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// The contents as read; `None` when the file did not exist.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Replaces the file with `contents` atomically (mode 0600, as
    /// [`write_atomic`](super::write_atomic) writes), creating its directory when the
    /// file did not exist. Fails with [`StdxError::FileChanged`] and writes nothing when
    /// the file changed since [`open`](Self::open), or when a file or a link appeared
    /// where none was, so the caller reads it again.
    pub fn write_if_unchanged(&self, contents: &[u8]) -> Result<(), StdxError> {
        let changed = || StdxError::FileChanged { path: self.target.clone() };
        if read(&self.target)? != self.text {
            return Err(changed());
        }
        if self.text.is_none() {
            // A link or a file that appeared since the read is somebody else's change.
            match fs::symlink_metadata(&self.path) {
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(changed()),
            }
            if let Some(dir) = self.target.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                fs::create_dir_all(dir)
                    .map_err(|source| StdxError::CreateDir { path: dir.to_path_buf(), source })?;
            }
        }
        write_atomic(&self.target, contents)
    }
}

/// The contents of `path`; `None` when it does not exist.
fn read(path: &Path) -> Result<Option<String>, StdxError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StdxError::ReadFile { path: path.to_path_buf(), source }),
    }
}

#[cfg(test)]
mod tests;
