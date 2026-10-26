//! The reply of `GET /localapi/v0/whois` (tailscale's `apitype.WhoIsResponse`).

use serde::Deserialize;

use super::status::null_is_empty;
use crate::HttpError;

/// Who a tailnet peer is, as tailscaled's whois says. Only the fields that efrd checks
/// are kept: the numeric `Node.ID` is left out on purpose, because ids of old nodes can
/// pass 2^53; the stable id is the one to pin.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PeerIdentity {
    /// `Node.StableID`, never empty.
    pub node_stable_id: String,
    /// `Node.Name`: the MagicDNS name of the node, with a dot at its end.
    pub node_name: String,
    /// `Node.Hostinfo.Hostname`, the host name that the node reports; empty when it
    /// sends none.
    pub host_name: String,
    /// `Node.Tags`. A tagged node belongs to no person: its user is the synthetic
    /// "tagged-devices" user.
    pub tags: Vec<String>,
    /// `Node.Hostinfo.ShareeNode`: true when the node is shared into this tailnet from
    /// another one.
    pub sharee_node: bool,
    /// `UserProfile.ID`.
    pub user_id: i64,
    /// `UserProfile.LoginName`, for display only.
    pub login_name: String,
    /// `UserProfile.DisplayName`, for display only.
    pub display_name: String,
}

impl PeerIdentity {
    /// True when the node has tags or is a sharee node: a node that does not belong to
    /// a person of this tailnet.
    pub fn is_tagged_or_shared(&self) -> bool {
        !self.tags.is_empty() || self.sharee_node
    }
}

/// The reply as tailscaled sends it. `Node` and `UserProfile` are required, so a reply
/// without them fails to parse.
#[derive(Debug, Deserialize)]
pub(crate) struct WhoIsReply {
    #[serde(rename = "Node")]
    node: WireNode,
    #[serde(rename = "UserProfile")]
    user: WireUser,
}

#[derive(Debug, Deserialize)]
struct WireNode {
    #[serde(rename = "StableID", default)]
    stable_id: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Tags", default, deserialize_with = "null_is_empty")]
    tags: Vec<String>,
    #[serde(rename = "Hostinfo", default)]
    hostinfo: Option<WireHostinfo>,
}

#[derive(Debug, Default, Deserialize)]
struct WireHostinfo {
    #[serde(rename = "Hostname", default)]
    hostname: String,
    #[serde(rename = "ShareeNode", default)]
    sharee_node: bool,
}

#[derive(Debug, Deserialize)]
struct WireUser {
    #[serde(rename = "ID")]
    id: i64,
    #[serde(rename = "LoginName", default)]
    login_name: String,
    #[serde(rename = "DisplayName", default)]
    display_name: String,
}

impl WhoIsReply {
    /// The identity, or [`HttpError::LocalApiReply`] when the node has no stable id.
    pub(crate) fn identity(self, url: String) -> Result<PeerIdentity, HttpError> {
        let WhoIsReply { node, user } = self;
        if node.stable_id.trim().is_empty() {
            return Err(HttpError::LocalApiReply { url, problem: "the node has no stable id" });
        }
        let hostinfo = node.hostinfo.unwrap_or_default();
        Ok(PeerIdentity {
            node_stable_id: node.stable_id,
            node_name: node.name,
            host_name: hostinfo.hostname,
            tags: node.tags,
            sharee_node: hostinfo.sharee_node,
            user_id: user.id,
            login_name: user.login_name,
            display_name: user.display_name,
        })
    }
}
