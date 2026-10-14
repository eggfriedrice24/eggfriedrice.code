//! The git commands that reach a remote, and the hosts behind their remote names
//! (phase D2, DP22).
//!
//! A line such as `git fetch origin` or `git push` names no host: the host is in the
//! git config. efrd reads the remotes of each directory that [`remote_dir`] names
//! (`CallFacts::remotes`), and [`selected`] picks the remotes that the command uses, so
//! the exits of the line carry every host that it reaches.

use std::path::{Path, PathBuf};

use efr_protocol::{ExitKind, ExitSource, Grant};

use super::programs::{ProxyRoute, base_name, proxy_route};
use super::scan::is_assignment;
use super::{Approver, ExitNeed};
use crate::{CallFacts, HostList, RemoteFact, RemoteUrl};

/// What a git command does with its remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteUse {
    /// It fetches: `fetch`, `pull`, `ls-remote`, `remote update`.
    Fetch,
    /// It pushes: `push`.
    Push,
    /// It fetches the submodules of `.gitmodules`: `submodule update`.
    Submodules,
}

/// The options of git before its subcommand that take the next word as their value.
const GIT_VALUES: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

/// The options of git before its subcommand that make the remote facts of the
/// directory wrong: another repository, or a config value of the line, such as
/// `-c remote.origin.url=...` or `-c url.<base>.insteadOf=...`.
const STEERING: &[&str] = &["-c", "--git-dir", "--work-tree", "--namespace", "--config-env"];

/// The options of `fetch`, `pull`, `ls-remote`, `push` and `remote update` that take
/// the next word as their value.
const VALUE_OPTIONS: &[&str] = &[
    "--depth",
    "--deepen",
    "--shallow-since",
    "--shallow-exclude",
    "-j",
    "--jobs",
    "--upload-pack",
    "--receive-pack",
    "--exec",
    "--repo",
    "-o",
    "--push-option",
    "--server-option",
    "--refmap",
    "--negotiation-tip",
    "--recurse-submodules-default",
    "-s",
    "--strategy",
    "-X",
    "--strategy-option",
];

/// A git command of a line that reaches a remote, as [`remote_command`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteCommand<'a> {
    /// What it does with the remote.
    pub(super) kind: RemoteUse,
    /// The remote that the line names, such as `upstream` in `git fetch upstream`;
    /// `None` when it names none, and also when it names a URL or a path.
    pub(super) named: Option<&'a str>,
    /// True for `fetch --all`, `fetch --multiple` and `remote update`, which use every
    /// remote.
    pub(super) all: bool,
}

/// The remote use of the git command `words` (its program word first), or `None` for
/// another command, for a git command that reaches no remote, and for one whose options
/// make the facts of its directory wrong ([`STEERING`]).
pub(super) fn remote_command(words: &[String]) -> Option<RemoteCommand<'_>> {
    let (first, args) = words.split_first()?;
    if base_name(first) != "git" {
        return None;
    }
    let mut at = 0;
    while let Some(word) = args.get(at) {
        let option = word.split_once('=').map_or(word.as_str(), |(option, _)| option);
        if STEERING.contains(&option) {
            return None;
        }
        if GIT_VALUES.contains(&word.as_str()) {
            at += 2;
        } else if word.starts_with('-') {
            at += 1;
        } else {
            break;
        }
    }
    let sub = args.get(at)?.as_str();
    let rest = &args[at + 1..];
    let operands = operands(rest);
    let (kind, operands) = match sub {
        "fetch" | "pull" | "ls-remote" => (RemoteUse::Fetch, operands),
        "push" => (RemoteUse::Push, operands),
        "remote" if operands.first() == Some(&"update") => {
            return Some(RemoteCommand { kind: RemoteUse::Fetch, named: None, all: true });
        }
        "submodule" if operands.first() == Some(&"update") => {
            return Some(RemoteCommand { kind: RemoteUse::Submodules, named: None, all: true });
        }
        _ => return None,
    };
    let all = rest.iter().any(|word| word == "--all" || word == "--multiple");
    let all = all && sub == "fetch";
    let named = operands.first().copied().filter(|word| is_remote_name(word));
    // NOTE: a URL or a path names its host on the line itself.
    if operands.first().is_some_and(|word| !is_remote_name(word)) {
        return None;
    }
    Some(RemoteCommand { kind, named, all })
}

/// The words of `rest` that are no option and no option's value.
fn operands(rest: &[String]) -> Vec<&str> {
    let mut found = Vec::new();
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            found.extend(words.by_ref().map(String::as_str));
            break;
        }
        if VALUE_OPTIONS.contains(&word.as_str()) {
            words.next();
        } else if !word.starts_with('-') {
            found.push(word.as_str());
        }
    }
    found
}

