//! [`Proxy`]: the deps, the live host list and the lookups of the whole proxy, which
//! serves one listener per call ([`Proxy::serve`]).

use std::net::IpAddr;
use std::sync::Arc;

use efr_permissions::HostList;
use efr_permissions::network::host_and_port;
use efr_protocol::CallId;
use tokio::net::UnixListener;
use tokio::sync::{Semaphore, watch};

use crate::call::{self, CallProxy};
use crate::deps::Deps;

/// One call's policy, frozen when efrd binds its socket (DP24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPolicy {
    /// The call.
    pub call: CallId,
    /// The call's `Grant::Host` entries: one host and one port each. Port 80 means plain
    /// `GET` and `HEAD`, never a tunnel.
    pub grants: Vec<(String, u16)>,
    /// Hosts that this call may not reach besides the deny list (DP25), such as a host
    /// for which a project file holds a token.
    pub deny: Vec<String>,
    /// The bytes that the call may send through the proxy, counted from the first byte
    /// after each `CONNECT` head, over every connection of the call (DP9).
    pub max_upload_bytes: u64,
}

/// The proxy: the deps, the live global host list and the lookups of every call.
#[derive(Debug, Clone)]
pub struct Proxy {
    pub(crate) deps: Arc<Deps>,
    pub(crate) hosts: watch::Receiver<Arc<HostList>>,
    pub(crate) lookups: Arc<Semaphore>,
}

impl Proxy {
    /// `hosts` is the live global list; a reload sends a new one (DP3), and every
    /// listener reads the newest list for each request.
    pub fn new(deps: Deps, hosts: watch::Receiver<Arc<HostList>>) -> Proxy {
        // NOTE: a semaphore with no permit would stall every lookup for ever.
        let lookups = Arc::new(Semaphore::new(deps.limits.lookups.max(1)));
        Proxy { deps: Arc::new(deps), hosts, lookups }
    }

    /// Serves one call's listener on the current runtime until the handle closes or
    /// drops. Must be called inside a tokio runtime.
    pub fn serve(&self, listener: UnixListener, policy: CallPolicy) -> CallProxy {
        let policy = CallPolicy {
            grants: policy.grants.iter().map(|(host, port)| (normal(host), *port)).collect(),
            deny: policy.deny.iter().map(|host| normal(host)).collect(),
            ..policy
        };
        call::serve(self, listener, policy)
    }
}

/// `host` as the proxy compares it: lower case, no trailing dot, an IPv6 address
/// without brackets.
pub(crate) fn normal(host: &str) -> String {
    let bare = host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')).unwrap_or(host);
    if let Ok(address) = bare.parse::<IpAddr>() {
        return address.to_string();
    }
    match host_and_port(host) {
        Some((host, _)) => host,
        None => host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase(),
    }
}
