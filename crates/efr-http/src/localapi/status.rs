//! The reply of `GET /localapi/v0/status` (tailscale's `ipnstate.Status`).

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Deserializer};

use super::{TailscaleVersion, unmapped};

/// The state of this node, as `status?peers=false` gives it. Only the fields that
/// efrd checks are kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct TailscaleStatus {
    /// The state of the backend; efrd binds only while it is
    /// [`Running`](BackendState::Running).
    #[serde(rename = "BackendState", default)]
    pub backend_state: BackendState,
    /// `true` when tailscaled runs a kernel TUN device. In userspace networking mode it
    /// is `false`, and inbound tailnet connections arrive on 127.0.0.1, so a listener on
    /// the tailnet address cannot exist.
    #[serde(rename = "TUN", default)]
    pub tun: bool,
    /// The tailnet addresses of this node, IPv4 and IPv6.
    #[serde(rename = "TailscaleIPs", default, deserialize_with = "null_is_empty")]
    pub tailscale_ips: Vec<IpAddr>,
    /// This node, when tailscaled has one.
    #[serde(rename = "Self", default)]
    pub self_node: Option<StatusNode>,
    /// The tailnet this node is in, when tailscaled has one.
    #[serde(rename = "CurrentTailnet", default)]
    pub current_tailnet: Option<CurrentTailnet>,
    /// The version of tailscaled: from the `Tailscale-Version` header, else from the
    /// body's `Version`.
    #[serde(skip)]
    pub version: Option<TailscaleVersion>,
    /// The body's `Version`, as sent.
    #[serde(rename = "Version", default)]
    pub(crate) version_text: String,
}

impl TailscaleStatus {
    /// The first IPv4 address of this node in the tailnet range 100.64.0.0/10.
    pub fn tailnet_ipv4(&self) -> Option<Ipv4Addr> {
        self.tailscale_ips.iter().find_map(|ip| match unmapped(*ip) {
            IpAddr::V4(v4) if is_tailnet_ipv4(v4) => Some(v4),
            _ => None,
        })
    }

    /// True when the backend runs with a kernel TUN device: the only state in which a
    /// listener on the tailnet address can work.
    pub fn is_running_with_tun(&self) -> bool {
        self.backend_state == BackendState::Running && self.tun
    }

    /// True when `ip` is one of this node's own tailnet addresses. A connection from one
    /// of them comes from a process on this machine.
    pub fn is_own_ip(&self, ip: IpAddr) -> bool {
        let ip = unmapped(ip);
        self.tailscale_ips.iter().any(|own| unmapped(*own) == ip)
    }

    /// The stable id of this node (`Self.ID`), when tailscaled has a node.
    pub fn self_stable_id(&self) -> Option<&str> {
        self.self_node.as_ref().map(|node| node.stable_id.as_str()).filter(|id| !id.is_empty())
    }

    /// The stable id of the tailnet (`CurrentTailnet.StableID`), when tailscaled names
    /// one.
    pub fn tailnet_stable_id(&self) -> Option<&str> {
        self.current_tailnet
            .as_ref()
            .map(|tailnet| tailnet.stable_id.as_str())
            .filter(|id| !id.is_empty())
    }
}

/// True when `ip` is in the tailnet range 100.64.0.0/10.
pub(crate) fn is_tailnet_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 100 && (second & 0b1100_0000) == 64
}

/// `Self` of the status: this node.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct StatusNode {
    /// The stable node id (`ID`, a `StableNodeID` string such as `nXXXXCNTRL`).
    #[serde(rename = "ID", default)]
    pub stable_id: String,
    /// The host name.
    #[serde(rename = "HostName", default)]
    pub host_name: String,
    /// The MagicDNS name, with a dot at its end.
    #[serde(rename = "DNSName", default)]
    pub dns_name: String,
    /// The id of the user that owns the node.
    #[serde(rename = "UserID", default)]
    pub user_id: i64,
    /// The tags of the node; empty for a node that a user owns.
    #[serde(rename = "Tags", default, deserialize_with = "null_is_empty")]
    pub tags: Vec<String>,
}

/// `CurrentTailnet` of the status.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct CurrentTailnet {
    /// The name of the tailnet, for display only.
    #[serde(rename = "Name", default)]
    pub name: String,
    /// The stable id of the tailnet (`StableID`); empty when tailscaled sends none.
    #[serde(rename = "StableID", default)]
    pub stable_id: String,
    /// The MagicDNS suffix, such as `example.ts.net`.
    #[serde(rename = "MagicDNSSuffix", default)]
    pub magic_dns_suffix: String,
    /// True when MagicDNS is on.
    #[serde(rename = "MagicDNSEnabled", default)]
    pub magic_dns_enabled: bool,
}

/// `BackendState` of the status, sent as a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[non_exhaustive]
pub enum BackendState {
    /// No state yet.
    #[default]
    NoState,
    /// Another user of this machine uses tailscaled.
    InUseOtherUser,
    /// The node must log in.
    NeedsLogin,
    /// An admin must approve the node.
    NeedsMachineAuth,
    /// The user stopped Tailscale.
    Stopped,
    /// The backend starts.
    Starting,
    /// The backend runs.
    Running,
    /// A state this client does not know.
    #[serde(other)]
    Unknown,
}

/// A JSON `null` read as an empty list (Go sends `null` for a nil slice).
pub(crate) fn null_is_empty<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}
