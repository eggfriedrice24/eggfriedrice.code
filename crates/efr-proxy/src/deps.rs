//! What the proxy needs from the world: a resolver, a connector, the host's own
//! addresses, the guard and the limits (DP30). They are constructor arguments with no
//! default, so a test always says which ones it uses and never reaches the network by
//! accident.

use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;

use crate::Guard;

/// The future of [`Resolver::resolve`].
pub type Resolving<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>>;

/// The future of [`Connector::connect`].
pub type Connecting<'a> = Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send + 'a>>;

/// Name to addresses. The system one is `tokio::net::lookup_host`.
pub trait Resolver: Send + Sync + 'static {
    /// Every address of `host`, a name that passed the lists.
    fn resolve<'a>(&'a self, host: &'a str) -> Resolving<'a>;
}

/// Address to stream. The proxy connects only to an address that the guard checked.
pub trait Connector: Send + Sync + 'static {
    /// A TCP connection to `addr`.
    fn connect(&self, addr: SocketAddr) -> Connecting<'_>;
}

/// The host's own addresses with their prefix lengths (address and netmask of
/// `getifaddrs`). The system one reads them at most once a second.
pub trait OwnAddresses: Send + Sync + 'static {
    /// The addresses now.
    fn addresses(&self) -> Vec<(IpAddr, u8)>;
}

/// The limits of the proxy (DP6, DP10). A test sets short times, such as 200 ms, and
/// never a paused clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes of a request head: 8 KiB.
    pub head_bytes: usize,
    /// The time from the accept to the end of the head: 10 s.
    pub head_timeout: Duration,
    /// The most bytes of a ClientHello with the records around it: 16 KiB.
    pub hello_bytes: usize,
    /// The time from the `200` to the end of the ClientHello: 10 s.
    pub hello_timeout: Duration,
    /// The time of one lookup, with the wait for a free lookup: 5 s.
    pub resolve_timeout: Duration,
    /// The lookups of one call at once: 4.
    pub lookups_per_call: usize,
    /// The lookups of the whole proxy at once: 32.
    pub lookups: usize,
    /// The addresses that one request tries: 4.
    pub addresses: usize,
    /// The time of one connect: 10 s.
    pub connect_timeout: Duration,
    /// The time of every connect of one request: 15 s.
    pub connect_total: Duration,
    /// The connections of one call that are open at once, counted from the accept: 64.
    pub connections: usize,
    /// The refusal records of one call, one for each host, port and reason: 32.
    pub blocked: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            head_bytes: 8 * 1024,
            head_timeout: Duration::from_secs(10),
            hello_bytes: 16 * 1024,
            hello_timeout: Duration::from_secs(10),
            resolve_timeout: Duration::from_secs(5),
            lookups_per_call: 4,
            lookups: 32,
            addresses: 4,
            connect_timeout: Duration::from_secs(10),
            connect_total: Duration::from_secs(15),
            connections: 64,
            blocked: efr_protocol::SandboxSummary::MAX_BLOCKED,
        }
    }
}

/// Everything the proxy needs besides the lists. No field has a default.
#[derive(Clone)]
pub struct Deps {
    /// Name to addresses.
    pub resolver: Arc<dyn Resolver>,
    /// Address to stream.
    pub connector: Arc<dyn Connector>,
    /// The host's own addresses, which the guard refuses with their links.
    pub own: Arc<dyn OwnAddresses>,
    /// Which address classes pass.
    pub guard: Guard,
    /// The caps and the times.
    pub limits: Limits,
}

impl fmt::Debug for Deps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deps")
            .field("guard", &self.guard)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}
