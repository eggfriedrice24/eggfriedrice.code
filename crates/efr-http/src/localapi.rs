//! A small typed client of tailscaled's LocalAPI, over its Unix socket.
//!
//! efrd needs three answers from tailscaled for its remote listener: the state of this
//! node ([`LocalApi::status`]), who a tailnet peer is ([`LocalApi::whois`]) and when
//! either changes ([`LocalApi::watch`]). Each reply is parsed into a small struct that
//! keeps only the fields efrd checks; tailscaled sends many more, and they are
//! ignored. The socket path is a parameter: the daemon takes it from
//! `remote.tailscaled_socket`.
//!
//! tailscaled is a local daemon, not a model provider, so it is the one vendor this
//! crate knows. What efrd allows from these answers (which peers it admits, when it
//! binds) is the daemon's decision, not this module's.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_stdx::time::Clock;
use http::StatusCode;
use url::Url;

use crate::{HttpError, HttpRequest, HttpResponse, UnixClient};

mod status;
mod version;
mod watch;
mod whois;

pub use status::{BackendState, CurrentTailnet, StatusNode, TailscaleStatus};
pub use version::TailscaleVersion;
pub use watch::{IpnNotify, IpnState, IpnWatch, SelfChange};
pub use whois::PeerIdentity;

/// The host name that tailscaled's LocalAPI expects in the `Host` header. It refuses
/// other hosts, so a web page that reaches the socket through a proxy gets nothing.
pub const LOCALAPI_HOST: &str = "local-tailscaled.sock";

/// The response header in which tailscaled names its version.
pub const VERSION_HEADER: &str = "tailscale-version";

/// The mask of [`LocalApi::watch`]: `NotifyInitialState` (2) and `NotifyInitialStatus`
/// (16384). It never holds bit 8, the obsolete net map bit, which tailscaled refuses
/// with HTTP 400.
pub const WATCH_MASK: u32 = 2 | 16_384;

/// The line cap of [`LocalApi::watch`]. The first message holds the whole status of
/// the node, peers included, which can be large on a big tailnet.
pub const WATCH_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// A client of tailscaled's LocalAPI on one Unix socket.
#[derive(Debug, Clone)]
pub struct LocalApi {
    client: UnixClient,
}

impl LocalApi {
    /// A client for the LocalAPI socket at `socket`, with the default timeout of
    /// [`UnixClient`].
    pub fn new(socket: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        LocalApi { client: UnixClient::new(socket, clock) }
    }

    /// The same client with a different timeout for connecting, sending and the
    /// response head.
    #[must_use]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        LocalApi { client: self.client.with_timeout(timeout) }
    }

    /// The socket.
    pub fn socket(&self) -> &Path {
        self.client.socket()
    }

    /// `GET /localapi/v0/status?peers=false`: the state of this node, without its
    /// peers.
    pub async fn status(&self) -> Result<TailscaleStatus, HttpError> {
        let response = self.get("/localapi/v0/status", &[("peers", "false")]).await?;
        let header = TailscaleVersion::from_headers(response.headers());
        let response = ok(response)?;
        let mut status: TailscaleStatus = response.json().await?;
        status.version = header.or_else(|| TailscaleVersion::parse(&status.version_text));
        Ok(status)
    }

    /// `GET /localapi/v0/whois?addr=<ip:port>&proto=tcp`: who the peer at `peer` is.
    /// `None` when tailscaled knows no node for it (HTTP 404), which a caller must read
    /// as "refuse".
    ///
    /// An IPv4 address mapped into IPv6 is unmapped first, as tailscaled's own lookup
    /// does. Fails when the reply lacks the node or its user, or names no stable node
    /// id, so a caller never admits a peer on a partial answer.
    pub async fn whois(&self, peer: SocketAddr) -> Result<Option<PeerIdentity>, HttpError> {
        let addr = SocketAddr::new(unmapped(peer.ip()), peer.port()).to_string();
        let response = self.get("/localapi/v0/whois", &[("addr", &addr), ("proto", "tcp")]).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = ok(response)?;
        let url = response.url().to_owned();
        let reply: whois::WhoIsReply = response.json().await?;
        reply.identity(url).map(Some)
    }

    /// `GET /localapi/v0/watch-ipn-bus?mask=16386`: a stream of the changes of this
    /// node. The first message holds the state and the whole status; later messages
    /// hold what changed. The body never ends while tailscaled runs.
    pub async fn watch(&self) -> Result<IpnWatch, HttpError> {
        let mask = WATCH_MASK.to_string();
        let response = self.get("/localapi/v0/watch-ipn-bus", &[("mask", &mask)]).await?;
        let version = TailscaleVersion::from_headers(response.headers());
        let response = ok(response)?;
        Ok(IpnWatch::new(response.into_json_lines(WATCH_MAX_LINE_BYTES), version))
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<HttpResponse, HttpError> {
        let mut url = Url::parse(&format!("http://{LOCALAPI_HOST}"))
            .map_err(|source| HttpError::InvalidUrl { source })?;
        url.set_path(path);
        url.query_pairs_mut().extend_pairs(query);
        self.client.send(&HttpRequest::get(url.as_str())?).await
    }
}

/// `response` when its status is a success, else [`HttpError::LocalApiStatus`].
fn ok(response: HttpResponse) -> Result<HttpResponse, HttpError> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        Err(HttpError::LocalApiStatus { url: response.url().to_owned(), status })
    }
}

/// `ip` with an IPv4 address mapped into IPv6 turned back into IPv4.
pub(crate) fn unmapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

#[cfg(test)]
mod tests;
