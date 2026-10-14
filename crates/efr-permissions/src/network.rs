//! The host list of efr's network proxy (spec 8.3, phase D2): the presets, the rules of
//! `network.allow`, `network.deny` and `[[network.hosts]]`, and the verdict for one host
//! and port.
//!
//! Pure: no file and no DNS lookup. efrd reads the pacman files and gives their text to
//! [`parse_pacman`]; the config check, the engine and the proxy share one compiled
//! [`HostList`]. [`tokens`] holds the line scans that find a credential in a registry
//! file or in the git config (DP25).
//!
//! Host syntax (DP2): ASCII only, compared in lower case; labels of `a-z`, `0-9` and
//! `-`, 1 to 63 bytes each; a name of at most 253 bytes; one trailing dot is dropped.
//! `*.suffix` matches the names below a suffix of at least two labels. A name whose last
//! label is all digits or starts with `0x` is an IP address, which only
//! `[[network.hosts]]` may list.

mod list;
mod pacman;
pub mod tokens;

use std::net::{IpAddr, Ipv6Addr};

pub use self::list::HostList;
pub use self::pacman::{PacmanFile, parse_pacman};
pub use crate::error::HostError;

/// A preset of the allow list (spec 8.3): a name for a fixed set of hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Preset {
    /// `@crates`: crates.io, its index and its downloads.
    Crates,
    /// `@npm`: the npm and yarn registries.
    Npm,
    /// `@pypi`: PyPI and its file host.
    Pypi,
    /// `@go`: the Go module proxy and checksum database.
    Go,
    /// `@github-read`: GitHub's web, archive, raw file and release download hosts. No
    /// `api.github.com`: it takes writes with a token.
    GithubRead,
    /// `@pacman-mirrors`: the hosts of the pacman mirror list, which efrd reads from the
    /// files ([`parse_pacman`]).
    PacmanMirrors,
}

impl Preset {
    /// Every preset, in the order of the enum.
    pub const ALL: [Preset; 6] = [
        Preset::Crates,
        Preset::Npm,
        Preset::Pypi,
        Preset::Go,
        Preset::GithubRead,
        Preset::PacmanMirrors,
    ];

    /// The preset of a config word: `"@crates"` is [`Preset::Crates`].
    pub fn parse(word: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|preset| preset.name() == word)
    }

    /// The config word, such as `@github-read`.
    pub const fn name(self) -> &'static str {
        match self {
            Preset::Crates => "@crates",
            Preset::Npm => "@npm",
            Preset::Pypi => "@pypi",
            Preset::Go => "@go",
            Preset::GithubRead => "@github-read",
            Preset::PacmanMirrors => "@pacman-mirrors",
        }
    }

    /// The fixed hosts, on port 443; empty for [`Preset::PacmanMirrors`], whose hosts
    /// come from the files.
    pub const fn hosts(self) -> &'static [&'static str] {
        match self {
            Preset::Crates => &["crates.io", "index.crates.io", "static.crates.io"],
            Preset::Npm => &["registry.npmjs.org", "registry.yarnpkg.com"],
            Preset::Pypi => &["pypi.org", "files.pythonhosted.org"],
            Preset::Go => &["proxy.golang.org", "sum.golang.org"],
            Preset::GithubRead => &[
                "github.com",
                "codeload.github.com",
                "objects.githubusercontent.com",
                "raw.githubusercontent.com",
                "release-assets.githubusercontent.com",
            ],
            Preset::PacmanMirrors => &[],
        }
    }
}

/// An exact host, or every name below a suffix of at least two labels.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HostRule {
    /// One host in lower case with no trailing dot, or an IP address in its canonical
    /// form (only from `[[network.hosts]]`).
    Exact(String),
    /// Every name below this suffix, not the suffix itself: `*.example.com` is
    /// `Suffix("example.com")`, which matches `a.example.com` but not `example.com`.
    Suffix(String),
}

impl HostRule {
    /// True when the rule matches `host`, which must be in lower case with no trailing
    /// dot.
    pub fn matches(&self, host: &str) -> bool {
        match self {
            HostRule::Exact(exact) => exact == host,
            HostRule::Suffix(suffix) => is_below(host, suffix),
        }
    }
}

/// One `[[network.hosts]]` table, as the config holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostTable<'a> {
    /// The host, `*.suffix` or IP address.
    pub host: &'a str,
    /// The one port that the entry allows; absent means 443, and also 80 with `http`.
    pub port: Option<u16>,
    /// Plain HTTP `GET` and `HEAD` are allowed too.
    pub http: bool,
    /// The host may resolve to a private address, such as a mirror on the LAN (DP7).
    pub private: bool,
}

/// A host of a pacman mirror.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MirrorHost {
    /// The host, in lower case.
    pub host: String,
    /// The port of the mirror's URL: 443 for `https://` and 80 for `http://` unless the
    /// URL names one.
    pub port: u16,
    /// True for an `http://` mirror, which gets plain `GET` and `HEAD` only.
    pub http: bool,
}

/// What the host list says about one host and port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostVerdict {
    /// A rule allows the host on this port.
    Allowed {
        /// Plain HTTP `GET` and `HEAD` are allowed too.
        http: bool,
        /// The host may resolve to a private address (DP7).
        private: bool,
    },
    /// The deny list holds the host. Deny wins over every allow and grant.
    Denied,
    /// No rule holds the host on this port.
    NotListed,
}

