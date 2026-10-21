//! The pure rules of job port forwards: which loopback port of a contained job efr
//! forwards to the same port of the host's `127.0.0.1`, and the frames between the
//! forwarder in the job (`efr-sbx forward`) and efrd.
//!
//! The forwarder reads the job's own `/proc/self/net/tcp` and `tcp6` at each poll and
//! gives the text to [`parse_tcp_table`], [`loopback_listens`] and [`Debounce`]. efrd
//! reads the host's tables with the same parse for [`peer_uid`] and [`v6_holds`], and
//! decodes every frame with [`Frame::decode`], which trusts no byte: the forwarder runs
//! in a place that the job may control. efrd checks each report again with
//! [`refusal`]. No IO, no clock: the caller passes the text and the time.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::ops::RangeInclusive;

use crate::SandboxError;
use crate::plan::PROXY_PORT;
use crate::spec::ForwardPlan;

/// The lowest port that efr forwards. A new network namespace keeps
/// `ip_unprivileged_port_start` at 1024, so a job cannot bind a lower one, and efrd
/// could not bind it on the host.
pub const MIN_PORT: u16 = 1024;

/// The port of the bridge in a proxy job. A forward of it would give host processes
/// the job's proxy grants.
pub const BRIDGE_PORT: u16 = PROXY_PORT;

/// The ports that efr never forwards (OQL1): ports where a client sends a secret first.
/// efr's own login callback (1455), databases, caches, brokers, container, cluster and
/// secret services, a local model server, Syncthing's web UI and the proxy ports.
/// Sorted.
pub const SERVICE_PORTS: &[u16] = &[
    1080, 1433, 1455, 1521, 2181, 2375, 2376, 2379, 3128, 3306, 4222, 5432, 5672, 5984, 6379, 6443,
    7474, 7687, 8086, 8118, 8200, 8384, 9042, 9050, 9092, 9150, 9200, 10250, 11211, 11434, 27017,
];

/// The most ports that efr forwards for one job.
pub const MAX_JOB_FORWARDS: u8 = 8;

/// The most ports that one efrd forwards for all its jobs.
pub const MAX_DAEMON_FORWARDS: usize = 32;

/// The most open connections of one job's forwards, the bridge's number.
pub const MAX_FORWARD_CONNECTIONS: usize = 64;

/// The version of the frames that a `Hello` carries in its status byte.
pub const FORWARD_VERSION: u8 = 1;

/// The state `LISTEN` of a row of `/proc/net/tcp`.
pub const TCP_LISTEN: u8 = 0x0A;

/// The state `TIME_WAIT` of a row of `/proc/net/tcp`: the row of a closed connection,
/// which the kernel shows with uid 0.
pub const TCP_TIME_WAIT: u8 = 0x06;

/// One row of `/proc/net/tcp` or `/proc/net/tcp6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TcpRow {
    /// The state, such as [`TCP_LISTEN`].
    pub state: u8,
    /// The local address and port.
    pub local: SocketAddr,
    /// The remote address and port; `0.0.0.0:0` or `[::]:0` for a listener.
    pub remote: SocketAddr,
    /// The uid of the socket's owner, as the reader's user namespace shows it.
    pub uid: u32,
}

/// The rows of `/proc/net/tcp` (`v6 = false`) or `/proc/net/tcp6` (`v6 = true`). The
/// header line and a line that is not a row of that table are skipped.
///
/// The kernel prints each address as 32-bit words in the byte order of the machine, so
/// the parse reads them in the byte order of the machine that runs it: the table comes
/// from the same machine.
pub fn parse_tcp_table(text: &str, v6: bool) -> Vec<TcpRow> {
    text.lines().filter_map(|line| parse_row(line, v6)).collect()
}

fn parse_row(line: &str, v6: bool) -> Option<TcpRow> {
    let mut fields = line.split_whitespace();
    let slot = fields.next()?.strip_suffix(':')?;
    if slot.is_empty() || !slot.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let local = parse_socket(fields.next()?, v6)?;
    let remote = parse_socket(fields.next()?, v6)?;
    let state = fields.next()?;
    if state.len() != 2 {
        return None;
    }
    let state = u8::try_from(hex(state)?).ok()?;
    // The queues, the timer and the retransmits.
    for _ in 0..3 {
        fields.next()?;
    }
    let uid = fields.next()?;
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let uid = uid.parse().ok()?;
    Some(TcpRow { state, local, remote, uid })
}

