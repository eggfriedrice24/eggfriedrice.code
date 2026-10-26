//! The TCP listener on the tailnet address, its rebinds, and one task per connection.
//!
//! [`run`] binds where the [`View`] says, and binds again when anything of it changes:
//! the address, the node, the user, the tailnet, the other addresses of the node or its
//! name. Each binding is a generation with a token of its
//! own: a new binding, or a view that goes down, cancels it, which closes the listener
//! and every phone socket of the old generation. A bind that fails (the address is not
//! up yet) is tried again after a pause.
//!
//! Each accepted stream gets a task: the admission (whois before any byte), then the
//! WebSocket of `efr-transport` with the self origin. At most [`MAX_CONNECTIONS`] run at
//! once; a stream beyond them is closed at once.

use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use efr_http::LocalApi;
use efr_transport::{RemotePeer, WebSocketOptions};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::RemoteEndpoint;
use super::admission::{self, Admission};
use super::tailnet::{Tailnet, View};
use crate::discovery::{self, DaemonInfo};
use crate::methods::Methods;
use crate::state::State;

/// The most phone connections at one time, the ones in admission included.
pub(crate) const MAX_CONNECTIONS: usize = 16;

/// The pause before a failed bind is tried again.
const BIND_RETRY: Duration = Duration::from_secs(5);

/// The pause after an `accept` that failed, such as for too many open files.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// The next key of an admitted connection in the table of live devices.
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// What the listener needs.
pub(crate) struct Listen {
    pub(crate) state: Arc<State>,
    pub(crate) methods: Arc<Methods>,
    pub(crate) api: LocalApi,
    /// The port of `[remote] port`.
    pub(crate) port: u16,
    /// A test's address in place of the tailnet address.
    pub(crate) listen: Option<SocketAddrV4>,
    /// `daemon.json` without the endpoint.
    pub(crate) info: DaemonInfo,
}

/// One binding of the listener.
struct Generation {
    tailnet: Tailnet,
    token: CancellationToken,
    task: JoinHandle<()>,
}

impl Generation {
    async fn end(self) {
        self.token.cancel();
        if self.task.await.is_err() {
            tracing::warn!("the remote listener's accept task panicked");
        }
    }
}

/// Where to listen for `tailnet`.
fn address(tailnet: &Tailnet, port: u16, listen: Option<SocketAddrV4>) -> SocketAddrV4 {
    listen.unwrap_or(SocketAddrV4::new(tailnet.ipv4, port))
}

/// Binds and rebinds as `view` says, until `stop`.
pub(crate) async fn run(listen: Listen, mut view: watch::Receiver<View>, stop: CancellationToken) {
    let listen = Arc::new(listen);
    let mut current: Option<Generation> = None;
    let mut retry = false;
    loop {
        let wanted = match &*view.borrow_and_update() {
            View::Up(tailnet) => Some(tailnet.clone()),
            View::Down(_) | View::Unknown => None,
        };
        let keep = matches!(
            (&current, &wanted),
            (Some(generation), Some(tailnet)) if generation.tailnet == *tailnet
        );
        if !keep || retry {
            if let Some(generation) = current.take() {
                generation.end().await;
                unbound(&listen);
            }
            retry = false;
            if let Some(tailnet) = wanted {
                match bind(&listen, &tailnet, &stop).await {
                    Some(generation) => current = Some(generation),
                    None => retry = true,
                }
            }
        }
        let clock = Arc::clone(&listen.state.clock);
        let pause = async move {
            if retry {
                clock.sleep(BIND_RETRY).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! {
            () = stop.cancelled() => break,
            changed = view.changed() => if changed.is_err() { break },
            () = pause => {}
        }
    }
    if let Some(generation) = current.take() {
        generation.end().await;
        unbound(&listen);
    }
}

/// Binds the listener for `tailnet` and starts its accept loop; `None` when the bind
/// failed.
async fn bind(
    listen: &Arc<Listen>,
    tailnet: &Tailnet,
    stop: &CancellationToken,
) -> Option<Generation> {
    let addr = address(tailnet, listen.port, listen.listen);
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%addr, error = %error, "the remote listener could not bind");
            return None;
        }
    };
    let bound = match listener.local_addr() {
        Ok(SocketAddr::V4(bound)) => bound,
        Ok(SocketAddr::V6(_)) | Err(_) => addr,
    };
    let endpoint =
        RemoteEndpoint { host: *bound.ip(), port: bound.port(), fqdn: tailnet.fqdn.clone() };
    let mut options = WebSocketOptions::new().allow_origin(format!("http://{bound}"));
    if let Some(fqdn) = &tailnet.fqdn {
        options = options.allow_origin(format!("http://{fqdn}:{}", bound.port()));
    }
    tracing::info!(address = %bound, "the remote listener listens");
    let token = stop.child_token();
    let accept = Accept {
        listen: Arc::clone(listen),
        tailnet: tailnet.clone(),
        options: Arc::new(options),
        slots: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        token: token.clone(),
    };
    let task = tokio::spawn(accept.run(listener));
    // NOTE: the watch first, so a client that reads the endpoint in `daemon.json` finds
    // the listener ready for `device.pair_start`.
    listen.state.remote.endpoint.send_replace(Some(endpoint.clone()));
    write_endpoint(listen, Some(&endpoint));
    Some(Generation { tailnet: tailnet.clone(), token, task })
}