/// `word` as a host and a port: in lower case, with no scheme, user info, path or
/// trailing dot; the word's port, else 80 for `http://`, else 443. An IPv6 address in
/// brackets gives its canonical form without the brackets.
///
/// The zone of a link-local address stays, as in `[fe80::1%eth0]`.
///
/// `None` for a name outside ASCII (the proxy compares the ASCII `xn--` form), a bad
/// label (empty, longer than 63 bytes, or a character other than `a-z`, `0-9`, `-` and
/// `_`), a name longer than 253 bytes, an empty name, and a port that is not 1 to 65535.
pub fn host_and_port(word: &str) -> Option<(String, u16)> {
    let word = word.trim();
    let (scheme, rest) = match word.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, word),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let (host, port, bracketed) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        let port = if after.is_empty() { None } else { Some(after.strip_prefix(':')?) };
        (host, port, true)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port), false),
            None => (authority, None, false),
        }
    };
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|port| *port != 0)?,
        None if scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case("http")) => 80,
        None => 443,
    };
    if !host.is_ascii() {
        return None;
    }
    if bracketed {
        let (address, zone) = match host.split_once('%') {
            Some((address, zone)) => (address, Some(zone)),
            None => (host, None),
        };
        let address: Ipv6Addr = address.parse().ok()?;
        return match zone {
            None => Some((address.to_string(), port)),
            Some(zone) if is_zone(zone) => Some((format!("{address}%{zone}"), port)),
            Some(_) => None,
        };
    }
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    is_name(host, true).then(|| (host.to_owned(), port))
}

/// True when getaddrinfo would read `host` as an address and not as a name: dotted or
/// bare numbers, hex or octal parts (`2130706433`, `127.1`, `0x7f.0.0.1`,
/// `0177.0.0.1`), or an IPv6 form, with or without brackets. A name whose last label is
/// all digits or starts with `0x` counts too, because no top-level domain looks so
/// (DP2).
pub fn is_ip_literal(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.starts_with('[') || host.contains(':') {
        return true;
    }
    let last = host.rsplit('.').next().unwrap_or(host);
    let digits = !last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit());
    let hex = last.get(..2).is_some_and(|prefix| prefix.eq_ignore_ascii_case("0x"));
    digits || hex
}

/// What a config entry of the host list holds.
enum Entry {
    Preset(Preset),
    Rule(HostRule),
}

/// Reads one config entry with the syntax of DP2. `ip` allows an IP address, which only
/// `[[network.hosts]]` may list; it must be in a form that the standard parser reads,
/// such as `192.168.1.5` or `[fd00::5]`.
fn entry(raw: &str, ip: bool) -> Result<Entry, HostError> {
    if !raw.is_ascii() {
        return Err(HostError::NotAscii(raw.to_owned()));
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with('@') {
        return Preset::parse(&lower)
            .map(Entry::Preset)
            .ok_or_else(|| HostError::UnknownPreset(raw.to_owned()));
    }
    let host = lower.strip_suffix('.').unwrap_or(&lower);
    if host == "*" {
        return Err(HostError::BareStar);
    }
    if let Some(suffix) = host.strip_prefix("*.") {
        if !suffix.contains('.') {
            return Err(HostError::ShortSuffix(raw.to_owned()));
        }
        if is_ip_literal(suffix) {
            return Err(HostError::IpLiteral(raw.to_owned()));
        }
        if !is_name(suffix, false) {
            return Err(HostError::BadName(raw.to_owned()));
        }
        return Ok(Entry::Rule(HostRule::Suffix(suffix.to_owned())));
    }
    if is_ip_literal(host) {
        if !ip {
            return Err(HostError::IpLiteral(raw.to_owned()));
        }
        let bare = host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')).unwrap_or(host);
        return match bare.parse::<IpAddr>() {
            Ok(address) => Ok(Entry::Rule(HostRule::Exact(address.to_string()))),
            Err(_) => Err(HostError::BadName(raw.to_owned())),
        };
    }
    if !is_name(host, false) {
        return Err(HostError::BadName(raw.to_owned()));
    }
    Ok(Entry::Rule(HostRule::Exact(host.to_owned())))
}

/// True for a host name of DP2: labels of `a-z`, `0-9` and `-` (and `_` with
/// `underscore`), 1 to 63 bytes each, and at most 253 bytes in all.
pub(crate) fn is_name(host: &str, underscore: bool) -> bool {
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && label.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || byte == b'-'
                    || (underscore && byte == b'_')
            })
    };
    !host.is_empty() && host.len() <= 253 && host.split('.').all(label)
}

/// True for the zone of an IPv6 address, an interface name such as `eth0` or `wlp3s0`.
fn is_zone(zone: &str) -> bool {
    (1..=15).contains(&zone.len())
        && zone
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// True when `host` is a name below `suffix`: `a.example.com` is below `example.com`,
/// `badexample.com` is not.
fn is_below(host: &str, suffix: &str) -> bool {
    host.strip_suffix(suffix)
        .and_then(|head| head.strip_suffix('.'))
        .is_some_and(|head| !head.is_empty())
}

#[cfg(test)]
mod tests;