/// A hex number of at most 8 digits, with no sign.
fn hex(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 8 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}

fn parse_socket(text: &str, v6: bool) -> Option<SocketAddr> {
    let (addr, port) = text.split_once(':')?;
    if port.len() != 4 {
        return None;
    }
    let port = u16::try_from(hex(port)?).ok()?;
    if v6 {
        if addr.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (index, chunk) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *chunk = hex(addr.get(index * 8..index * 8 + 8)?)?.to_ne_bytes();
        }
        Some(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(bytes), port, 0, 0)))
    } else {
        if addr.len() != 8 {
            return None;
        }
        let word = hex(addr)?;
        Some(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(word.to_ne_bytes()), port)))
    }
}

/// A loopback port that a job listens on, with the address that the forwarder connects
/// to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Listen {
    /// The address to connect to: an IPv4 address of `127.0.0.0/8`, or `::1`.
    pub addr: IpAddr,
    /// The port.
    pub port: u16,
}

/// The loopback listens of a job's `tcp` (`v4`) and `tcp6` (`v6`) rows, one for each
/// port, sorted by port (DL3).
///
/// A `LISTEN` row counts when its local address is in `127.0.0.0/8`, `0.0.0.0`, `::1`,
/// `::` or `::ffff:127.x.y.z`. The address to connect to is `127.0.0.1` for
/// `0.0.0.0`, `::1` for `::`, the IPv4 address for a v4-mapped one, else the address
/// itself. When one port has more than one listen, IPv4 wins, and `127.0.0.1` before
/// another IPv4 address.
pub fn loopback_listens(v4: &[TcpRow], v6: &[TcpRow]) -> Vec<Listen> {
    let mut best: BTreeMap<u16, (u8, IpAddr)> = BTreeMap::new();
    for row in v4.iter().chain(v6) {
        if row.state != TCP_LISTEN {
            continue;
        }
        let Some(addr) = connect_addr(row.local.ip()) else { continue };
        let rank = match addr {
            IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST => 0,
            IpAddr::V4(_) => 1,
            IpAddr::V6(_) => 2,
        };
        let port = row.local.port();
        let better = best.get(&port).is_none_or(|known| (rank, addr) < *known);
        if better {
            best.insert(port, (rank, addr));
        }
    }
    best.into_iter().map(|(port, (_, addr))| Listen { addr, port }).collect()
}

/// The address that the forwarder connects to for a listen on `local`, or `None` when
/// the listen is not on loopback.
fn connect_addr(local: IpAddr) -> Option<IpAddr> {
    match local {
        IpAddr::V4(ip) if ip.is_unspecified() => Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        IpAddr::V4(ip) if ip.is_loopback() => Some(IpAddr::V4(ip)),
        IpAddr::V4(_) => None,
        IpAddr::V6(ip) if ip.is_unspecified() || ip.is_loopback() => {
            Some(IpAddr::V6(Ipv6Addr::LOCALHOST))
        }
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) if v4.is_loopback() => Some(IpAddr::V4(v4)),
            _ => None,
        },
    }
}

/// Why efr never forwards a port (DL5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PortRefusal {
    /// Below [`MIN_PORT`].
    BelowMin,
    /// [`BRIDGE_PORT`] in a proxy job.
    Bridge,
    /// One of [`SERVICE_PORTS`].
    Service,
}

impl PortRefusal {
    /// The short reason, as the line of a refused listen ends: `a service port`.
    pub const fn reason(self) -> &'static str {
        match self {
            PortRefusal::BelowMin => "a port below 1024",
            PortRefusal::Bridge => "the port of the proxy bridge",
            PortRefusal::Service => "a service port",
        }
    }
}

/// Why efr never forwards `port`, or `None` when it may. `proxy` is true in a proxy
/// job, where [`BRIDGE_PORT`] is the bridge's.
pub fn refusal(port: u16, proxy: bool) -> Option<PortRefusal> {
    if port < MIN_PORT {
        Some(PortRefusal::BelowMin)
    } else if proxy && port == BRIDGE_PORT {
        Some(PortRefusal::Bridge)
    } else if SERVICE_PORTS.binary_search(&port).is_ok() {
        Some(PortRefusal::Service)
    } else {
        None
    }
}

