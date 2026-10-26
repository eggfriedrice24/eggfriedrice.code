//! Who may open a connection on the remote listener, and what efrd keeps about it.
//!
//! [`decide`] runs right after `accept`, before any byte of HTTP is read. It fails
//! closed: a whois that fails or that names no node refuses the peer. The rules
//! (Tailscale's whois resolves an address of this node to this node, so a local
//! process, an agent's tool among them, that dials 100.x would pass as the owner's own
//! machine):
//!
//! - an address of this node refuses at once, with no whois;
//! - the self node, a tagged node and a sharee node are refused;
//! - a node of another user is refused, and so is every node when this node is tagged,
//!   because a tagged node belongs to no person.
//!
//! An [`Admission`] is the opaque value of the transport's remote peer: the facts of the
//! whois, this node's and tailnet's ids, the token that closes the connection, the
//! challenge of the hello and, after `device.prove`, the device and its scopes.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use efr_http::PeerIdentity;
use efr_protocol::{DeviceId, ProveContext, ScopeName};
use tokio_util::sync::CancellationToken;

use super::tailnet::Tailnet;

/// Why a peer was refused before its handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The peer has an address of this node: a local process.
    OwnAddress,
    /// The whois failed.
    WhoisFailed,
    /// tailscaled knows no node at the address.
    UnknownPeer,
    /// The whois named this node.
    SelfNode,
    /// The node has tags, so it belongs to no person.
    Tagged,
    /// The node is shared in from another tailnet.
    Shared,
    /// The node belongs to another user.
    OtherUser,
    /// This node is tagged, so no peer can be of its user.
    TaggedDaemon,
}

impl Refusal {
    /// The reason for the log.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Refusal::OwnAddress => "an address of this node",
            Refusal::WhoisFailed => "the whois of tailscaled failed",
            Refusal::UnknownPeer => "tailscaled knows no node there",
            Refusal::SelfNode => "this node itself",
            Refusal::Tagged => "a tagged node",
            Refusal::Shared => "a node shared from another tailnet",
            Refusal::OtherUser => "a node of another user",
            Refusal::TaggedDaemon => "this node is tagged, so no peer is its user's",
        }
    }
}

/// Whether to refuse `peer` at once, before its whois: a peer at an address of this
/// node is a local process.
pub(crate) fn refuse_before_whois(tailnet: &Tailnet, peer: SocketAddr) -> Option<Refusal> {
    let ip = unmapped(peer.ip());
    let own = tailnet.own_ips.iter().any(|own| unmapped(*own) == ip);
    own.then_some(Refusal::OwnAddress)
}

/// What efrd reads of a whois reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Whois {
    pub(crate) node_stable_id: String,
    pub(crate) node_name: String,
    pub(crate) host_name: String,
    pub(crate) tags: Vec<String>,
    pub(crate) sharee_node: bool,
    pub(crate) user_id: i64,
    pub(crate) login_name: String,
    pub(crate) display_name: String,
}

impl From<PeerIdentity> for Whois {
    fn from(identity: PeerIdentity) -> Self {
        Whois {
            node_stable_id: identity.node_stable_id,
            node_name: identity.node_name,
            host_name: identity.host_name,
            tags: identity.tags,
            sharee_node: identity.sharee_node,
            user_id: identity.user_id,
            login_name: identity.login_name,
            display_name: identity.display_name,
        }
    }
}

/// The facts of a peer that its whois `identity` admits, on `tailnet`.
pub(crate) fn decide(tailnet: &Tailnet, identity: Option<Whois>) -> Result<PeerFacts, Refusal> {
    let identity = identity.ok_or(Refusal::UnknownPeer)?;
    if identity.node_stable_id == tailnet.self_node {
        return Err(Refusal::SelfNode);
    }
    if !identity.tags.is_empty() {
        return Err(Refusal::Tagged);
    }
    if identity.sharee_node {
        return Err(Refusal::Shared);
    }
    let Some(owner) = tailnet.self_user else {
        return Err(Refusal::TaggedDaemon);
    };
    if identity.user_id != owner {
        return Err(Refusal::OtherUser);
    }
    Ok(PeerFacts {
        node_stable_id: identity.node_stable_id,
        node_name: identity.node_name.trim_end_matches('.').to_owned(),
        host_name: identity.host_name,
        user_id: identity.user_id,
        login_name: identity.login_name,
        display_name: identity.display_name,
    })
}

