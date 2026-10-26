//! The stream of `GET /localapi/v0/watch-ipn-bus` (tailscale's `ipn.Notify`, one JSON
//! value per line).

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Deserializer};

use super::status::{is_tailnet_ipv4, null_is_empty};
use super::{TailscaleStatus, TailscaleVersion, unmapped};
use crate::{HttpError, JsonLines};

/// The open watch of the IPN bus.
///
/// tailscaled ends the stream when it stops, and also when this reader falls 128
/// messages behind: then the last message has an [`err_message`](IpnNotify::err_message)
/// ("IPN bus consumer fell behind") and the body ends. So the end of the stream does
/// not prove that tailscaled is down; open the watch again, then ask the status.
#[derive(Debug)]
pub struct IpnWatch {
    lines: JsonLines,
    version: Option<TailscaleVersion>,
}

impl IpnWatch {
    pub(crate) fn new(lines: JsonLines, version: Option<TailscaleVersion>) -> Self {
        IpnWatch { lines, version }
    }

    /// The version from the `Tailscale-Version` header of the watch's response.
    pub fn version(&self) -> Option<&TailscaleVersion> {
        self.version.as_ref()
    }

    /// The next message, or `None` when the stream ends. Cancel safe, like
    /// [`JsonLines::next_json`].
    pub async fn next(&mut self) -> Result<Option<IpnNotify>, HttpError> {
        let mut notify: Option<IpnNotify> = self.lines.next_json().await?;
        if let Some(status) = notify.as_mut().and_then(|notify| notify.initial_status.as_mut()) {
            status.version = TailscaleVersion::parse(&status.version_text);
        }
        Ok(notify)
    }
}

/// One message of the IPN bus. Only the fields that efrd reads are kept; every field is
/// absent from most messages.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct IpnNotify {
    /// The version of tailscaled; only the first message of a watch has it.
    #[serde(rename = "Version", default, deserialize_with = "empty_is_none")]
    pub version: Option<String>,
    /// An error from tailscaled. The stream ends after it.
    #[serde(rename = "ErrMessage", default, deserialize_with = "empty_is_none")]
    pub err_message: Option<String>,
    /// The new state of the backend.
    #[serde(rename = "State", default)]
    pub state: Option<IpnState>,
    /// This node as it is now, after control sent a new net map. It arrives on every
    /// full net map, also when nothing efrd reads has changed, so compare it with the
    /// last one before acting.
    #[serde(rename = "SelfChange", default)]
    pub self_change: Option<SelfChange>,
    /// The whole status, in the first message of a watch only (tailscaled 1.100 or
    /// later). Its version comes from its own `Version` field.
    #[serde(rename = "InitialStatus", default)]
    pub initial_status: Option<TailscaleStatus>,
}

impl IpnNotify {
    /// True when tailscaled ended the watch because this reader fell behind.
    pub fn fell_behind(&self) -> bool {
        self.err_message.as_deref().is_some_and(|text| text.contains("fell behind"))
    }
}

/// `SelfChange`: this node (a `tailcfg.Node`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct SelfChange {
    /// `StableID`.
    #[serde(rename = "StableID", default)]
    pub stable_id: String,
    /// `Name`, the MagicDNS name with a dot at its end.
    #[serde(rename = "Name", default)]
    pub name: String,
    /// `Addresses`, as prefixes such as `100.64.0.1/32`.
    #[serde(rename = "Addresses", default, deserialize_with = "null_is_empty")]
    pub addresses: Vec<String>,
}

impl SelfChange {
    /// The first IPv4 address in 100.64.0.0/10 among the node's addresses.
    pub fn tailnet_ipv4(&self) -> Option<Ipv4Addr> {
        self.addresses.iter().find_map(|prefix| {
            let address = prefix.split_once('/').map_or(prefix.as_str(), |(address, _)| address);
            match unmapped(address.parse::<IpAddr>().ok()?) {
                IpAddr::V4(v4) if is_tailnet_ipv4(v4) => Some(v4),
                _ => None,
            }
        })
    }
}

/// `State` of a message (tailscale's `ipn.State`, sent as a number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IpnState {
    /// 0: no state yet.
    NoState,
    /// 1: another user of this machine uses tailscaled.
    InUseOtherUser,
    /// 2: the node must log in.
    NeedsLogin,
    /// 3: an admin must approve the node.
    NeedsMachineAuth,
    /// 4: the user stopped Tailscale.
    Stopped,
    /// 5: the backend starts.
    Starting,
    /// 6: the backend runs.
    Running,
    /// A number this client does not know.
    Unknown(i64),
}

impl IpnState {
    /// The state of the number `value`.
    pub fn from_number(value: i64) -> IpnState {
        match value {
            0 => IpnState::NoState,
            1 => IpnState::InUseOtherUser,
            2 => IpnState::NeedsLogin,
            3 => IpnState::NeedsMachineAuth,
            4 => IpnState::Stopped,
            5 => IpnState::Starting,
            6 => IpnState::Running,
            other => IpnState::Unknown(other),
        }
    }
}

impl<'de> Deserialize<'de> for IpnState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        i64::deserialize(deserializer).map(IpnState::from_number)
    }
}

/// A missing, `null` or empty string read as `None`.
fn empty_is_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.filter(|text| !text.is_empty()))
}