/// A change that the forwarder reports to efrd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Change {
    /// A port listens and is stable.
    Listen(Listen),
    /// A port that was reported no longer listens.
    Gone(Listen),
}

/// A port in the last poll.
#[derive(Debug, Clone, Copy)]
struct Seen {
    listen: Listen,
    since_ms: u64,
    reported: bool,
}

/// The debounce of the forwarder's polls (DL4): a port counts after it shows in polls
/// at least `stable_ms` apart, and a port of the namespace's ephemeral range only after
/// `ephemeral_stable_ms`. A port that one poll misses is gone, and counts again only
/// after it is stable again.
///
/// The forwarder never reports a port below [`MIN_PORT`] or the bridge's port of a
/// proxy job. It reports a service port, so efrd can say why it does not forward it,
/// but such a port does not count to the limit. Of the other ports it reports at most
/// `max_ports + 1` over the job's life: the ports that efrd can forward, and one more,
/// so efrd can say once that the job reached its limit. A port that it reported
/// before is reported again after it was gone.
#[derive(Debug, Clone)]
pub struct Debounce {
    stable_ms: u64,
    ephemeral_stable_ms: u64,
    ephemeral: RangeInclusive<u16>,
    proxy: bool,
    max_reports: usize,
    seen: BTreeMap<u16, Seen>,
    counted: BTreeSet<u16>,
}

impl Debounce {
    /// The debounce of `plan`, with the namespace's ephemeral range `ephemeral`
    /// (`/proc/sys/net/ipv4/ip_local_port_range`).
    pub fn new(plan: &ForwardPlan, ephemeral: RangeInclusive<u16>) -> Debounce {
        Debounce {
            stable_ms: u64::from(plan.stable_ms),
            ephemeral_stable_ms: u64::from(plan.ephemeral_stable_ms),
            ephemeral,
            proxy: plan.proxy,
            max_reports: usize::from(plan.max_ports) + 1,
            seen: BTreeMap::new(),
            counted: BTreeSet::new(),
        }
    }

    /// The changes after a poll at `now_ms` that found `listens` (one for each port, as
    /// [`loopback_listens`] gives them; a second listen of a port is ignored). The
    /// `Gone` changes come first, then the `Listen` changes, each sorted by port.
    pub fn observe(&mut self, now_ms: u64, listens: &[Listen]) -> Vec<Change> {
        let mut now: BTreeMap<u16, Listen> = BTreeMap::new();
        for listen in listens {
            let never = matches!(
                refusal(listen.port, self.proxy),
                Some(PortRefusal::BelowMin | PortRefusal::Bridge)
            );
            if !never {
                now.entry(listen.port).or_insert(*listen);
            }
        }
        let mut gone = Vec::new();
        self.seen.retain(|port, seen| {
            let keep = now.contains_key(port);
            if !keep && seen.reported {
                gone.push(Change::Gone(seen.listen));
            }
            keep
        });
        let mut found = Vec::new();
        for (port, listen) in now {
            let seen =
                self.seen.entry(port).or_insert(Seen { listen, since_ms: now_ms, reported: false });
            if seen.listen != listen {
                if seen.reported {
                    gone.push(Change::Gone(seen.listen));
                }
                *seen = Seen { listen, since_ms: now_ms, reported: false };
            }
            if seen.reported {
                continue;
            }
            let need = if self.ephemeral.contains(&port) {
                self.ephemeral_stable_ms
            } else {
                self.stable_ms
            };
            if now_ms.saturating_sub(seen.since_ms) < need {
                continue;
            }
            let service = refusal(port, self.proxy).is_some();
            if !service && !self.counted.contains(&port) {
                if self.counted.len() >= self.max_reports {
                    continue;
                }
                self.counted.insert(port);
            }
            seen.reported = true;
            found.push(Change::Listen(listen));
        }
        gone.sort_by_key(|change| match change {
            Change::Gone(listen) | Change::Listen(listen) => listen.port,
        });
        gone.extend(found);
        gone
    }
}

