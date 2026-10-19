//! The git remote facts of a line (phase D2, DP22): for each directory of
//! [`FactRequest::remotes`](efr_permissions::FactRequest), every remote of its
//! repository with each fetch and push URL as host, port, ssh and credentials, the
//! remote of the current branch, and the submodule URLs.
//!
//! One hardened `git config --show-origin -z --get-regexp` run per repository reads
//! the remotes, the `insteadOf` rules, the branches' remotes and the credential
//! settings; git applies the system, global, XDG and include files itself. A
//! repository with a `.gitmodules` file gets one more run over it. A run that fails or
//! passes its time leaves the fact out: the directory then has no remote hosts.
//!
//! A remote is the project's own ([`RemoteFact::project`]) only for the top `.git` dir
//! of a registered project, or the git dir of its worktree record, whose config files
//! did not change since the conversation's first fact request, and whose URLs equal
//! the ones that efrd read at that request (also through an `[include]` file or the
//! system config, which have no stamp). The snapshot lives in memory
//! (check C5-7): after an efrd restart, the first fact request of a conversation takes
//! it again.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use efr_permissions::network::host_and_port;
use efr_permissions::{RemoteFact, RemoteUrl};
use efr_protocol::ConversationId;
use efr_scope::{Git, Home};

use super::HARDENED;

/// How long one git run for the remote facts may take.
const GIT_TIMEOUT: Duration = Duration::from_millis(200);

/// The keys that one git run reads: the remotes, the URL rewrites, the branches'
/// remotes, the submodule URLs, and the settings that put a credential in a request
/// (DP25). Git matches them on the key with its section and variable in lower case.
const KEYS: &str = r"^(remote\..*\.(url|pushurl)|url\..*\.(insteadof|pushinsteadof)|branch\..*\.remote|submodule\..*\.url|http\..*extraheader|credential\..*helper)$";

/// The keys of `.gitmodules` that name a submodule's URL.
const SUBMODULE_KEYS: &str = r"^submodule\..*\.url$";

/// The most conversations whose snapshots efrd keeps; the oldest goes first.
const MAX_CONVERSATIONS: usize = 256;

/// One git config entry: where git read it (`file:<path>`), its key and its value.
pub(crate) type Entry = (String, String, String);

/// What tells that a config file changed: its modification and change times, its
/// inode and its size; `None` for a file that does not exist.
type Stamp = Option<(i64, i64, i64, i64, u64, u64)>;

/// A remote as the snapshot keeps it: its name and its fetch and push URLs.
type Signature = (String, Vec<RemoteUrl>, Vec<RemoteUrl>);

/// The config entries of the repository that holds `dir`, from one hardened git run;
/// `None` when git failed to answer in time. A repository with no such key gives an
/// empty list.
pub(crate) async fn config_entries(git: &Git, home: &Home, dir: &Path) -> Option<Vec<Entry>> {
    let mut args: Vec<&str> = HARDENED.to_vec();
    args.extend(["config", "--show-origin", "-z", "--get-regexp", KEYS]);
    run_config(git, home, dir, args).await
}

/// The submodule URLs of `gitmodules`, a `.gitmodules` file, from one hardened git run.
async fn submodule_entries(git: &Git, home: &Home, gitmodules: &Path) -> Option<Vec<Entry>> {
    let dir = gitmodules.parent()?;
    let file = gitmodules.to_string_lossy();
    let mut args: Vec<&str> = HARDENED.to_vec();
    args.extend(["config", "--show-origin", "-z", "-f", &file, "--get-regexp", SUBMODULE_KEYS]);
    run_config(git, home, dir, args).await
}

async fn run_config(git: &Git, home: &Home, dir: &Path, args: Vec<&str>) -> Option<Vec<Entry>> {
    if !dir.is_dir() {
        return None;
    }
    let git = git.clone().with_timeout(GIT_TIMEOUT);
    match git.run(dir, home, args).await {
        Ok(Some(bytes)) => Some(parse_entries(&bytes)),
        // NOTE: `--get-regexp` exits with 1 when no key matches.
        Ok(None) => Some(Vec::new()),
        Err(error) => {
            tracing::debug!(error = %error, dir = %dir.display(), "the git config of a directory is not known");
            None
        }
    }
}

/// The current branch of the repository that holds `dir`, from a hardened git run.
async fn current_branch(git: &Git, home: &Home, dir: &Path) -> Option<String> {
    let mut args: Vec<&str> = HARDENED.to_vec();
    args.extend(["symbolic-ref", "--quiet", "--short", "HEAD"]);
    let git = git.clone().with_timeout(GIT_TIMEOUT);
    let bytes = git.run(dir, home, args).await.ok()??;
    String::from_utf8(bytes).ok().filter(|branch| !branch.is_empty())
}

