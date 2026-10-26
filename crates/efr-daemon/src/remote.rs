//! The remote listener: the phone app's way to efrd over the tailnet.
//!
//! With `[remote] enabled`, efrd listens on its own Tailscale IPv4 address and on no
//! other interface. The parts, in the order a connection meets them:
//!
//! 1. `tailnet`: follows tailscaled's IPN bus and its status, and says where efrd may
//!    listen: only while the backend runs with a TUN device (never in userspace
//!    networking), with tailscaled 1.100 or later, an IPv4 address in 100.64.0.0/10, a
//!    node and a tailnet with stable ids. The end of the bus does not prove that
//!    tailscaled is down (a slow reader is dropped), so the follower asks the status
//!    first, and only a failed status closes the phone sockets.
//! 2. `listener`: binds the TCP listener there, and again when the address, the node
//!    or the tailnet changes; writes `tailnet_endpoint` in `daemon.json`.
//! 3. `admission`: right after `accept`, before any byte of HTTP, tailscaled's whois of
//!    the peer. It refuses an address of efrd's own node (any local process that dials
//!    100.x looks like efrd's own node), an unknown peer, the self node, a tagged
//!    node, a sharee node and every other user; a failed whois refuses too.
//! 4. The WebSocket handshake of `efr-transport` with the self origin, then the
//!    connection loop with a remote context, whose surface is always the phone.
//! 5. Before the proof, a connection may call only `hello`, `device.enroll` and
//!    `device.prove` (`methods.rs`). `prove` checks the device's P-256 signature over
//!    the challenge of the hello and the facts of the whois; three failed proofs close
//!    the socket. The scopes of a proved connection come from its device's row, never
//!    from the `/proc` walk of a local peer.
//! 6. `live`: the connections of each proved device. A revoke closes them at once and
//!    drops the frames still queued for them. The feed of revokes is subscribed when the
//!    daemon starts, before any device is looked up.
//!
//! `device.enroll` reads the facts of its connection through [`admission`]: the whois
//! facts of the peer (`Admission::peer`) and efrd's tailnet (`Admission::tailnet`),
//! which a new device row pins. The pairing QR code reads [`Remote::endpoint`].

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex, PoisonError};

use efr_config::RemoteSettings;
use efr_http::LocalApi;
use efr_protocol::{DeviceId, Hello, ProveContext};
use efr_store::WriterHandle;
use efr_transport::ConnectionContext;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

use crate::DaemonError;
use crate::discovery::DaemonInfo;
use crate::methods::Methods;
use crate::state::State;

mod admission;
mod listener;
mod live;
pub(crate) mod prove;
mod tailnet;

pub(crate) use admission::Admission;
use live::LiveDevices;
use tailnet::View;

/// How long a LocalAPI call of the remote listener may take before it fails. A whois
/// that takes longer refuses its peer.
const LOCALAPI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Where the remote listener listens now: what the pairing QR code shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteEndpoint {
    /// efrd's tailnet IPv4 address.
    pub(crate) host: Ipv4Addr,
    /// The port of the listener.
    pub(crate) port: u16,
    /// The MagicDNS name of efrd's node, without the dot at its end.
    pub(crate) fqdn: Option<String>,
}

/// The shared side of the remote listener in the daemon's state.
#[derive(Debug)]
pub(crate) struct Remote {
    /// Where the listener listens now; `None` while it does not listen. Only the
    /// listener sends on it; the pairing reads it for the QR code.
    pub(crate) endpoint: watch::Sender<Option<RemoteEndpoint>>,
    live: Arc<LiveDevices>,
    /// The feed of revokes, subscribed when the daemon starts; the follower of the
    /// listener takes it.
    revocations: Mutex<Option<broadcast::Receiver<DeviceId>>>,
    /// A test's address in place of the tailnet address (`Deps::with_remote_listen`).
    pub(crate) listen: Option<SocketAddrV4>,
}