/// The uid of the host process that connected to efrd's listener on `127.0.0.1:port`
/// from `peer`, from the host's tcp rows (DL9); `None` when no row or two rows that
/// disagree name it, or when `peer` is no IPv4 loopback address.
///
/// The peer's row has the local address `peer` and the remote address
/// `127.0.0.1:port`, in that order. efrd's own accepted socket has the mirror row
/// (local `port`, remote `peer`) with efrd's uid, and the match never takes it. A
/// listener and a `TIME_WAIT` row (uid 0) never count. A v4-mapped address of a `tcp6`
/// row counts as its IPv4 address.
pub fn peer_uid(rows: &[TcpRow], port: u16, peer: SocketAddr) -> Option<u32> {
    let peer_ip = v4_of(peer.ip()).filter(Ipv4Addr::is_loopback)?;
    if port == peer.port() && peer_ip == Ipv4Addr::LOCALHOST {
        return None;
    }
    let mut uid = None;
    for row in rows {
        if row.state == TCP_LISTEN || row.state == TCP_TIME_WAIT {
            continue;
        }
        let matches = row.local.port() == peer.port()
            && row.remote.port() == port
            && v4_of(row.local.ip()) == Some(peer_ip)
            && v4_of(row.remote.ip()) == Some(Ipv4Addr::LOCALHOST);
        if !matches {
            continue;
        }
        match uid {
            None => uid = Some(row.uid),
            Some(known) if known == row.uid => {}
            Some(_) => return None,
        }
    }
    uid
}

/// The IPv4 address of `ip`, also of a v4-mapped IPv6 address.
fn v4_of(ip: IpAddr) -> Option<Ipv4Addr> {
    match ip {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(ip) => ip.to_ipv4_mapped(),
    }
}

/// True when a `LISTEN` row of the host's `tcp6` rows holds `port` on `::1` or `::`
/// (DL8): a browser tries `::1` first for `localhost`, so a forward beside that server
/// would send the user to the wrong one.
pub fn v6_holds(rows: &[TcpRow], port: u16) -> bool {
    rows.iter().any(|row| {
        row.state == TCP_LISTEN
            && row.local.port() == port
            && matches!(row.local.ip(), IpAddr::V6(ip) if ip.is_loopback() || ip.is_unspecified())
    })
}

/// The length of every frame between the forwarder and efrd.
pub const FRAME_LEN: usize = 28;

/// The kind of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameKind {
    /// The forwarder's first frame on its control connection; the status is
    /// [`FORWARD_VERSION`].
    Hello = 1,
    /// The forwarder: a port listens and is stable.
    Listen = 2,
    /// The forwarder: a port no longer listens.
    Gone = 3,
    /// efrd: a host process connected; connect to the address and port, and answer
    /// with `Data` of the same id on a new connection.
    Open = 4,
    /// The forwarder's first frame on a data connection: the status is 0 when it
    /// connected, else the errno of the connect. With 0, opaque bytes follow both ways.
    Data = 5,
}

impl FrameKind {
    fn from_byte(byte: u8) -> Option<FrameKind> {
        match byte {
            1 => Some(FrameKind::Hello),
            2 => Some(FrameKind::Listen),
            3 => Some(FrameKind::Gone),
            4 => Some(FrameKind::Open),
            5 => Some(FrameKind::Data),
            _ => None,
        }
    }

    /// True for the kinds that name a listen: `Listen`, `Gone` and `Open`.
    pub const fn has_listen(self) -> bool {
        matches!(self, FrameKind::Listen | FrameKind::Gone | FrameKind::Open)
    }
}

/// One frame between the forwarder and efrd.
///
/// Layout, [`FRAME_LEN`] bytes: kind u8, family u8 (4 or 6), status u8, 0 u8, port u16
/// big-endian, 0 u16, id u32 big-endian, the address in 16 bytes (an IPv4 address in
/// the first 4, the rest 0).
///
/// `Listen`, `Gone` and `Open` carry a loopback address and a port from [`MIN_PORT`]
/// up, and status 0. `Hello` and `Data` carry no address: family 4, the address
/// `0.0.0.0` and port 0. Only `Open` and `Data` carry an id; the others have id 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Frame {
    /// The kind.
    pub kind: FrameKind,
    /// The version of a `Hello`, the result of a `Data`, else 0.
    pub status: u8,
    /// The connection of an `Open` and its `Data`, else 0.
    pub id: u32,
    /// The address of a listen, else `0.0.0.0`.
    pub addr: IpAddr,
    /// The port of a listen, else 0.
    pub port: u16,
}