/// The entries of `git config --show-origin -z` output: `origin NUL key LF value NUL`,
/// or `origin NUL key NUL` for a key with no value.
pub(crate) fn parse_entries(bytes: &[u8]) -> Vec<Entry> {
    let text = String::from_utf8_lossy(bytes);
    let mut fields = text.split('\0');
    let mut entries = Vec::new();
    while let (Some(origin), Some(pair)) = (fields.next(), fields.next()) {
        if origin.is_empty() && pair.is_empty() {
            break;
        }
        let (key, value) = pair.split_once('\n').unwrap_or((pair, ""));
        entries.push((origin.to_owned(), key.to_owned(), value.to_owned()));
    }
    entries
}

/// `key` as its section, its subsection and its variable: `remote.origin.url` gives
/// `("remote", Some("origin"), "url")`; a subsection may hold dots.
fn split_key(key: &str) -> Option<(String, Option<&str>, String)> {
    let (section, rest) = key.split_once('.')?;
    let (subsection, variable) = match rest.rsplit_once('.') {
        Some((subsection, variable)) => (Some(subsection), variable),
        None => (None, rest),
    };
    Some((section.to_ascii_lowercase(), subsection, variable.to_ascii_lowercase()))
}

/// The host and port of a remote URL, as git reads it: `https://`, `http://`,
/// `ssh://`, `git+ssh://`, `git://` and the scp form `user@host:path`. `None` for a
/// local path, a `file://` URL, a transport that efr does not know, or a host that is
/// not a valid name.
pub(crate) fn remote_url(url: &str) -> Option<RemoteUrl> {
    let url = url.trim();
    if let Some((scheme, rest)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((userinfo, hostport)) => (Some(userinfo), hostport),
            None => (None, authority),
        };
        let explicit_port = has_port(hostport);
        let (ssh, default_port, web) = match scheme.as_str() {
            "https" => (false, 443, true),
            "http" => (false, 80, true),
            "ssh" | "git+ssh" | "ssh+git" => (true, 22, false),
            "git" => (false, 9418, false),
            _ => return None,
        };
        let (host, port) = host_and_port(&format!("{scheme}://{hostport}"))?;
        let port = if explicit_port { port } else { default_port };
        let credentials = web && userinfo.is_some_and(|userinfo| !userinfo.is_empty());
        return Some(RemoteUrl { host, port, ssh, credentials });
    }
    // NOTE: git reads `host:path` as the scp form only when no `/` comes before the
    // first `:`; a path with a `/` first is a local path.
    let (before, after) = url.split_once(':')?;
    // NOTE: `<transport>::<address>` runs a remote helper, which efr does not read.
    if before.is_empty() || before.contains('/') || after.starts_with(':') {
        return None;
    }
    let host = before.rsplit_once('@').map_or(before, |(_, host)| host);
    let host = host.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(host);
    let (host, _) = host_and_port(host)?;
    Some(RemoteUrl { host, port: 22, ssh: true, credentials: false })
}

/// True when `hostport` names a port: `host:22` or `[::1]:22`.
fn has_port(hostport: &str) -> bool {
    match hostport.strip_prefix('[') {
        Some(rest) => rest.split_once(']').is_some_and(|(_, after)| after.starts_with(':')),
        None => hostport.contains(':'),
    }
}