/// `ip` with an IPv4 address mapped into IPv6 turned back into IPv4.
fn unmapped(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => {
            v6.to_ipv4_mapped().map_or(std::net::IpAddr::V6(v6), std::net::IpAddr::V4)
        }
        std::net::IpAddr::V4(_) => ip,
    }
}

/// What the whois said about an admitted peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeerFacts {
    /// `Node.StableID`, which a device row pins.
    pub(crate) node_stable_id: String,
    /// `Node.Name`, the MagicDNS name, without the dot at its end; for display only.
    pub(crate) node_name: String,
    /// The host name that the node reports; for display only.
    pub(crate) host_name: String,
    /// `UserProfile.ID`, which a device row pins.
    pub(crate) user_id: i64,
    /// `UserProfile.LoginName`; for display only.
    pub(crate) login_name: String,
    /// `UserProfile.DisplayName`; for display only.
    pub(crate) display_name: String,
}

/// What the hello of the connection gave.
#[derive(Debug, Clone)]
struct HelloFacts {
    challenge: String,
    device_id: Option<DeviceId>,
}

/// A device that proved its key on the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proved {
    pub(crate) device_id: DeviceId,
    /// The scopes of the connection: the device's, never `admin` or `terminal`.
    pub(crate) scopes: Vec<ScopeName>,
}

/// The daemon's value for one admitted remote connection.
#[derive(Debug)]
pub(crate) struct Admission {
    /// The key of the connection in the table of live devices.
    key: u64,
    peer: PeerFacts,
    daemon_node: String,
    tailnet: String,
    /// Cancelled to close the connection: a revoke, too many failed proofs, a rebind.
    close: CancellationToken,
    hello: Mutex<Option<HelloFacts>>,
    proved: OnceLock<Proved>,
    failures: AtomicU32,
}

impl Admission {
    pub(crate) fn new(
        key: u64,
        peer: PeerFacts,
        tailnet: &Tailnet,
        close: CancellationToken,
    ) -> Self {
        Admission {
            key,
            peer,
            daemon_node: tailnet.self_node.clone(),
            tailnet: tailnet.tailnet.clone(),
            close,
            hello: Mutex::new(None),
            proved: OnceLock::new(),
            failures: AtomicU32::new(0),
        }
    }

    pub(crate) fn key(&self) -> u64 {
        self.key
    }

    /// The facts of the whois of the peer.
    pub(crate) fn peer(&self) -> &PeerFacts {
        &self.peer
    }

    /// The stable id of efrd's tailnet when the connection was admitted.
    pub(crate) fn tailnet(&self) -> &str {
        &self.tailnet
    }

    /// What the hello result sends for the proof.
    pub(crate) fn prove_context(&self) -> ProveContext {
        ProveContext {
            peer_node: self.peer.node_stable_id.clone(),
            daemon_node: self.daemon_node.clone(),
            daemon_tailnet: self.tailnet.clone(),
        }
    }

    /// The token that closes the connection.
    pub(crate) fn close_token(&self) -> &CancellationToken {
        &self.close
    }

    /// Keeps the hello's challenge and the device that the hello named.
    pub(crate) fn hello(&self, challenge: &str, device_id: Option<DeviceId>) {
        *self.hello.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(HelloFacts { challenge: challenge.to_owned(), device_id });
    }

    /// The hello's challenge and device, while the connection has not proved a device.
    pub(crate) fn challenge(&self) -> Option<(String, Option<DeviceId>)> {
        let hello = self.hello.lock().unwrap_or_else(PoisonError::into_inner);
        hello.as_ref().map(|hello| (hello.challenge.clone(), hello.device_id))
    }

    /// Records the proof; false when the connection had proved a device before. The
    /// challenge is spent with it.
    pub(crate) fn prove(&self, proved: Proved) -> bool {
        let set = self.proved.set(proved).is_ok();
        if set {
            *self.hello.lock().unwrap_or_else(PoisonError::into_inner) = None;
        }
        set
    }

    /// The device that proved its key on this connection.
    pub(crate) fn proved(&self) -> Option<&Proved> {
        self.proved.get()
    }

    /// Counts one failed proof and returns the count so far.
    pub(crate) fn fail(&self) -> u32 {
        self.failures.fetch_add(1, Ordering::AcqRel).saturating_add(1)
    }
}

#[cfg(test)]
mod tests;
