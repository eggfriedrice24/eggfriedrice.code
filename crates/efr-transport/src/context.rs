//! Who is on the other end of a connection.
//!
//! The daemon closes every handler over the connection's context instead of a global
//! "current client", so attribution and per-surface behaviour need no shared state.
//!
//! A peer is local or remote. A local peer came through the Unix socket, and the kernel
//! names its uid and pid. A remote peer came through the WebSocket listener: the
//! transport knows only its socket address, and the daemon, which admitted it, keeps
//! what it learned about the peer in an opaque admission value.

use std::any::Any;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use efr_protocol::Origin;

/// The next connection id of this process. Every listener takes its ids from here, so
/// two listeners never give the same id.
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// The id of one connection, unique for the life of the daemon process. Log spans and
/// the daemon's per-connection state key on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(u64);

impl ConnId {
    /// The connection id `value`.
    pub const fn new(value: u64) -> Self {
        ConnId(value)
    }

    /// A connection id that no connection of this process had before. The Unix
    /// listener and the WebSocket entry take their ids from here.
    pub fn next() -> Self {
        ConnId(NEXT_CONN.fetch_add(1, Ordering::Relaxed))
    }

    /// The number inside.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The credentials that the kernel reports for a connected peer (`SO_PEERCRED`).
///
/// The kernel records them when the peer connects, so they cannot be forged by what
/// the peer later sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerCred {
    uid: u32,
    pid: Option<u32>,
}

impl PeerCred {
    /// Credentials with the given uid and pid.
    pub const fn new(uid: u32, pid: Option<u32>) -> Self {
        PeerCred { uid, pid }
    }

    /// The peer's user id.
    pub const fn uid(&self) -> u32 {
        self.uid
    }

    /// The peer's process id at connect time, when the kernel reported one.
    pub const fn pid(&self) -> Option<u32> {
        self.pid
    }
}

/// A peer that came through the WebSocket listener.
///
/// The transport reads only the socket address. The admission value belongs to the
/// daemon: it fills it when it admits the connection (for example the peer's node and
/// user, and later the device that proved its key) and reads it back from every request
/// with [`RemotePeer::admission`]. A value that must change after admission, such as the
/// device id after a proof, needs interior mutability of its own, because every request
/// gets a clone of the context and all clones share one value.
#[derive(Clone)]
pub struct RemotePeer {
    addr: SocketAddr,
    admission: Arc<dyn Any + Send + Sync>,
}

impl RemotePeer {
    /// A remote peer at `addr` with the daemon's admission value `admission`.
    pub fn new(addr: SocketAddr, admission: Arc<dyn Any + Send + Sync>) -> Self {
        RemotePeer { addr, admission }
    }

    /// The peer's socket address, as `accept` reported it.
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The admission value, when it is a `T`.
    pub fn admission<T: Any>(&self) -> Option<&T> {
        self.admission.downcast_ref::<T>()
    }
}

impl fmt::Debug for RemotePeer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: the admission value is the daemon's and can hold more than a log line
        // should show, so only the address is printed.
        f.debug_struct("RemotePeer").field("addr", &self.addr).finish_non_exhaustive()
    }
}

impl PartialEq for RemotePeer {
    fn eq(&self, other: &Self) -> bool {
        self.addr == other.addr && Arc::ptr_eq(&self.admission, &other.admission)
    }
}

impl Eq for RemotePeer {}

/// The other end of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Peer {
    /// A process on this machine, connected through the Unix socket.
    Local(PeerCred),
    /// A device connected through the WebSocket listener.
    Remote(RemotePeer),
}

impl From<PeerCred> for Peer {
    fn from(cred: PeerCred) -> Self {
        Peer::Local(cred)
    }
}

impl From<RemotePeer> for Peer {
    fn from(remote: RemotePeer) -> Self {
        Peer::Remote(remote)
    }
}

impl Peer {
    /// The peer's user id from the kernel, for a local peer.
    pub const fn uid(&self) -> Option<u32> {
        match self {
            Peer::Local(cred) => Some(cred.uid()),
            Peer::Remote(_) => None,
        }
    }

    /// The peer's process id at connect time, for a local peer whose pid the kernel
    /// reported.
    pub const fn pid(&self) -> Option<u32> {
        match self {
            Peer::Local(cred) => cred.pid(),
            Peer::Remote(_) => None,
        }
    }

    /// The remote peer, when the connection came through the WebSocket listener.
    pub const fn remote(&self) -> Option<&RemotePeer> {
        match self {
            Peer::Remote(remote) => Some(remote),
            Peer::Local(_) => None,
        }
    }
}

/// The per-connection context that every request of a connection carries:
/// `{ surface, peer, conn_id }`.
///
/// It exists from a successful `hello` on, because the surface is what the client
/// declares there. A local peer's uid and pid come from the kernel, not from the
/// client. A remote peer's surface is always [`Origin::Phone`], whatever its hello
/// declared, because only the phone app connects through the WebSocket listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionContext {
    conn_id: ConnId,
    surface: Origin,
    peer: Peer,
}

impl ConnectionContext {
    /// The context of connection `conn_id` from `peer` that declared itself as
    /// `surface`. A remote peer gets the surface [`Origin::Phone`] instead.
    pub fn new(conn_id: ConnId, surface: Origin, peer: impl Into<Peer>) -> Self {
        let peer = peer.into();
        let surface = match peer {
            Peer::Local(_) => surface,
            Peer::Remote(_) => Origin::Phone,
        };
        ConnectionContext { conn_id, surface, peer }
    }

    /// The connection.
    pub const fn conn_id(&self) -> ConnId {
        self.conn_id
    }

    /// The kind of client: the zsh plugin, `efr`, the PTY proxy or a phone.
    pub const fn surface(&self) -> Origin {
        self.surface
    }

    /// The peer's user id from the kernel, for a local peer.
    pub const fn uid(&self) -> Option<u32> {
        self.peer.uid()
    }

    /// The peer's process id at connect time from the kernel, for a local peer.
    pub const fn pid(&self) -> Option<u32> {
        self.peer.pid()
    }

    /// The other end of the connection.
    pub const fn peer(&self) -> &Peer {
        &self.peer
    }

    /// The remote peer, when the connection came through the WebSocket listener.
    pub const fn remote(&self) -> Option<&RemotePeer> {
        self.peer.remote()
    }
}

#[cfg(test)]
mod tests;
