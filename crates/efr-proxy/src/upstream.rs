//! The way to an upstream (DP6, DP7): the lookup on its semaphores and under its time
//! limit, the guard over every address, and the connect to the checked addresses only.

use std::io;
use std::net::{IpAddr, SocketAddr};

use efr_permissions::network::is_ip_literal;
use efr_protocol::BlockReason;
use efr_stdx::time::{Clock as _, SystemClock};
use tokio::net::TcpStream;

use crate::call::Call;
use crate::classify;

/// Why the proxy has no upstream for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    /// A refusal with a record: an address that the guard refused, or a literal that
    /// is no address.
    Refused(BlockReason),
    /// The upstream failed: a 502 with this reason and no record.
    Gateway(&'static str),
}

/// A stream to `host:port`. `private` is true for a host of a `private = true` entry.
///
/// An IP address needs no lookup. A name is resolved once; when any of its addresses
/// fails the guard, the name is refused. The connect tries the checked addresses in
/// order, never a second lookup.
pub(crate) async fn open(
    call: &Call,
    host: &str,
    port: u16,
    private: bool,
) -> Result<TcpStream, Failure> {
    let limits = call.deps.limits;
    let addresses = if is_ip_literal(host) {
        vec![host.parse::<IpAddr>().map_err(|_| Failure::Refused(BlockReason::IpLiteral))?]
    } else {
        let lookup = async {
            let _call = call.lookups.acquire().await.map_err(io::Error::other)?;
            let _all = call.all_lookups.acquire().await.map_err(io::Error::other)?;
            call.deps.resolver.resolve(host).await
        };
        match SystemClock.timeout(limits.resolve_timeout, lookup).await {
            Err(_) => return Err(gateway(call, host, port, "timed out")),
            Ok(Ok(addresses)) if !addresses.is_empty() => addresses,
            Ok(_) => return Err(gateway(call, host, port, "no such host")),
        }
    };
    let own = call.deps.own.addresses();
    if addresses.iter().any(|address| !call.deps.guard.passes(classify(*address, &own), private)) {
        return Err(Failure::Refused(BlockReason::PrivateAddress));
    }
    let attempts = async {
        let mut last = "unreachable";
        for address in addresses.iter().take(limits.addresses.max(1)) {
            let connect = call.deps.connector.connect(SocketAddr::new(*address, port));
            match SystemClock.timeout(limits.connect_timeout, connect).await {
                Ok(Ok(stream)) => return Ok(stream),
                Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    last = "refused";
                }
                Ok(Err(_)) => last = "unreachable",
                Err(_) => last = "timed out",
            }
        }
        Err(last)
    };
    match SystemClock.timeout(limits.connect_total, attempts).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(reason)) => Err(gateway(call, host, port, reason)),
        Err(_) => Err(gateway(call, host, port, "timed out")),
    }
}

/// A 502 for `reason`, with a debug line and no record.
fn gateway(call: &Call, host: &str, port: u16, reason: &'static str) -> Failure {
    tracing::debug!(call = %call.id, host, port, reason, "efr-proxy has no upstream");
    Failure::Gateway(reason)
}
