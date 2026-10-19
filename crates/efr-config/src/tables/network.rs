//! `[network]`: what network a contained call of the `auto` mode sees (efr's auto
//! spec, section 8.8 for phase D1, sections 8.2 to 8.5 for the proxy of phase D2).

use efr_permissions::{HostError, HostList, HostTable, MirrorHost, Preset};
use efr_protocol::NetworkMode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[network]`: the network of the `auto` sandbox. Every key applies from the next
/// call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct NetworkSettings {
    /// `true`: in `auto`, `ss`, the read forms of `ip`, `nstat` and reads of `/proc/net`
    /// run in the sandbox with your machine's network view and no network socket, with
    /// no question. `false`: they go to the reviewer, as a look at the host's network.
    pub netview: bool,
    /// `"proxy"` (the default): contained commands reach the hosts of `allow` through
    /// efr's proxy, and a network need is no review. `"none"`: no network; every network
    /// need is a review that opens the whole network for one call. A change applies to new calls;
    /// a running background job keeps its network until it ends.
    pub mode: NetworkMode,
    /// The hosts that contained commands reach through the proxy on port 443: presets
    /// (`@crates`, `@npm`, `@pypi`, `@go`, `@github-read`, `@pacman-mirrors`), host
    /// names, and `*.suffix` for the names below a suffix of at least two labels. Write
    /// a name in its ASCII (xn--) form. An IP address goes in `[[network.hosts]]`.
    pub allow: Vec<String>,
    /// Presets, hosts and `*.suffix` entries that no contained command reaches. The
    /// deny list wins over `allow`, `[[network.hosts]]` and every host grant.
    pub deny: Vec<String>,
    /// More allowed hosts, each a table: `host` (a name, `*.suffix` or an IP address),
    /// `port` (the one port that the entry allows; absent: 443, and also 80 with
    /// `http`), `http` (plain HTTP GET and HEAD too, such as for a mirror) and `private`
    /// (the host may resolve to an address of your LAN). Only an editor changes them.
    pub hosts: Vec<NetworkHost>,
    /// The bytes that a contained call may send through the proxy, counted from the
    /// first byte after the CONNECT head; 65536 to 1073741824. A background job counts
    /// for its whole life.
    pub max_upload_bytes: u64,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        NetworkSettings {
            netview: true,
            mode: NetworkMode::Proxy,
            allow: Preset::ALL.iter().map(|preset| preset.name().to_owned()).collect(),
            deny: Vec::new(),
            hosts: Vec::new(),
            max_upload_bytes: 4_194_304,
        }
    }
}

impl NetworkSettings {
    /// The compiled host list of `allow`, `deny` and `hosts`, with these mirror hosts
    /// for `@pacman-mirrors`. efrd reads the pacman files; the config check gives none.
    pub fn host_list(&self, mirrors: &[MirrorHost]) -> Result<HostList, HostError> {
        let tables: Vec<HostTable<'_>> = self.hosts.iter().map(NetworkHost::table).collect();
        HostList::compile(&self.allow, &self.deny, &tables, mirrors)
    }
}

/// One `[[network.hosts]]` table: a host that contained commands may reach through the
/// proxy, with its options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkHost {
    /// The host: a name, `*.suffix`, or an IP address such as `192.168.1.5`.
    pub host: String,
    /// The one port that the entry allows. Absent: 443, and also 80 with `http`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Plain HTTP GET and HEAD are allowed too, such as for a mirror.
    #[serde(default)]
    pub http: bool,
    /// The host may resolve to a private address, such as a mirror on your LAN; never
    /// to a loopback, link-local or own address.
    #[serde(default)]
    pub private: bool,
}

impl NetworkHost {
    /// The entry as the host list reads it.
    pub(crate) fn table(&self) -> HostTable<'_> {
        HostTable { host: &self.host, port: self.port, http: self.http, private: self.private }
    }
}