/// The remotes of `entries` (a repository's config) and `submodules` (the URLs of its
/// `.gitmodules`), with `branch` the current branch. URLs that efr cannot read are left
/// out; a remote with no URL left is left out too.
pub(crate) fn remotes(
    entries: &[Entry],
    branch: Option<&str>,
    submodules: &[Entry],
) -> Vec<RemoteFact> {
    let mut urls: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut instead: Vec<(String, String)> = Vec::new();
    let mut push_instead: Vec<(String, String)> = Vec::new();
    let mut upstream: Option<String> = None;
    let mut submodule_urls: Vec<(String, String)> = Vec::new();
    for (_, key, value) in entries {
        let Some((section, subsection, variable)) = split_key(key) else { continue };
        let value = value.trim().to_owned();
        match (section.as_str(), subsection, variable.as_str()) {
            ("remote", Some(name), "url" | "pushurl") => {
                let at = match urls.iter().position(|(known, _, _)| known == name) {
                    Some(at) => at,
                    None => {
                        urls.push((name.to_owned(), Vec::new(), Vec::new()));
                        urls.len() - 1
                    }
                };
                if variable == "url" {
                    urls[at].1.push(value);
                } else {
                    urls[at].2.push(value);
                }
            }
            ("url", Some(base), "insteadof") => instead.push((value, base.to_owned())),
            ("url", Some(base), "pushinsteadof") => push_instead.push((value, base.to_owned())),
            ("branch", Some(name), "remote") if Some(name) == branch => upstream = Some(value),
            ("submodule", Some(name), "url") => submodule_urls.push((name.to_owned(), value)),
            _ => {}
        }
    }
    for (_, key, value) in submodules {
        if let Some(("submodule", Some(name), "url")) =
            split_key(key).as_ref().map(|(s, sub, v)| (s.as_str(), *sub, v.as_str()))
        {
            submodule_urls.push((name.to_owned(), value.trim().to_owned()));
        }
    }
    let mut found = Vec::new();
    for (name, fetch_urls, push_urls) in urls {
        let fetch = read_urls(fetch_urls.iter().map(|url| rewrite(url, &instead)));
        // NOTE: git pushes to each `pushurl` (after `insteadOf`), else to each `url`,
        // after `pushInsteadOf` when one matches and `insteadOf` when none does.
        let push = if push_urls.is_empty() {
            read_urls(fetch_urls.iter().map(|url| match longest(url, &push_instead) {
                Some(rewritten) => rewritten,
                None => rewrite(url, &instead),
            }))
        } else {
            read_urls(push_urls.iter().map(|url| rewrite(url, &instead)))
        };
        if fetch.is_empty() && push.is_empty() {
            continue;
        }
        let upstream = upstream.as_deref() == Some(name.as_str());
        found.push(RemoteFact { name, fetch, push, project: false, upstream, submodule: false });
    }
    for (name, url) in submodule_urls {
        let fetch = read_urls(std::iter::once(rewrite(&url, &instead)));
        if fetch.is_empty() {
            continue;
        }
        let known = found.iter_mut().find(|remote| remote.submodule && remote.name == name);
        match known {
            Some(remote) => {
                for url in fetch {
                    if !remote.fetch.contains(&url) {
                        remote.push.push(url.clone());
                        remote.fetch.push(url);
                    }
                }
            }
            None => found.push(RemoteFact {
                name,
                push: fetch.clone(),
                fetch,
                project: false,
                upstream: false,
                submodule: true,
            }),
        }
    }
    found
}

/// `url` after the longest `insteadOf` prefix of `rules` that it starts with.
fn rewrite(url: &str, rules: &[(String, String)]) -> String {
    longest(url, rules).unwrap_or_else(|| url.to_owned())
}

/// `url` with the longest prefix of `rules` that it starts with replaced by its base;
/// `None` when no prefix matches.
fn longest(url: &str, rules: &[(String, String)]) -> Option<String> {
    rules
        .iter()
        .filter(|(prefix, _)| !prefix.is_empty() && url.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(prefix, base)| format!("{base}{}", &url[prefix.len()..]))
}

fn read_urls(urls: impl Iterator<Item = String>) -> Vec<RemoteUrl> {
    let mut found: Vec<RemoteUrl> = Vec::new();
    for url in urls.filter_map(|url| remote_url(&url)) {
        if !found.contains(&url) {
            found.push(url);
        }
    }
    found
}

/// The top of the work tree that holds `dir`: the nearest dir at or above it with a
/// `.git` entry, as git finds it. It blocks.
pub(crate) fn work_tree(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|at| std::fs::symlink_metadata(at.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

/// The config files whose change ends a project's snapshot: the repository's own
/// (`<root>/.git/config`, or the common dir's for a worktree record), the user's
/// global and XDG files.
fn config_files(git_dir: &Path, home: &Path) -> Vec<PathBuf> {
    vec![git_dir.join("config"), home.join(".gitconfig"), home.join(".config/git/config")]
}

fn stamp_of(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.mtime(), meta.mtime_nsec(), meta.ctime(), meta.ctime_nsec(), meta.ino(), meta.len()))
}

/// The per-conversation snapshots of the pinned project git dirs' remotes (DP22).
#[derive(Debug, Default)]
pub(crate) struct Snapshots {
    conversations: Mutex<HashMap<ConversationId, Snapshot>>,
}