/// True for a word that can only be a remote's name: no `/`, `:`, `$`, `\` or `.` at
/// its start, and no word that only the shell reads.
fn is_remote_name(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with('.')
        && word.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The directory whose remotes the git command `words` uses: the directory of `-C`,
/// else `dir`, the directory the command runs in. `None` when it is not known, such as
/// after a `cd`.
pub(super) fn remote_dir(
    words: &[String],
    dir: Option<&Path>,
    resolve: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    let at = words.iter().position(|word| word == "-C");
    match at.and_then(|at| words.get(at + 1)) {
        Some(other) => resolve(other),
        None => dir.map(Path::to_path_buf),
    }
}

/// True when the segment `words` sets a `GIT_*` variable for its commands, such as
/// `GIT_DIR` or `GIT_CONFIG_COUNT`: the remotes of the directory may then not be the
/// ones that the command uses.
pub(super) fn steers_git(words: &[String]) -> bool {
    words.iter().any(|word| is_assignment(word) && word.starts_with("GIT_"))
}

/// The remotes of `dir` that `command` uses, or `None` when no fact describes them:
/// the named remote; else every remote for `--all` and `remote update`; else the remote
/// of the current branch, else `origin`. `submodule update` uses every submodule URL of
/// `.gitmodules`.
pub(super) fn selected<'f>(
    command: &RemoteCommand<'_>,
    dir: &Path,
    facts: Option<&'f CallFacts>,
) -> Option<Vec<&'f RemoteFact>> {
    let (_, remotes) = facts?.remotes.iter().find(|(known, _)| known == dir)?;
    let remotes: Vec<&RemoteFact> = remotes.iter().collect();
    let (submodules, remotes): (Vec<&RemoteFact>, Vec<&RemoteFact>) =
        remotes.into_iter().partition(|remote| remote.submodule);
    let chosen: Vec<&RemoteFact> = match command.kind {
        RemoteUse::Submodules => submodules,
        _ if command.all => remotes,
        _ => match command.named {
            Some(name) => remotes.into_iter().filter(|remote| remote.name == name).collect(),
            None => {
                let upstream: Vec<&RemoteFact> =
                    remotes.iter().copied().filter(|remote| remote.upstream).collect();
                if upstream.is_empty() {
                    remotes.into_iter().filter(|remote| remote.name == "origin").collect()
                } else {
                    upstream
                }
            }
        },
    };
    (!chosen.is_empty()).then_some(chosen)
}

/// The URLs of `remotes` that `kind` reaches: the push URLs for a push, else the fetch
/// URLs.
pub(super) fn urls<'f>(kind: RemoteUse, remotes: &[&'f RemoteFact]) -> Vec<&'f RemoteUrl> {
    let mut found: Vec<&RemoteUrl> = Vec::new();
    for remote in remotes {
        let list = if kind == RemoteUse::Push { &remote.push } else { &remote.fetch };
        for url in list {
            if !found.contains(&url) {
                found.push(url);
            }
        }
    }
    found
}

/// The exit of a fetch from the remotes `remotes` under efr's proxy (DP20, DP22), for
/// the simple command `part`; `None` when the proxy decides it alone.
///
/// A remote with an ssh URL runs outside the sandbox, with the user's ssh agent and
/// keys, so its `outside` exit names its hosts, and the classifier judges it only when a
/// user message names one ([`Approver::ClassifierWhenNamed`]; the turn also counts a
/// project's own remote as named). A submodule URL comes from `.gitmodules`, which the
/// model can write, so an ssh submodule URL is for the user alone. An https remote
/// follows the list: a host that the list allows or denies needs nothing, an unlisted
/// one gets a grant of its host and port, and one that the proxy cannot serve gets the
/// whole network.
pub(super) fn proxy_fetch_exit(
    kind: RemoteUse,
    remotes: &[&RemoteFact],
    list: &HostList,
    part: &str,
) -> Option<ExitNeed> {
    let urls = urls(kind, remotes);
    let hosts = hosts(&urls);
    if urls.iter().any(|url| url.ssh) {
        let mut need = ExitNeed::new(ExitKind::Outside, part, ExitSource::Predicted);
        need.approver = if kind == RemoteUse::Submodules {
            Approver::User
        } else {
            Approver::ClassifierWhenNamed
        };
        need.names = hosts.clone();
        need.hosts = hosts;
        return Some(need);
    }
    let mut grants: Vec<Grant> = Vec::new();
    for url in &urls {
        match proxy_route(list, &url.host, url.port) {
            ProxyRoute::Listed | ProxyRoute::Denied => {}
            ProxyRoute::Open => {
                grants = vec![Grant::OpenNetwork];
                break;
            }
            ProxyRoute::Grant => {
                let grant = Grant::Host { host: url.host.clone(), port: url.port };
                if !grants.contains(&grant) {
                    grants.push(grant);
                }
            }
        }
    }
    if grants.is_empty() {
        return None;
    }
    let mut need = ExitNeed::new(ExitKind::Host, part, ExitSource::Predicted);
    need.grants = grants;
    need.hosts = hosts;
    Some(need)
}

/// The hosts of `urls`, each once, in order.
pub(super) fn hosts(urls: &[&RemoteUrl]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for url in urls {
        if !found.contains(&url.host) {
            found.push(url.host.clone());
        }
    }
    found
}

#[cfg(test)]
mod tests;
