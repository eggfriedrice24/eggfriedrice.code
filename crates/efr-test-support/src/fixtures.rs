//! Finding a crate's fixtures from the source file of a test.
//!
//! A test names its fixtures relative to the `fixtures/` directory of its own crate
//! and passes `file!()`, so the lookup never depends on the current directory:
//!
//! ```ignore
//! let path = efr_test_support::fixtures::path(file!(), "shell_marks/prompt.ndjson")?;
//! ```
//!
//! `file!()` is relative to the workspace root for a workspace member. The root is
//! found by walking up from this crate's own manifest directory until the source file
//! exists below it; the crate is the nearest directory above the source file that
//! holds a `Cargo.toml`.

use std::path::{Path, PathBuf};

use crate::TestSupportError;

/// This crate's manifest directory at build time; the workspace root is above it.
const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// The name of the fixture directory in every crate.
const FIXTURES: &str = "fixtures";

/// The directory of the crate that holds `source_file`, a path as `file!()` gives it.
pub fn crate_dir(source_file: &str) -> Result<PathBuf, TestSupportError> {
    let not_found = || TestSupportError::NoCrateForSource { source_file: source_file.into() };
    let file = resolve(Path::new(source_file)).ok_or_else(not_found)?;
    file.ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(not_found)
}

/// The `fixtures/` directory of the crate that holds `source_file`.
pub fn dir(source_file: &str) -> Result<PathBuf, TestSupportError> {
    Ok(crate_dir(source_file)?.join(FIXTURES))
}

/// The fixture `relative` in the `fixtures/` directory of the crate that holds
/// `source_file`. The file need not exist yet, so a bless step can write it.
pub fn path(source_file: &str, relative: impl AsRef<Path>) -> Result<PathBuf, TestSupportError> {
    Ok(dir(source_file)?.join(relative))
}

/// The absolute path of `source_file`, when it exists.
fn resolve(source_file: &Path) -> Option<PathBuf> {
    if source_file.is_absolute() {
        return source_file.is_file().then(|| source_file.to_path_buf());
    }
    Path::new(MANIFEST_DIR)
        .ancestors()
        .map(|ancestor| ancestor.join(source_file))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests;