impl Remote {
    /// The remote side of a daemon whose store writer is `writer`. It subscribes to the
    /// revokes now, so no revoke after the start can pass a device lookup unseen.
    pub(crate) fn new(writer: &WriterHandle, listen: Option<SocketAddrV4>) -> Self {
        Remote {
            endpoint: watch::Sender::new(None),
            live: Arc::new(LiveDevices::default()),
            revocations: Mutex::new(Some(writer.subscribe_revocations())),
            listen,
        }
    }

    pub(crate) fn live(&self) -> &LiveDevices {
        &self.live
    }

    fn take_revocations(&self) -> Option<broadcast::Receiver<DeviceId>> {
        self.revocations.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The admission of the remote connection of `context`; `None` for a local one.
pub(crate) fn admission(context: &ConnectionContext) -> Option<&Admission> {
    context.remote().and_then(|remote| remote.admission::<Admission>())
}

/// An admission for a test's remote connection, with the whois facts of `peer`.
#[cfg(test)]
pub(crate) fn test_admission(peer: &crate::pairing::EnrollPeer) -> Admission {
    let facts = admission::PeerFacts {
        node_stable_id: peer.node_stable_id.clone(),
        node_name: peer.node_name.clone(),
        host_name: String::new(),
        user_id: peer.user_id,
        login_name: peer.user_login.clone(),
        display_name: String::new(),
    };
    let tailnet = tailnet::Tailnet {
        ipv4: Ipv4Addr::new(100, 64, 0, 1),
        own_ips: Vec::new(),
        self_node: "nDAEMON1CNTRL".to_owned(),
        self_user: Some(peer.user_id),
        tailnet: peer.tailnet_stable_id.clone(),
        fqdn: None,
    };
    Admission::new(0, facts, &tailnet, CancellationToken::new())
}

/// The prove facts of the `hello` of a remote connection, which also keeps the
/// connection's challenge for `device.prove`; `None` for a local connection.
pub(crate) fn hello(
    context: &ConnectionContext,
    hello: &Hello,
    challenge: &str,
) -> Result<Option<ProveContext>, DaemonError> {
    if context.remote().is_none() {
        return Ok(None);
    }
    let admission = admission(context).ok_or(DaemonError::RemoteAdmissionMissing)?;
    admission.hello(challenge, hello.device_id);
    Ok(Some(admission.prove_context()))
}

/// Runs the remote listener with `settings` until `stop`: follows tailscaled, binds
/// and rebinds, admits and serves each phone, closes the sockets of revoked devices.
/// `info` is `daemon.json` without the endpoint; `listen` is a test's address in place
/// of the tailnet address ([`Remote::listen`]).
pub(crate) async fn serve(
    state: Arc<State>,
    methods: Arc<Methods>,
    settings: RemoteSettings,
    info: DaemonInfo,
    listen: Option<SocketAddrV4>,
    stop: CancellationToken,
) {
    // NOTE: a unit test must never reach the user's real tailscaled.
    #[cfg(test)]
    assert!(
        !settings.tailscaled_socket.starts_with("/run")
            && !settings.tailscaled_socket.starts_with("/var/run"),
        "a test points remote.tailscaled_socket at the real tailscaled"
    );
    let api = LocalApi::new(&settings.tailscaled_socket, Arc::clone(&state.clock))
        .with_timeout(LOCALAPI_TIMEOUT);
    tracing::info!(
        port = settings.port,
        socket = %settings.tailscaled_socket.display(),
        "the remote listener follows tailscaled"
    );
    let (views, view) = watch::channel(View::Unknown);
    let follower =
        tokio::spawn(tailnet::follow(api.clone(), Arc::clone(&state.clock), views, stop.clone()));
    let revokes = state.remote.take_revocations().map(|feed| {
        tokio::spawn(live::follow(
            Arc::clone(&state.remote.live),
            state.readers.clone(),
            feed,
            stop.clone(),
        ))
    });
    let listen = listener::Listen {
        state: Arc::clone(&state),
        methods,
        api,
        port: settings.port,
        listen,
        info,
    };
    listener::run(listen, view, stop).await;
    if follower.await.is_err() {
        tracing::warn!("the follower of tailscaled panicked");
    }
    if let Some(revokes) = revokes
        && revokes.await.is_err()
    {
        tracing::warn!("the follower of device revokes panicked");
    }
}

#[cfg(test)]
mod tests;