/// Records that the listener does not listen.
fn unbound(listen: &Listen) {
    listen.state.remote.endpoint.send_replace(None);
    write_endpoint(listen, None);
    tracing::info!("the remote listener stopped listening");
}

/// Writes `daemon.json` with the endpoint of the listener.
fn write_endpoint(listen: &Listen, endpoint: Option<&RemoteEndpoint>) {
    let info = DaemonInfo {
        tailnet_endpoint: endpoint.map(|endpoint| format!("{}:{}", endpoint.host, endpoint.port)),
        ..listen.info.clone()
    };
    if let Err(error) = discovery::write(&listen.state.dirs.daemon_json_path(), &info) {
        tracing::warn!(error = %error, "daemon.json could not be written");
    }
}

/// The accept loop of one generation.
struct Accept {
    listen: Arc<Listen>,
    tailnet: Tailnet,
    options: Arc<WebSocketOptions>,
    slots: Arc<Semaphore>,
    token: CancellationToken,
}

impl Accept {
    async fn run(self, listener: TcpListener) {
        let this = Arc::new(self);
        loop {
            let accepted = tokio::select! {
                () = this.token.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            match accepted {
                Ok((stream, peer)) => {
                    let Ok(slot) = Arc::clone(&this.slots).try_acquire_owned() else {
                        tracing::warn!(%peer, "too many phone connections; one was closed");
                        continue;
                    };
                    let accept = Arc::clone(&this);
                    tokio::spawn(async move {
                        accept.connection(stream, peer).await;
                        drop(slot);
                    });
                }
                Err(error) => {
                    tracing::debug!(error = %error, "the remote listener's accept failed");
                    tokio::select! {
                        () = this.token.cancelled() => return,
                        () = this.listen.state.clock.sleep(ACCEPT_PAUSE) => {}
                    }
                }
            }
        }
    }

    /// Admits and serves one stream.
    async fn connection(&self, stream: TcpStream, peer: SocketAddr) {
        let facts = tokio::select! {
            () = self.token.cancelled() => return,
            facts = self.admit(peer) => facts,
        };
        let facts = match facts {
            Ok(facts) => facts,
            Err(refusal) => {
                tracing::warn!(%peer, reason = refusal.as_str(), "a remote peer was refused");
                return;
            }
        };
        if let Err(error) = stream.set_nodelay(true) {
            tracing::debug!(error = %error, "TCP_NODELAY could not be set");
        }
        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
        let close = self.token.child_token();
        tracing::info!(
            %peer,
            node = facts.node_stable_id,
            node_name = facts.node_name,
            "a remote peer was admitted"
        );
        let admission = Arc::new(Admission::new(key, facts, &self.tailnet, close.clone()));
        let remote = RemotePeer::new(peer, admission);
        let state = &self.listen.state;
        let served = efr_transport::serve_websocket(
            stream,
            remote,
            &self.options,
            Arc::clone(&self.listen.methods),
            Arc::clone(&state.clock),
            close,
        )
        .await;
        state.remote.live.forget(key);
        if let Err(error) = served {
            tracing::debug!(%peer, error = %error, "a remote handshake failed");
        }
    }

    /// The whois of `peer` and the decision on it.
    async fn admit(&self, peer: SocketAddr) -> Result<admission::PeerFacts, admission::Refusal> {
        if let Some(refusal) =
            admission::refuse_before_whois(&self.tailnet, peer, self.listen.listen)
        {
            return Err(refusal);
        }
        let identity = self.listen.api.whois(peer).await.map_err(|error| {
            tracing::debug!(%peer, error = %error, "the whois failed");
            admission::Refusal::WhoisFailed
        })?;
        admission::decide(&self.tailnet, identity.map(admission::Whois::from))
    }
}
