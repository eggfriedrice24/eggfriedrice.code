//! The hosts that a call with a proxy plan may not reach because a file that the
//! sandbox must read holds a credential for them (DP25).
//!
//! The sandbox cannot hide one line of a file, so efrd denies those hosts for the call
//! instead (`CallPolicy::deny`), and the call's output says why:
//!
//! - a line scan of the registry files of each project root of the call and of the
//!   call's dir (`.npmrc`, `.yarnrc.yml`, `.yarnrc`, `bunfig.toml`, `.cargo/config.toml`,
//!   `.cargo/config`; at most 64 KiB each), through the pure scanners of
//!   `efr-permissions`;
//! - one hardened `git config` run per repository that holds one of those dirs, and one
//!   for a dir in no repository, which also reads the global, XDG and include files: a
//!   remote or `insteadOf` base with user info, an `http.<url>.extraHeader`, an
//!   unscoped `http.extraHeader` (every remote and `@github-read`), and a shell or
//!   file-store credential helper (every remote). The call's own dir is always one of
//!   them, so the user's global files count also outside a project, and a project at a
//!   subdir of a repository counts the repository's config.

use std::path::{Path, PathBuf};

use efr_permissions::Preset;
use efr_permissions::network::tokens::{TokenFile, git_tokens, token_hosts};
use efr_scope::{Git, Home};

use crate::sandbox::facts::remotes;

/// The hosts that a call may not reach, and the notes that say why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Denied {
    /// The hosts, sorted, each once.
    pub(crate) hosts: Vec<String>,
    /// One note per file or repository that holds a credential.
    pub(crate) notes: Vec<String>,
}

impl Denied {
    fn add(&mut self, hosts: Vec<String>, source: &str) {
        if hosts.is_empty() {
            return;
        }
        self.notes.push(format!(
            "[efr: efr blocks {} for this command: {source} holds a credential for it. Move \
             the credential to a file that the sandbox hides, such as ~/.npmrc.]",
            hosts.join(", ")
        ));
        self.hosts.extend(hosts);
        self.hosts.sort();
        self.hosts.dedup();
    }
}

/// The hosts that the registry files of `dirs` and the git config of each of `dirs`
/// hold a credential for. `dirs` holds the call's project roots and its own dir.
pub(crate) async fn denied(git: &Git, home: &Home, dirs: &[PathBuf]) -> Denied {
    let mut denied = Denied::default();
    let owned = dirs.to_vec();
    let scanned = tokio::task::spawn_blocking(move || scan_files(&owned)).await.unwrap_or_default();
    for (file, hosts) in scanned {
        denied.add(hosts, &file.display().to_string());
    }
    // NOTE: one run per work tree, and one for all dirs in no work tree: there git reads
    // only the system, global and XDG files, which are the same for each of them.
    let mut seen: Vec<Option<PathBuf>> = Vec::new();
    for dir in dirs {
        let owned = dir.clone();
        let top =
            tokio::task::spawn_blocking(move || remotes::work_tree(&owned)).await.ok().flatten();
        if seen.contains(&top) {
            continue;
        }
        seen.push(top.clone());
        let Some(entries) = remotes::config_entries(git, home, dir).await else { continue };
        let hosts = git_hosts(&entries);
        let source = match &top {
            Some(top) => format!("the git config of {}", top.display()),
            None => "your git config".to_owned(),
        };
        denied.add(hosts, &source);
    }
    denied
}

/// The hosts of each registry file of `dirs` that holds a token. It blocks.
fn scan_files(dirs: &[PathBuf]) -> Vec<(PathBuf, Vec<String>)> {
    let mut found = Vec::new();
    let mut seen: Vec<&Path> = Vec::new();
    for dir in dirs {
        if seen.contains(&dir.as_path()) {
            continue;
        }
        seen.push(dir);
        for (name, kind) in TokenFile::FILES {
            let path = dir.join(name);
            let Some(bytes) = read_head(&path) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            let hosts = token_hosts(kind, &text);
            if !hosts.is_empty() {
                found.push((path, hosts));
            }
        }
    }
    found
}

/// The first [`TokenFile::MAX_BYTES`] bytes of the regular file `path`, through its
/// links as the sandbox reads it, and without waiting on a fifo. It blocks.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    let limit = u64::try_from(TokenFile::MAX_BYTES).unwrap_or(u64::MAX);
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The hosts that the git config `entries` hold a credential for: the hosts of
/// [`git_tokens`], every remote host when it says so, and `@github-read` for an
/// unscoped extra header.
pub(crate) fn git_hosts(entries: &[remotes::Entry]) -> Vec<String> {
    let tokens = git_tokens(entries);
    let mut hosts = tokens.hosts;
    if tokens.every_remote {
        for remote in remotes::remotes(entries, None, &[]) {
            hosts.extend(remote.fetch.iter().chain(&remote.push).map(|url| url.host.clone()));
        }
    }
    if tokens.github_read {
        hosts.extend(Preset::GithubRead.hosts().iter().map(|host| (*host).to_owned()));
    }
    hosts.sort();
    hosts.dedup();
    hosts
}

#[cfg(test)]
mod tests;