/// One conversation's snapshot: the stamps of the config files of every pinned git dir
/// at its first fact request, and the remotes that efrd read for each one then.
#[derive(Debug, Default)]
struct Snapshot {
    /// When it was taken, in the order of the conversations.
    order: u64,
    stamps: HashMap<PathBuf, Stamp>,
    repos: HashMap<PathBuf, Vec<Signature>>,
}

impl Snapshots {
    /// True when `conversation` made its first fact request already.
    pub(crate) fn has(&self, conversation: ConversationId) -> bool {
        self.lock().contains_key(&conversation)
    }

    /// Notes the first fact request of `conversation`: the stamps of the config files
    /// of `pinned`, each pinned project's work tree with its git dir. It blocks.
    pub(crate) fn begin(
        &self,
        conversation: ConversationId,
        pinned: &[(PathBuf, PathBuf)],
        home: &Path,
    ) {
        let mut conversations = self.lock();
        if conversations.contains_key(&conversation) {
            return;
        }
        let order = conversations.values().map(|snapshot| snapshot.order + 1).max().unwrap_or(0);
        let mut stamps = HashMap::new();
        for (_, git_dir) in pinned {
            for file in config_files(git_dir, home) {
                let stamp = stamp_of(&file);
                stamps.insert(file, stamp);
            }
        }
        conversations.insert(conversation, Snapshot { order, stamps, repos: HashMap::new() });
        if conversations.len() > MAX_CONVERSATIONS
            && let Some(oldest) =
                conversations.iter().min_by_key(|(_, snapshot)| snapshot.order).map(|(id, _)| *id)
        {
            conversations.remove(&oldest);
        }
    }

    /// Keeps `remotes`, the remotes of the pinned project at `root` that efrd read at
    /// the first fact request of `conversation`, as the project's own. A later call for
    /// the same root changes nothing.
    pub(crate) fn baseline(
        &self,
        conversation: ConversationId,
        root: &Path,
        remotes: &[RemoteFact],
    ) {
        let mut conversations = self.lock();
        let Some(snapshot) = conversations.get_mut(&conversation) else { return };
        let signatures: Vec<Signature> = remotes
            .iter()
            .filter(|remote| !remote.submodule)
            .map(|remote| (remote.name.clone(), remote.fetch.clone(), remote.push.clone()))
            .collect();
        snapshot.repos.entry(root.to_path_buf()).or_insert(signatures);
    }

    /// Sets [`RemoteFact::project`] on each remote of `remotes` (never a submodule) of
    /// the pinned git dir `git_dir` whose URLs equal the ones that efrd read at the
    /// conversation's first fact request ([`baseline`](Self::baseline)), while the git
    /// dir's config files did not change since then. A git dir with no baseline has no
    /// project remote. It blocks.
    pub(crate) fn mark(
        &self,
        conversation: ConversationId,
        root: &Path,
        git_dir: &Path,
        home: &Path,
        remotes: &mut [RemoteFact],
    ) {
        let conversations = self.lock();
        let Some(snapshot) = conversations.get(&conversation) else { return };
        let Some(first) = snapshot.repos.get(root) else { return };
        let unchanged = config_files(git_dir, home)
            .iter()
            .all(|file| snapshot.stamps.get(file).is_some_and(|stamp| *stamp == stamp_of(file)));
        if !unchanged {
            return;
        }
        for remote in remotes.iter_mut().filter(|remote| !remote.submodule) {
            remote.project = first.iter().any(|(name, fetch, push)| {
                *name == remote.name && *fetch == remote.fetch && *push == remote.push
            });
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, Snapshot>> {
        // NOTE: each snapshot changes whole, so a poisoned lock still holds usable ones.
        self.conversations.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The remotes of the repository that holds `dir`: one config run, one run for the
/// current branch, and one over its `.gitmodules` when it has one. `None` when git did
/// not answer.
pub(crate) async fn read(
    git: &Git,
    home: &Home,
    dir: &Path,
) -> Option<(Option<PathBuf>, Vec<RemoteFact>)> {
    let entries = config_entries(git, home, dir).await?;
    let branch = current_branch(git, home, dir).await;
    let owned = dir.to_path_buf();
    let top = tokio::task::spawn_blocking(move || work_tree(&owned)).await.ok().flatten();
    let gitmodules = top.as_ref().map(|top| top.join(".gitmodules")).filter(|file| file.is_file());
    let submodules = match &gitmodules {
        Some(file) => submodule_entries(git, home, file).await.unwrap_or_default(),
        None => Vec::new(),
    };
    Some((top, remotes(&entries, branch.as_deref(), &submodules)))
}

#[cfg(test)]
mod tests;
