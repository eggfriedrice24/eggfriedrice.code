//! [`HostList`]: the compiled allow and deny lists of the proxy.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use super::{
    Entry, HostError, HostRule, HostTable, HostVerdict, MirrorHost, Preset, entry, is_below,
    is_ip_literal, is_name,
};

/// What one allow entry opens for its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Opening {
    /// The one port; `None` means 443, and also 80 with `http`.
    port: Option<u16>,
    http: bool,
    private: bool,
}

impl Opening {
    const PLAIN: Opening = Opening { port: None, http: false, private: false };

    fn covers(self, port: u16) -> bool {
        match self.port {
            Some(own) => own == port,
            None => port == 443 || (self.http && port == 80),
        }
    }
}

/// The allow and deny lists of the proxy, compiled from `network.allow`,
/// `network.deny`, `[[network.hosts]]` and the pacman mirror hosts.
///
/// Every part is sorted, so `==` compares the content: two configs that say the same in
/// another order compile to equal lists, and a reload can tell whether the list changed.
/// The default list is empty: it allows nothing and denies nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostList {
    exact: BTreeMap<String, BTreeSet<Opening>>,
    suffix: BTreeSet<(String, Opening)>,
    deny_exact: BTreeSet<String>,
    deny_suffix: BTreeSet<String>,
}

impl HostList {
    /// The list of these entries. `allow` holds presets and hosts on port 443;
    /// `hosts` the `[[network.hosts]]` tables, the only place for an IP address;
    /// `deny` presets and hosts that no allow and no host grant may reach; `mirrors` the
    /// hosts that efrd read from the pacman files, which `@pacman-mirrors` in `allow` or
    /// `deny` stands for.
    pub fn compile(
        allow: &[String],
        deny: &[String],
        hosts: &[HostTable<'_>],
        mirrors: &[MirrorHost],
    ) -> Result<HostList, HostError> {
        let mut list = HostList::default();
        let mirrors: Vec<&MirrorHost> = mirrors.iter().filter(|mirror| valid(mirror)).collect();
        for raw in allow {
            match entry(raw, false)? {
                Entry::Preset(Preset::PacmanMirrors) => {
                    for mirror in &mirrors {
                        let opening =
                            Opening { port: Some(mirror.port), http: mirror.http, private: false };
                        list.allow(HostRule::Exact(mirror.host.clone()), opening);
                    }
                }
                Entry::Preset(preset) => {
                    for host in preset.hosts() {
                        list.allow(HostRule::Exact((*host).to_owned()), Opening::PLAIN);
                    }
                }
                Entry::Rule(rule) => list.allow(rule, Opening::PLAIN),
            }
        }
        for table in hosts {
            if table.port == Some(0) {
                return Err(HostError::BadPort(table.host.to_owned()));
            }
            let opening = Opening { port: table.port, http: table.http, private: table.private };
            match entry(table.host, true)? {
                Entry::Preset(_) => return Err(HostError::BadName(table.host.to_owned())),
                Entry::Rule(rule) => list.allow(rule, opening),
            }
        }
        for raw in deny {
            match entry(raw, false)? {
                Entry::Preset(Preset::PacmanMirrors) => {
                    list.deny_exact.extend(mirrors.iter().map(|mirror| mirror.host.clone()));
                }
                Entry::Preset(preset) => {
                    list.deny_exact.extend(preset.hosts().iter().map(|host| (*host).to_owned()));
                }
                Entry::Rule(HostRule::Exact(host)) => {
                    list.deny_exact.insert(host);
                }
                Entry::Rule(HostRule::Suffix(suffix)) => {
                    list.deny_suffix.insert(suffix);
                }
            }
        }
        Ok(list)
    }

    /// The verdict for `host:port`. A plain entry allows 443; an `http` entry also 80;
    /// an entry with a `port` only that port. The host is compared in lower case with
    /// no trailing dot, and an IP address in its canonical form.
    pub fn decide(&self, host: &str, port: u16) -> HostVerdict {
        let host = canonical(host);
        if self.denies(&host) {
            return HostVerdict::Denied;
        }
        let exact = self.exact.get(host.as_ref()).into_iter().flatten();
        let below = self
            .suffix
            .iter()
            .filter(|(suffix, _)| is_below(&host, suffix))
            .map(|(_, opening)| opening);
        let mut verdict = HostVerdict::NotListed;
        for opening in exact.chain(below).filter(|opening| opening.covers(port)) {
            verdict = match verdict {
                HostVerdict::Allowed { http, private } => HostVerdict::Allowed {
                    http: http || opening.http,
                    private: private || opening.private,
                },
                _ => HostVerdict::Allowed { http: opening.http, private: opening.private },
            };
        }
        verdict
    }

    /// True when [`decide`](Self::decide) gives `Allowed` for `host:port`. The engine
    /// sets `on_allow_list` from it only under DP23's conditions (the proxy egress, a
    /// `Grant::Host` exit, the exit's own port).
    pub fn allows(&self, host: &str, port: u16) -> bool {
        matches!(self.decide(host, port), HostVerdict::Allowed { .. })
    }

    /// True when the deny list holds `host`, on any port.
    pub fn denies(&self, host: &str) -> bool {
        let host = canonical(host);
        self.deny_exact.contains(host.as_ref())
            || self.deny_suffix.iter().any(|suffix| is_below(&host, suffix))
    }

    fn allow(&mut self, rule: HostRule, opening: Opening) {
        match rule {
            HostRule::Exact(host) => {
                self.exact.entry(host).or_default().insert(opening);
            }
            HostRule::Suffix(suffix) => {
                self.suffix.insert((suffix, opening));
            }
        }
    }
}

/// A mirror host that the list can hold: a name, not an address, and a real port.
fn valid(mirror: &MirrorHost) -> bool {
    mirror.port != 0 && !is_ip_literal(&mirror.host) && is_name(&mirror.host, false)
}

/// `host` as the list compares it: lower case, no trailing dot, and an IP address in its
/// canonical form without brackets.
fn canonical(host: &str) -> Cow<'_, str> {
    let host = host.strip_suffix('.').unwrap_or(host);
    let bare = host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')).unwrap_or(host);
    if let Ok(address) = bare.parse::<IpAddr>() {
        return Cow::Owned(address.to_string());
    }
    if host.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(host.to_ascii_lowercase())
    } else {
        Cow::Borrowed(host)
    }
}
