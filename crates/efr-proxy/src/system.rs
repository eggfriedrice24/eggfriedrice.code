//! The system seams: the real resolver, the real connector and the host's real
//! addresses. efrd builds them; no test does (DP30). This is the one file of the crate
//! that calls `lookup_host` or `TcpStream::connect`.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nix::ifaddrs::getifaddrs;
use nix::sys::socket::SockaddrStorage;
use tokio::net::TcpStream;

use crate::deps::{Connecting, Connector, OwnAddresses, Resolver, Resolving};

/// `getaddrinfo` through `tokio::net::lookup_host`, on tokio's blocking pool. It gives
/// every address of every family.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str) -> Resolving<'a> {
        Box::pin(async move {
            let found = tokio::net::lookup_host((host, 0)).await?;
            Ok(found.map(|addr| addr.ip()).collect())
        })
    }
}

/// A plain TCP connect.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemConnector;

impl Connector for SystemConnector {
    fn connect(&self, addr: SocketAddr) -> Connecting<'_> {
        Box::pin(async move {
            let stream = TcpStream::connect(addr).await?;
            stream.set_nodelay(true)?;
            Ok(stream)
        })
    }
}

/// How long a read of the host's addresses stays fresh. VPN and DHCP change them, so
/// the proxy reads them again after this time.
const FRESH: Duration = Duration::from_secs(1);

/// One read of the host's addresses, and when it was.
type Read = (Instant, Vec<(IpAddr, u8)>);

/// The host's addresses from `getifaddrs`, each with the prefix length of its netmask,
/// read again at most once a second.
#[derive(Debug, Default)]
pub struct SystemAddresses {
    cache: Mutex<Option<Read>>,
}

impl SystemAddresses {
    /// A reader with nothing read yet.
    pub fn new() -> SystemAddresses {
        SystemAddresses::default()
    }
}

impl OwnAddresses for SystemAddresses {
    fn addresses(&self) -> Vec<(IpAddr, u8)> {
        let mut cache = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((read, addresses)) = cache.as_ref()
            && read.elapsed() < FRESH
        {
            return addresses.clone();
        }
        match read_addresses() {
            Ok(addresses) => {
                *cache = Some((Instant::now(), addresses.clone()));
                addresses
            }
            Err(error) => {
                // NOTE: an old list is better than none: with none, the guard would pass
                // the host's own global addresses.
                tracing::warn!(%error, "efr-proxy could not read the host's addresses");
                cache.as_ref().map(|(_, addresses)| addresses.clone()).unwrap_or_default()
            }
        }
    }
}

fn read_addresses() -> io::Result<Vec<(IpAddr, u8)>> {
    let mut addresses = Vec::new();
    for interface in getifaddrs().map_err(io::Error::from)? {
        let (Some(address), netmask) = (interface.address, interface.netmask) else {
            continue;
        };
        let Some(ip) = ip_of(&address) else {
            continue;
        };
        let full = if ip.is_ipv4() { 32 } else { 128 };
        let prefix = netmask.as_ref().and_then(ip_of).map_or(full, |mask| prefix_of(mask, full));
        addresses.push((ip, prefix));
    }
    Ok(addresses)
}

fn ip_of(address: &SockaddrStorage) -> Option<IpAddr> {
    if let Some(v4) = address.as_sockaddr_in() {
        return Some(IpAddr::V4(v4.ip()));
    }
    address.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip()))
}

/// The leading ones of a netmask, at most `full`.
fn prefix_of(mask: IpAddr, full: u8) -> u8 {
    let ones = match mask {
        IpAddr::V4(mask) => mask.to_bits().leading_ones(),
        IpAddr::V6(mask) => mask.to_bits().leading_ones(),
    };
    u8::try_from(ones).map_or(full, |ones| ones.min(full))
}
