//! Credentials in files that a contained call must read (DP25).
//!
//! The sandbox cannot hide one line of a file, so a registry token in a project's
//! `.npmrc` or a token in a git remote URL stays readable. efrd denies the hosts of such
//! a credential for the call instead. These scans only read lines: no TOML, YAML or ini
//! parser touches the files. efrd reads the files and runs git; this module is pure.

use std::collections::BTreeSet;

pub use self::helper::{Helper, HelperKind, HelperScope, helper, helper_kind};
use super::{Preset, host_and_port};

mod helper;

/// A registry config file that can hold a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenFile {
    /// `.npmrc` (npm, pnpm and bun read it).
    Npmrc,
    /// `.yarnrc.yml` (yarn 2 and later).
    YarnrcYml,
    /// `.yarnrc` (yarn 1).
    Yarnrc,
    /// `bunfig.toml`.
    Bunfig,
    /// `.cargo/config.toml` or `.cargo/config`.
    CargoConfig,
}

impl TokenFile {
    /// The file names that efrd reads in a project root: `.npmrc`, `.yarnrc.yml`,
    /// `.yarnrc`, `bunfig.toml`, `.cargo/config.toml`, `.cargo/config`.
    pub const FILES: [(&'static str, TokenFile); 6] = [
        (".npmrc", TokenFile::Npmrc),
        (".yarnrc.yml", TokenFile::YarnrcYml),
        (".yarnrc", TokenFile::Yarnrc),
        ("bunfig.toml", TokenFile::Bunfig),
        (".cargo/config.toml", TokenFile::CargoConfig),
        (".cargo/config", TokenFile::CargoConfig),
    ];

    /// The most bytes of one file that efrd reads and [`token_hosts`] scans.
    pub const MAX_BYTES: usize = 64 * 1024;

    /// The default registry hosts of the file's ecosystem, which a token without a host
    /// goes to.
    const fn registry(self) -> Preset {
        match self {
            TokenFile::CargoConfig => Preset::Crates,
            TokenFile::Npmrc | TokenFile::YarnrcYml | TokenFile::Yarnrc | TokenFile::Bunfig => {
                Preset::Npm
            }
        }
    }
}

/// The keys that hold a token, in lower case: npm's `_authToken`, `_auth` and
/// `_password`, yarn's `npmAuthToken` and `npmAuthIdent`, and the `token` of cargo and
/// bun.
const TOKEN_KEYS: [&str; 6] =
    ["_authtoken", "_auth", "_password", "npmauthtoken", "npmauthident", "token"];

/// The hosts that a file with a token key or a URL with a password names, plus the
/// default registry hosts of its ecosystem, sorted; empty when the file has neither.
/// Only the first [`TokenFile::MAX_BYTES`] bytes count. Every host of the file counts,
/// not only the host next to the token: a line scan cannot tell which registry a token
/// belongs to, and a deny of one host too many costs only a fetch.
pub fn token_hosts(file: TokenFile, text: &str) -> Vec<String> {
    let mut end = text.len().min(TokenFile::MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut credential = false;
    let mut hosts = BTreeSet::new();
    for line in text[..end].lines() {
        let line = line.trim_start();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        credential |= has_token_key(line, file);
        for (host, password) in url_hosts(line) {
            credential |= password;
            hosts.insert(host);
        }
    }
    if !credential {
        return Vec::new();
    }
    hosts.extend(file.registry().hosts().iter().map(|host| (*host).to_owned()));
    hosts.into_iter().collect()
}

/// What the git config entries hold, from `git config --show-origin --get-regexp`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitTokens {
    /// Hosts of remotes, `insteadOf` bases and scoped `extraHeader`s with a credential,
    /// sorted.
    pub hosts: Vec<String>,
    /// An `http.extraHeader` with no URL, or with one whose host efr cannot read: deny
    /// every remote host (and `@github-read` for an unscoped header).
    pub every_remote: bool,
    /// An unscoped `http.extraHeader`: deny the hosts of `@github-read` too, because the
    /// header goes to every https host that git reaches.
    pub github_read: bool,
    /// The credential helpers that can give a call a token, in the order of the config,
    /// each once. Whether one denies its hosts depends on the call
    /// ([`HelperKind::denies`]), so efrd decides.
    pub helpers: Vec<Helper>,
}

/// What these git config entries hold, each an origin, a key and a value, such as
/// `("file:.git/config", "remote.origin.url", "https://u:tok@git.test/r.git")`. Git
/// writes the section and the variable of a key in lower case and the subsection as it
/// is; this scan reads both forms.
///
/// - `remote.<name>.url`, `remote.<name>.pushurl` and `submodule.<name>.url` with user
///   info in an `http://` or `https://` URL: that host.
/// - `url.<base>.insteadOf` and `url.<base>.pushInsteadOf` whose base has such user
///   info: the base's host.
/// - `http.<url>.extraHeader` with a value: the URL's host, or every remote when the URL
///   names no host that efr can read.
/// - `http.extraHeader` with a value: every remote and `@github-read`.
/// - `credential.helper` or `credential.<url>.helper` that [`helper`] keeps (gh's
///   helper, a shell form, a `store` with `--file`, whose file is readable in the
///   sandbox wherever it is, or a program that efr does not know): in
///   [`helpers`](GitTokens::helpers), with its scope.
pub fn git_tokens(entries: &[(String, String, String)]) -> GitTokens {
    let mut tokens = GitTokens::default();
    let mut hosts = BTreeSet::new();
    for (_origin, key, value) in entries {
        let Some((section, rest)) = key.split_once('.') else { continue };
        let (subsection, variable) = match rest.rsplit_once('.') {
            Some((subsection, variable)) => (Some(subsection), variable),
            None => (None, rest),
        };
        let section = section.to_ascii_lowercase();
        let variable = variable.to_ascii_lowercase();
        let value = value.trim();
        match (section.as_str(), subsection, variable.as_str()) {
            ("remote", Some(_), "url" | "pushurl") | ("submodule", Some(_), "url") => {
                hosts.extend(credential_host(value));
            }
            ("url", Some(base), "insteadof" | "pushinsteadof") => {
                hosts.extend(credential_host(base));
            }
            ("http", Some(url), "extraheader") if !value.is_empty() => match host_and_port(url) {
                Some((host, _)) => {
                    hosts.insert(host);
                }
                None => tokens.every_remote = true,
            },
            ("http", None, "extraheader") if !value.is_empty() => {
                tokens.every_remote = true;
                tokens.github_read = true;
            }
            ("credential", scope, "helper") => {
                if let Some(found) = helper(scope, value)
                    && !tokens.helpers.contains(&found)
                {
                    tokens.helpers.push(found);
                }
            }
            _ => {}
        }
    }
    tokens.hosts = hosts.into_iter().collect();
    tokens
}

/// True when `line` sets one of [`TOKEN_KEYS`]: the key stands alone as a word, after
/// a `:`, a `/`, a quote, a space or the start, and is followed by `=` or `:` (or, in
/// `.yarnrc`, by a space and a value).
fn has_token_key(line: &str, file: TokenFile) -> bool {
    let lower = line.to_ascii_lowercase();
    TOKEN_KEYS.iter().any(|key| {
        lower.match_indices(key).any(|(at, _)| {
            let before = lower[..at].bytes().next_back();
            let starts = before.is_none_or(|byte| !is_word_byte(byte));
            let after = &lower[at + key.len()..];
            starts && sets_value(after, file)
        })
    })
}

/// True when the text after a key sets its value.
fn sets_value(after: &str, file: TokenFile) -> bool {
    if after.bytes().next().is_some_and(is_word_byte) {
        return false;
    }
    let after = after.strip_prefix(['"', '\'']).unwrap_or(after);
    let value = after.trim_start();
    if value.starts_with(['=', ':']) {
        return true;
    }
    file == TokenFile::Yarnrc && value.len() < after.len() && !value.is_empty()
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// The host of each `//host` of a line, and whether its user info holds a password
/// (`//user:password@host`).
fn url_hosts(line: &str) -> Vec<(String, bool)> {
    let stops = ['/', '?', '#', '"', '\'', ' ', '\t', ',', ']', '}', ')', ';', '`', '\\'];
    line.match_indices("//")
        .filter_map(|(at, _)| {
            let rest = &line[at + 2..];
            let authority = rest.split(stops).next().unwrap_or_default();
            let password =
                authority.rsplit_once('@').is_some_and(|(userinfo, _)| userinfo.contains(':'));
            host_and_port(authority).map(|(host, _)| (host, password))
        })
        .collect()
}

/// The host of an `http://` or `https://` URL with user info, such as
/// `https://u:tok@git.test/r.git` or `https://ghp_x@github.com/o/r`. An ssh URL has a
/// user name but no credential in it.
fn credential_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (userinfo, _) = authority.rsplit_once('@')?;
    if userinfo.is_empty() {
        return None;
    }
    host_and_port(authority).map(|(host, _)| host)
}

#[cfg(test)]
mod tests;
