//! Who is on the other end of a connection.
//!
//! The daemon closes every handler over the connection's context instead of a global
//! "current client", so attribution and per-surface behaviour need no shared state.

use std::fmt;

use efr_protocol::Origin;

/// The id of one connection, unique for the life of the daemon process. Log spans and
/// the daemon's per-connection state key on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(u64);

impl ConnId {
    /// The connection id `value`.
    pub const fn new(value: u64) -> Self {
        ConnId(value)
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

/// The per-connection context that every request of a connection carries:
/// `{ surface, uid, pid, conn_id }`.
///
/// It exists from a successful `hello` on, because the surface is what the client
/// declares there. The uid and pid come from the kernel, not from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionContext {
    conn_id: ConnId,
    surface: Origin,
    peer: PeerCred,
}

impl ConnectionContext {
    /// The context of connection `conn_id` from a peer with credentials `peer` that
    /// declared itself as `surface`.
    pub const fn new(conn_id: ConnId, surface: Origin, peer: PeerCred) -> Self {
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

    /// The peer's user id, from the kernel.
    pub const fn uid(&self) -> u32 {
        self.peer.uid()
    }

    /// The peer's process id at connect time, from the kernel.
    pub const fn pid(&self) -> Option<u32> {
        self.peer.pid()
    }

    /// The peer's credentials.
    pub const fn peer(&self) -> PeerCred {
        self.peer
    }
}
