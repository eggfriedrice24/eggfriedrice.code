//! The dotfile link scan (efr's auto spec, section 5.4): `~/.zshrc ->
//! ~/dotfiles/zsh/.zshrc` with `~/dotfiles` registered as a project would make the
//! user's startup file writable in a contained call. efrd looks at the dot entries of
//! the home directory, the entries of `~/.config` and of `~/.local/bin`, follows each
//! symbolic link, and keeps the targets that lie in a write root: they become floors,
//! read-only even inside the project.

use std::fs;
use std::path::{Path, PathBuf};

use efr_sandbox::is_within;

/// The directories whose entries the scan looks at, below the home directory; `true`
/// takes only the names that start with a dot.
const SCANNED: &[(&str, bool)] = &[("", true), (".config", false), (".local/bin", false)];

/// The most entries the scan reads per directory, so a huge directory cannot stall a
/// call.
const MAX_ENTRIES: usize = 4096;

/// The real targets of the links in the scanned directories of `home` that lie at or
/// below one of `roots`. It blocks.
pub(crate) fn link_targets(home: &Path, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    if roots.is_empty() {
        return targets;
    }
    for (relative, dot_only) in SCANNED {
        let dir = if relative.is_empty() { home.to_path_buf() } else { home.join(relative) };
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten().take(MAX_ENTRIES) {
            let name = entry.file_name();
            if *dot_only && !name.as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if !entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                continue;
            }
            let Ok(target) = fs::canonicalize(entry.path()) else { continue };
            if roots.iter().any(|root| is_within(&target, root)) && !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    targets
}

#[cfg(test)]
mod tests;
