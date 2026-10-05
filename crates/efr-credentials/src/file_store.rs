//! The default store: one 0600 JSON file per record in a 0700 directory.

use std::fs::{self, DirBuilder, File, Metadata};
use std::io::{self, Read as _};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use efr_stdx::paths::Dirs;
use zeroize::Zeroizing;

use crate::{CredentialId, CredentialRecord, CredentialsError, SecretStore};

const SUFFIX: &str = ".json";
const DIR_MODE: u32 = 0o700;
/// Permission bits that let a user other than the owner reach a path.
const GROUP_OTHER_BITS: u32 = 0o077;
/// No record this crate writes comes near this size, so a larger file is not a record
/// and is not read into memory.
const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// Keeps each record as `<dir>/<id>.json`.
///
/// The directory is created with mode 0700 on the first save, and every file is
/// written through [`efr_stdx::fs::write_atomic`], which gives it mode 0600 and makes
/// the replacement atomic. A file or directory that other users can reach, or a
/// symbolic link in place of either, is refused rather than repaired: the store
/// cannot tell whether the secret has already been read.
///
/// The methods block; see [`SecretStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// A store whose records live directly in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        FileStore { dir: dir.into() }
    }

    /// The store at its standard place, `<data root>/secrets`, which is
    /// `$XDG_DATA_HOME/efr/secrets` unless `EFR_DATA_DIR` moves the data root.
    pub fn in_data_dir(dirs: &Dirs) -> Self {
        FileStore::new(dirs.secrets_dir())
    }

    /// The directory that holds the record files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file that holds the record under `id`.
    pub fn path_of(&self, id: &CredentialId) -> PathBuf {
        self.dir.join(format!("{id}{SUFFIX}"))
    }

    /// The ids that have a record file, in order. Files whose names are not
    /// `<valid id>.json`, such as the hidden temporary files of an interrupted write,
    /// are skipped.
    pub fn list(&self) -> Result<Vec<CredentialId>, CredentialsError> {
        if !self.private_dir_exists()? {
            return Ok(Vec::new());
        }
        let read_error = |source| CredentialsError::Read { path: self.dir.clone(), source };
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.dir).map_err(read_error)? {
            let entry = entry.map_err(read_error)?;
            if !entry.file_type().map_err(read_error)?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(SUFFIX)) else {
                continue;
            };
            if let Ok(id) = CredentialId::new(stem) {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Creates the directory when it is missing and checks it.
    fn ensure_dir(&self) -> Result<(), CredentialsError> {
        // `recursive` gives every directory it creates mode 0700, the data root
        // included, which is right for a directory that only efr reads.
        DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(&self.dir)
            .map_err(|source| CredentialsError::CreateDir { path: self.dir.clone(), source })?;
        if self.private_dir_exists()? {
            Ok(())
        } else {
            // Removed between the create and the check: report it as the create
            // failing, which is what the caller experiences.
            Err(CredentialsError::CreateDir {
                path: self.dir.clone(),
                source: io::Error::from(io::ErrorKind::NotFound),
            })
        }
    }

    /// `false` when the directory does not exist; an error when it is not a private
    /// directory.
    fn private_dir_exists(&self) -> Result<bool, CredentialsError> {
        match fs::symlink_metadata(&self.dir) {
            Ok(meta) => check_private(&self.dir, &meta, Expected::Directory).map(|()| true),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(CredentialsError::Read { path: self.dir.clone(), source }),
        }
    }
}

impl SecretStore for FileStore {
    fn load(&self, id: &CredentialId) -> Result<Option<CredentialRecord>, CredentialsError> {
        if !self.private_dir_exists()? {
            return Ok(None);
        }
        let path = self.path_of(id);
        let read_error = |source| CredentialsError::Read { path: path.clone(), source };
        // `symlink_metadata` refuses a link before `open` would follow it. A writer
        // that could swap the file in between can already write the 0700 directory.
        match fs::symlink_metadata(&path) {
            Ok(meta) => check_private(&path, &meta, Expected::File)?,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(read_error(source)),
        }
        let file = File::open(&path).map_err(read_error)?;
        let meta = file.metadata().map_err(read_error)?;
        check_private(&path, &meta, Expected::File)?;
        if meta.len() > MAX_RECORD_BYTES {
            return Err(CredentialsError::TooLarge { path, limit: MAX_RECORD_BYTES });
        }
        let bytes = read_capped(file, meta.len()).map_err(read_error)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(CredentialsError::TooLarge { path, limit: MAX_RECORD_BYTES });
        }
        CredentialRecord::decode(id, &bytes).map(Some)
    }

    fn save(&self, id: &CredentialId, record: &CredentialRecord) -> Result<(), CredentialsError> {
        self.ensure_dir()?;
        let bytes = record.encode(id)?;
        let path = self.path_of(id);
        efr_stdx::fs::write_atomic(&path, &bytes)
            .map_err(|source| CredentialsError::Write { path, source })
    }

    fn delete(&self, id: &CredentialId) -> Result<bool, CredentialsError> {
        let path = self.path_of(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(CredentialsError::Delete { path, source }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Expected {
    File,
    Directory,
}

/// Refuses anything but a regular file or directory (as expected) that only its owner
/// can reach. `meta` must come from `symlink_metadata` or an open file, so a symbolic
/// link shows up as itself.
fn check_private(path: &Path, meta: &Metadata, expected: Expected) -> Result<(), CredentialsError> {
    let (kind_ok, expected_name) = match expected {
        Expected::File => (meta.file_type().is_file(), "file"),
        Expected::Directory => (meta.file_type().is_dir(), "directory"),
    };
    if !kind_ok {
        return Err(CredentialsError::UnexpectedFileType {
            path: path.to_path_buf(),
            expected: expected_name,
        });
    }
    let mode = meta.permissions().mode() & 0o7777;
    if mode & GROUP_OTHER_BITS != 0 {
        return Err(CredentialsError::InsecurePermissions { path: path.to_path_buf(), mode });
    }
    Ok(())
}

/// Reads at most one byte more than the limit, into a buffer sized up front so that it
/// never reallocates and leaves an unzeroed copy behind.
fn read_capped(file: File, len_hint: u64) -> io::Result<Zeroizing<Vec<u8>>> {
    let capacity = usize::try_from(len_hint.min(MAX_RECORD_BYTES) + 1).unwrap_or(0);
    let mut bytes = Zeroizing::new(Vec::with_capacity(capacity));
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
