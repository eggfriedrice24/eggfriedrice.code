//! The sandbox launcher's copy: efrd copies the installed `efr-sbx` to
//! `$R/bin/efr-sbx` (mode 0500) at start and checks its SHA-256 against the source, so
//! the program that the hidden shells run lies in efr's runtime root, which every
//! contained call masks, and an update of the installed file while efrd runs changes
//! nothing until a restart.

use std::fs;
use std::io::{self, Read as _};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// The launcher's file name, next to `efrd` and in `$R/bin`.
pub(crate) const LAUNCHER: &str = "efr-sbx";

/// The directory below efr's runtime root that holds the copy.
pub(crate) const BIN_DIR: &str = "bin";

/// The copy's mode: read and run, by its owner only.
const COPY_MODE: u32 = 0o500;

/// The installed launcher for the daemon at `exe`: next to it, as `just run` and the
/// AUR packages place it, else in `../lib/efr/` (`~/.local/lib/efr/efr-sbx` next to
/// `~/.local/bin/efrd`, `/usr/lib/efr/efr-sbx` next to `/usr/bin/efrd`).
pub(crate) fn find_source(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    [dir.join(LAUNCHER), dir.join("../lib/efr").join(LAUNCHER)]
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|found| fs::canonicalize(&found).unwrap_or(found))
}

/// Copies `source` to `copy` (mode 0500, its directory 0700). The old copy goes first;
/// the new one is written next to it and renamed into place, so the hidden shells never
/// run half a file. It blocks.
pub(crate) fn install(source: &Path, copy: &Path) -> io::Result<()> {
    let dir = copy.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let partial = dir.join(format!(".{LAUNCHER}.partial"));
    match fs::remove_file(&partial) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    fs::copy(source, &partial)?;
    fs::set_permissions(&partial, fs::Permissions::from_mode(COPY_MODE))?;
    fs::rename(&partial, copy)
}

/// The SHA-256 of the file at `path`. It blocks.
pub(crate) fn sha256(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

/// True when `copy` holds the same bytes as `source`. A file that cannot be read never
/// matches. It blocks.
pub(crate) fn matches(source: &Path, copy: &Path) -> bool {
    match (sha256(source), sha256(copy)) {
        (Ok(source), Ok(copy)) => source == copy,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