impl Frame {
    /// The forwarder's `Hello`.
    pub const fn hello() -> Frame {
        Frame {
            kind: FrameKind::Hello,
            status: FORWARD_VERSION,
            id: 0,
            addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
        }
    }

    /// A `Listen` of `listen`.
    pub const fn listen(listen: Listen) -> Frame {
        Frame { kind: FrameKind::Listen, status: 0, id: 0, addr: listen.addr, port: listen.port }
    }

    /// A `Gone` of `listen`.
    pub const fn gone(listen: Listen) -> Frame {
        Frame { kind: FrameKind::Gone, status: 0, id: 0, addr: listen.addr, port: listen.port }
    }

    /// efrd's `Open` of connection `id` to `listen`.
    pub const fn open(id: u32, listen: Listen) -> Frame {
        Frame { kind: FrameKind::Open, status: 0, id, addr: listen.addr, port: listen.port }
    }

    /// The forwarder's `Data` of connection `id`: `status` 0 when it connected, else
    /// the errno of the connect.
    pub const fn data(id: u32, status: u8) -> Frame {
        Frame {
            kind: FrameKind::Data,
            status,
            id,
            addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
        }
    }

    /// The listen that a `Listen`, `Gone` or `Open` names.
    pub const fn target(&self) -> Listen {
        Listen { addr: self.addr, port: self.port }
    }

    /// The frame's bytes.
    pub fn encode(&self) -> [u8; FRAME_LEN] {
        let mut bytes = [0u8; FRAME_LEN];
        bytes[0] = self.kind as u8;
        bytes[2] = self.status;
        bytes[4..6].copy_from_slice(&self.port.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.id.to_be_bytes());
        match self.addr {
            IpAddr::V4(ip) => {
                bytes[1] = 4;
                bytes[12..16].copy_from_slice(&ip.octets());
            }
            IpAddr::V6(ip) => {
                bytes[1] = 6;
                bytes[12..28].copy_from_slice(&ip.octets());
            }
        }
        bytes
    }

    /// Reads a frame, and refuses an unknown kind, a reserved byte that is not 0, a
    /// family other than 4 and 6, and a frame that breaks the rules of its kind: an
    /// address that is not loopback or a port below [`MIN_PORT`] for a listen, an
    /// address, a port or an id where the kind has none.
    pub fn decode(bytes: &[u8; FRAME_LEN]) -> Result<Frame, SandboxError> {
        let refuse = |problem: &'static str| Err(SandboxError::Frame { problem });
        let Some(kind) = FrameKind::from_byte(bytes[0]) else {
            return refuse("an unknown kind");
        };
        if bytes[3] != 0 || bytes[6] != 0 || bytes[7] != 0 {
            return refuse("a reserved byte that is not 0");
        }
        let addr = match bytes[1] {
            4 => {
                if bytes[16..].iter().any(|byte| *byte != 0) {
                    return refuse("a reserved byte that is not 0");
                }
                IpAddr::V4(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]))
            }
            6 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&bytes[12..28]);
                IpAddr::V6(Ipv6Addr::from(octets))
            }
            _ => return refuse("a family other than 4 and 6"),
        };
        let status = bytes[2];
        let port = u16::from_be_bytes([bytes[4], bytes[5]]);
        let id = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if kind.has_listen() {
            if !addr.is_loopback() {
                return refuse("an address that is not loopback");
            }
            if port < MIN_PORT {
                return refuse("a port below 1024");
            }
            if status != 0 {
                return refuse("a status where the kind has none");
            }
        } else if addr != IpAddr::V4(Ipv4Addr::UNSPECIFIED) || port != 0 {
            return refuse("an address where the kind has none");
        }
        if id != 0 && !matches!(kind, FrameKind::Open | FrameKind::Data) {
            return refuse("an id where the kind has none");
        }
        Ok(Frame { kind, status, id, addr, port })
    }
}

#[cfg(test)]
mod tests;
