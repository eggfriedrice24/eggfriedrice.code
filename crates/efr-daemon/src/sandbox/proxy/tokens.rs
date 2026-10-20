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
//!   unscoped `http.extraHeader` (every remote and `@github-read`), and a credential
//!   helper that can hand the call a token. The call's own dir is always one of them, so
//!   the user's global files count also outside a project, and a project at a subdir of
//!   a repository counts the repository's config.
//!
//! A credential helper denies the hosts of its scope: its URL's host, or every remote
//! for an unscoped helper and for a scope whose host efr cannot read (v0.0.4, DV17). A
//! shell form, a `store` with its own file and a program that efr does not know always
//! deny. gh's helper (`gh auth git-credential`) denies only when the call could reach
//! gh's token ([`HelperFacts::gh_open`], DV15): when the call does not hide gh's
//! `hosts.yml`, when `sandbox.env_keep` keeps a GitHub token variable, or when the call
//! has the whole session bus, where gh reads the keyring.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use efr_permissions::Preset;
use efr_permissions::network::tokens::{
    GitTokens, HelperKind, HelperScope, TokenFile, git_tokens, token_hosts,
};
use efr_scope::{Git, Home};

use crate::sandbox::facts::remotes;

/// The variables of the hidden shell that choose gh's config dir, as gh reads them.
pub(crate) const GH_DIR_VARIABLES: [&str; 2] = ["GH_CONFIG_DIR", "XDG_CONFIG_HOME"];

/// The variables from which gh takes a token before its `hosts.yml`.
pub(crate) const GH_TOKEN_VARIABLES: [&str; 4] =
    ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];

/// The hosts that a call may not reach, and the notes that say why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Denied {
    /// The hosts, sorted, each once.
    pub(crate) hosts: Vec<String>,
    /// One note per file or repository that holds a credential, and one per helper
    /// reason.
    pub(crate) notes: Vec<String>,
}

impl Denied {
    /// Denies `hosts` with today's note: `source` holds a credential for them.
    fn add(&mut self, hosts: Vec<String>, source: &str) {
        let note = |hosts: &str| {
            format!(
                "[efr: efr blocks {hosts} for this command: {source} holds a credential for \
                 it. Move the credential to a file that the sandbox hides, such as ~/.npmrc.]"
            )
        };
        self.add_with(hosts, note);
    }

    /// Denies `hosts` with the note that `note` makes from their list.
    fn add_with(&mut self, hosts: Vec<String>, note: impl Fn(&str) -> String) {
        if hosts.is_empty() {
            return;
        }
        let note = note(&hosts.join(", "));
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
        self.hosts.extend(hosts);
        self.hosts.sort();
        self.hosts.dedup();
    }
}

/// What decides whether a call can reach gh's token (DV15): where the hidden shell
/// points gh, the call's environment lists, its masks and its bus grants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HelperFacts {
    /// The [`GH_DIR_VARIABLES`] of the hidden shell's environment (a job's own); `None`
    /// when efrd does not know that environment.
    pub(crate) env: Option<BTreeMap<String, String>>,
    /// `sandbox.env_keep`.
    pub(crate) keep: Vec<String>,
    /// `sandbox.env_deny`, which wins over `env_keep`.
    pub(crate) deny: Vec<String>,
    /// The paths that the call reads as empty: its masks, less the ones that a grant of
    /// the call opens.
    pub(crate) masks: Vec<PathBuf>,
    /// The call has the whole session bus, where gh reads its token from the keyring.
    pub(crate) session_bus: bool,
}

impl HelperFacts {
    /// The [`GH_DIR_VARIABLES`] of the `env` file bytes `env` (NUL-separated
    /// `NAME=value` entries).
    pub(crate) fn gh_env(env: &[u8]) -> BTreeMap<String, String> {
        env.split(|byte| *byte == 0)
            .filter_map(|entry| std::str::from_utf8(entry).ok()?.split_once('='))
            .filter(|(name, _)| GH_DIR_VARIABLES.contains(name))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }

    /// Why the call can reach gh's token, as the end of a note; `None` when the sandbox
    /// hides it. `home` is the home dir, where gh's config dir lies by default.
    pub(crate) fn gh_open(&self, home: &Path) -> Option<String> {
        let Some(env) = &self.env else {
            return Some(
                "efr does not know the hidden shell's environment, so it cannot tell where gh \
                 reads its token"
                    .to_owned(),
            );
        };
        let set = |name: &str| env.get(name).map(String::as_str).filter(|value| !value.is_empty());
        let (variable, file) = match (set("GH_CONFIG_DIR"), set("XDG_CONFIG_HOME")) {
            (Some(dir), _) => (Some("GH_CONFIG_DIR"), PathBuf::from(dir).join("hosts.yml")),
            (None, Some(dir)) => (Some("XDG_CONFIG_HOME"), PathBuf::from(dir).join("gh/hosts.yml")),
            (None, None) => (None, home.join(".config/gh/hosts.yml")),
        };
        if !file.is_absolute() {
            let variable = variable.unwrap_or("HOME");
            return Some(format!(
                "{variable} is not an absolute path, so efr cannot tell where gh reads its token"
            ));
        }
        if !self.masks.iter().any(|mask| file.starts_with(mask)) {
            return Some(format!(
                "gh's credential helper reads its token from {}, which the sandbox does not hide",
                file.display()
            ));
        }
        let kept = GH_TOKEN_VARIABLES
            .iter()
            .find(|name| matches_any(&self.keep, name) && !matches_any(&self.deny, name));
        if let Some(name) = kept {
            return Some(format!(
                "sandbox.env_keep keeps {name}, which gives gh's credential helper a token"
            ));
        }
        if self.session_bus {
            return Some(
                "the command has the whole session bus, where gh's credential helper reads its \
                 token from the keyring"
                    .to_owned(),
            );
        }
        None
    }
}

/// True when a pattern of `patterns` (with `*`) matches `name`.
fn matches_any(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|pattern| efr_sandbox::name_matches(pattern, name))
}

/// The hosts that the registry files of `dirs` and the git config of each of `dirs`
/// hold a credential for, or that a credential helper of that config can hand a token
/// for. `dirs` holds the call's project roots and its own dir.
pub(crate) async fn denied(
    git: &Git,
    home: &Home,
    dirs: &[PathBuf],
    helpers: &HelperFacts,
) -> Denied {
    let mut denied = Denied::default();
    let owned = dirs.to_vec();
    let scanned = tokio::task::spawn_blocking(move || scan_files(&owned)).await.unwrap_or_default();
    for (file, hosts) in scanned {
        denied.add(hosts, &file.display().to_string());
    }
    let gh_open = helpers.gh_open(home.path());
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
        let source = match &top {
            Some(top) => format!("the git config of {}", top.display()),
            None => "your git config".to_owned(),
        };
        git_denied(&mut denied, &entries, &source, gh_open.as_deref());
    }
    denied
}

/// Adds what the git config `entries` of `source` deny: the hosts of [`git_tokens`],
/// every remote host when it says so, `@github-read` for an unscoped extra header, and
/// the hosts of each credential helper that denies. `gh_open` says why the call can
/// reach gh's token, if it can.
fn git_denied(
    denied: &mut Denied,
    entries: &[remotes::Entry],
    source: &str,
    gh_open: Option<&str>,
) {
    let tokens = git_tokens(entries);
    let every: Vec<String> = remote_hosts(entries);
    let mut hosts = credential_hosts(&tokens, &every);
    let mut unknown: Vec<(String, Vec<String>)> = Vec::new();
    let mut gh: Vec<String> = Vec::new();
    for helper in &tokens.helpers {
        let scoped = match &helper.scope {
            HelperScope::Host(host) => vec![host.clone()],
            HelperScope::Every => every.clone(),
        };
        match helper.kind {
            HelperKind::Gh if gh_open.is_some() => gh.extend(scoped),
            HelperKind::Unknown => {
                match unknown.iter_mut().find(|(name, _)| *name == helper.program) {
                    Some((_, known)) => known.extend(scoped),
                    None => unknown.push((helper.program.clone(), scoped)),
                }
            }
            kind if kind.denies(gh_open.is_none()) => hosts.extend(scoped),
            _ => {}
        }
    }
    denied.add(sorted(hosts), source);
    for (program, hosts) in unknown {
        denied.add_with(sorted(hosts), |hosts| {
            format!(
                "[efr: efr blocks {hosts} for this command: {source} names the credential helper \
                 {program}, and a credential helper that efr does not know can hand out a token.]"
            )
        });
    }
    if let Some(reason) = gh_open {
        denied.add_with(sorted(gh), |hosts| {
            format!("[efr: efr blocks {hosts} for this command: {reason}.]")
        });
    }
}

/// The hosts of [`git_tokens`]' credentials: its hosts, the remote hosts `every` when
/// it denies every remote, and `@github-read` for an unscoped extra header.
fn credential_hosts(tokens: &GitTokens, every: &[String]) -> Vec<String> {
    let mut hosts = tokens.hosts.clone();
    if tokens.every_remote {
        hosts.extend(every.iter().cloned());
    }
    if tokens.github_read {
        hosts.extend(Preset::GithubRead.hosts().iter().map(|host| (*host).to_owned()));
    }
    hosts
}

/// The hosts of every remote of the git config `entries`, fetch and push.
fn remote_hosts(entries: &[remotes::Entry]) -> Vec<String> {
    let mut hosts: Vec<String> = remotes::remotes(entries, None, &[])
        .iter()
        .flat_map(|remote| remote.fetch.iter().chain(&remote.push).map(|url| url.host.clone()))
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

fn sorted(mut hosts: Vec<String>) -> Vec<String> {
    hosts.sort();
    hosts.dedup();
    hosts
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

#[cfg(test)]
mod tests;
