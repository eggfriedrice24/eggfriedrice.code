//! The reach of a contained call (v0.0.4 shell items, DS34-DS48): efrd's side of the
//! call's reach helper (`efr-sbx reach`), which lets the call reach the forwarded ports
//! of its own conversation's jobs at `127.0.0.1` and `[::1]` in its own network
//! namespace.
//!
//! - Which call ([`plan_of`], DS34): a contained foreground call with a network namespace
//!   of its own (no grant of the whole network, no network view), while
//!   `jobs.forward_ports` is true and a running job of the conversation has forwards that
//!   efrd serves. Its spec then holds a `ReachPlan` with the forwarded ports of the
//!   conversation at the prepare (DS35), and efrd binds `$CALL/reach.sock` (0600) before
//!   the launch ([`bind`]). A conversation with no such job makes no socket.
//! - The control connection ([`serve`]): the first connection that sends `Hello` with
//!   `REACH_VERSION`. When none came within [`HELLO_WAIT`], efrd logs one warning; the
//!   call runs on with no reach. efrd sends `Reach` for each port that a job of the
//!   conversation forwards and the helper does not hold yet, and `Unreach` for each port
//!   that no job forwards any more, on both `127.0.0.1` and `[::1]` (DS39). It only logs
//!   the helper's `Bound` frames. Any other frame, or one that does not decode, closes
//!   the control connection, and the helper ends.
//! - A data connection: its first frame is `Connect` with a port. efrd looks the port up
//!   in [`Forwards`] for the call's own conversation on every connection, and opens it
//!   in the job through the job's forwarder (DS41); efrd never opens a TCP connection on
//!   the host for a call. A port of another conversation, a port that no job forwards and
//!   a server that does not answer close the connection with no byte, and the helper then
//!   resets the client's connection.
//! - Limits (DS42): at most [`MAX_REACH_CONNECTIONS`] connections of a call at one time,
//!   which leaves room in the job's own limit for the host's connections; no byte budget
//!   and no idle limit.
//! - The end: dropping the [`ReachCall`] closes the socket, the control connection and
//!   every copy. A call that ended drops it; a call that runs on past its timeout keeps
//!   it until its launcher wrote the result (`SandboxService::keep_reach`).

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use efr_config::JobsSettings;
use efr_protocol::{CallId, ConversationId, Grant};
use efr_sandbox::{
    FRAME_LEN, Frame, FrameKind, Listen, MAX_REACH_CONNECTIONS, NetworkPlan, REACH_SOCKET_FILE,
    REACH_VERSION, ReachPlan, SandboxError, SandboxSpec, SpecLaunch,
};
use efr_stdx::time::Clock;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio_util::sync::CancellationToken;

use super::{Forwards, OPEN_WAIT};
use crate::DaemonError;
use crate::sandbox::proxy::bind_socket;

/// How long efrd waits for the reach helper's `Hello` before it warns once.
pub(crate) const HELLO_WAIT: Duration = super::HELLO_WAIT;

/// How long the accept of the reach socket waits after it failed, such as at the limit
/// of open files.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// The addresses of the call's loopback where the helper listens for each port (DS36).
const ADDRESSES: [IpAddr; 2] = [IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)];

/// The reach plan of the call of `spec`, whose socket is `$CALL/reach.sock`; `None`
/// when the call gets no reach (DS34): a job, the exit child, a call with the host's
/// network namespace, `jobs.forward_ports = false`, or a conversation with no running
/// job whose forwards efrd serves. The plan says `proxy` exactly when the call's
/// network is the proxy, as the spec check wants.
pub(crate) fn plan_of(
    settings: &JobsSettings,
    forwards: &Forwards,
    spec: &SandboxSpec,
) -> Option<ReachPlan> {
    if !settings.forward_ports || spec.job.is_some() || spec.launch != SpecLaunch::Contained {
        return None;
    }
    let proxy = match spec.network {
        NetworkPlan::None => false,
        NetworkPlan::Proxy { .. } => true,
        _ => return None,
    };
    if spec.grants.iter().any(|grant| matches!(grant, Grant::OpenNetwork | Grant::NetView)) {
        return None;
    }
    if !forwards.has_forward_job(spec.conversation) {
        return None;
    }
    let ports = forwards.ports_of(spec.conversation);
    Some(ReachPlan::new(spec.runtime.reach_socket(), ports, proxy))
}

/// Binds `$CALL/reach.sock` (0600) in the call's `call_dir`, before the launch.
pub(crate) async fn bind(call_dir: &Path) -> Result<UnixListener, DaemonError> {
    let dir = call_dir.to_path_buf();
    let path = call_dir.join(REACH_SOCKET_FILE);
    let listener = tokio::task::spawn_blocking(move || bind_socket(&dir, REACH_SOCKET_FILE))
        .await
        .map_err(|_| DaemonError::TaskPanicked { task: "reach socket" })?
        .map_err(|source| DaemonError::Io { path: path.clone(), source })?;
    UnixListener::from_std(listener).map_err(|source| DaemonError::Io { path, source })
}

/// The reach of one call, from its prepare to its end. Dropping it closes the reach
/// socket, the control connection and every connection of the call.
#[derive(Debug)]
pub(crate) struct ReachCall {
    token: CancellationToken,
}

impl Drop for ReachCall {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

/// What the tasks of one call's reach share.
struct Shared {
    forwards: Forwards,
    clock: Arc<dyn Clock>,
    conversation: ConversationId,
    call: CallId,
    token: CancellationToken,
    /// The places of the call's connections (DS42).
    places: Arc<Semaphore>,
    /// True once a connection said `Hello`: it is the control connection.
    hello: watch::Sender<bool>,
    /// The ports of the plan, which the helper holds from its start.
    start: BTreeSet<u16>,
}

/// Serves the reach socket `listener` of the call `call` of `conversation`, whose plan
/// held the ports `start`.
pub(crate) fn serve(
    forwards: Forwards,
    clock: Arc<dyn Clock>,
    conversation: ConversationId,
    call: CallId,
    listener: UnixListener,
    start: &[u16],
) -> ReachCall {
    let token = CancellationToken::new();
    let (hello, _) = watch::channel(false);
    let shared = Arc::new(Shared {
        forwards,
        clock,
        conversation,
        call,
        token: token.clone(),
        places: Arc::new(Semaphore::new(MAX_REACH_CONNECTIONS)),
        hello,
        start: start.iter().copied().collect(),
    });
    tokio::spawn(warn_late(Arc::clone(&shared)));
    tokio::spawn(accept(shared, listener));
    ReachCall { token }
}

/// Warns once when no helper said `Hello` within [`HELLO_WAIT`]: its start failed, and
/// the call runs with no reach.
async fn warn_late(shared: Arc<Shared>) {
    let mut hello = shared.hello.subscribe();
    let waited = tokio::select! {
        () = shared.token.cancelled() => return,
        waited = shared.clock.timeout(HELLO_WAIT, hello.wait_for(|said| *said)) => waited,
    };
    if waited.is_err() {
        tracing::warn!(conversation = %shared.conversation, call_id = %shared.call, "no reach helper of a call said hello in 5 s; the call runs with no reach");
    }
}

/// Accepts the connections of the reach socket, each within a place of the call.
async fn accept(shared: Arc<Shared>, listener: UnixListener) {
    loop {
        let accepted = tokio::select! {
            () = shared.token.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, _)) => {
                let Ok(place) = Arc::clone(&shared.places).try_acquire_owned() else {
                    tracing::debug!(call_id = %shared.call, "a call has as many reach connections as it may");
                    continue;
                };
                tokio::spawn(connection(Arc::clone(&shared), stream, place));
            }
            Err(error) => {
                tracing::warn!(%error, call_id = %shared.call, "the reach socket of a call could not accept");
                tokio::select! {
                    () = shared.token.cancelled() => return,
                    () = shared.clock.sleep(ACCEPT_PAUSE) => {}
                }
            }
        }
    }
}

/// One connection of the helper: the control connection after `Hello`, or a data
/// connection after `Connect`. Any other first frame closes it.
async fn connection(shared: Arc<Shared>, mut stream: UnixStream, place: OwnedSemaphorePermit) {
    let mut bytes = [0u8; FRAME_LEN];
    let read = tokio::select! {
        () = shared.token.cancelled() => return,
        read = shared.clock.timeout(OPEN_WAIT, stream.read_exact(&mut bytes)) => read,
    };
    if !matches!(read, Ok(Ok(_))) {
        return;
    }
    match Frame::decode(&bytes) {
        Ok(frame) if frame.kind == FrameKind::Hello && frame.status == REACH_VERSION => {
            if shared.hello.send_replace(true) {
                tracing::debug!(call_id = %shared.call, "a second hello on a call's reach socket; efrd closed it");
                return;
            }
            // NOTE: the control connection is no connection of the call's processes.
            drop(place);
            control(&shared, stream).await;
        }
        Ok(frame) if frame.kind == FrameKind::Connect => {
            route(&shared, stream, frame.target()).await;
            drop(place);
        }
        Ok(_) => {
            tracing::debug!(call_id = %shared.call, "a reach connection began with a frame of another kind; efrd closed it");
        }
        Err(error) => {
            tracing::debug!(call_id = %shared.call, %error, "a reach connection began with a bad frame; efrd closed it");
        }
    }
}

/// Routes the data connection `stream` to `target` to the job of the call's own
/// conversation that forwards its port (DS41); closes it with no byte when there is
/// none, or when the job's server was not reached.
async fn route(shared: &Shared, stream: UnixStream, target: Listen) {
    let port = target.port;
    let Some(ctx) = shared.forwards.route(shared.conversation, port) else {
        tracing::debug!(conversation = %shared.conversation, call_id = %shared.call, port, "a reach connection names a port that no job of the conversation forwards; efrd closed it");
        return;
    };
    tracing::debug!(conversation = %shared.conversation, call_id = %shared.call, job = %ctx.job(), port, "efrd routes a reach connection to a job");
    let reached = tokio::select! {
        () = shared.token.cancelled() => return,
        reached = ctx.reach(port, stream) => reached,
    };
    if !reached {
        tracing::debug!(call_id = %shared.call, job = %ctx.job(), port, "a reach connection did not reach the job's server; efrd closed it");
    }
}

/// The control connection: the `Reach` and `Unreach` frames for each change of the
/// conversation's forwarded ports, and the helper's `Bound` frames, until the call's
/// reach ends or the helper sends something else.
async fn control(shared: &Shared, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let (frames_tx, mut frames) = mpsc::channel(16);
    tokio::spawn(read_frames(read, frames_tx, shared.token.clone()));
    let mut changes = shared.forwards.changes();
    let mut held = shared.start.clone();
    loop {
        changes.borrow_and_update();
        let now: BTreeSet<u16> =
            shared.forwards.ports_of(shared.conversation).into_iter().collect();
        let mut sends: Vec<Frame> = Vec::new();
        for port in now.difference(&held) {
            sends.extend(ADDRESSES.map(|addr| Frame::reach(Listen { addr, port: *port })));
        }
        for port in held.difference(&now) {
            sends.extend(ADDRESSES.map(|addr| Frame::unreach(Listen { addr, port: *port })));
        }
        if !send(shared, &mut write, &sends).await {
            return;
        }
        held = now;
        tokio::select! {
            () = shared.token.cancelled() => return,
            changed = changes.changed() => if changed.is_err() {
                return;
            },
            frame = frames.recv() => match frame {
                Some(Ok(frame)) if frame.kind == FrameKind::Bound => {
                    tracing::debug!(call_id = %shared.call, addr = %frame.addr, port = frame.port, status = frame.status, "the reach helper of a call answered");
                }
                Some(Ok(_)) => {
                    tracing::warn!(call_id = %shared.call, "the reach helper of a call sent a frame of another kind; efrd closed its reach");
                    return;
                }
                Some(Err(error)) => {
                    tracing::warn!(call_id = %shared.call, %error, "the reach helper of a call sent a bad frame; efrd closed its reach");
                    return;
                }
                None => {
                    tracing::debug!(call_id = %shared.call, "the reach helper of a call closed its connection");
                    return;
                }
            },
        }
    }
}

/// Writes `frames` to the control connection; false when the call's reach ended or the
/// write failed.
async fn send(shared: &Shared, write: &mut OwnedWriteHalf, frames: &[Frame]) -> bool {
    for frame in frames {
        tracing::debug!(call_id = %shared.call, kind = ?frame.kind, addr = %frame.addr, port = frame.port, "efrd tells a call's reach helper");
        let bytes = frame.encode();
        let written = tokio::select! {
            () = shared.token.cancelled() => return false,
            written = write.write_all(&bytes) => written,
        };
        if written.is_err() {
            return false;
        }
    }
    true
}

/// Reads the frames of the control connection into `frames` until it ends; a frame
/// that does not decode is the last.
async fn read_frames(
    mut read: OwnedReadHalf,
    frames: mpsc::Sender<Result<Frame, SandboxError>>,
    token: CancellationToken,
) {
    loop {
        let mut bytes = [0u8; FRAME_LEN];
        let read = tokio::select! {
            () = token.cancelled() => return,
            read = read.read_exact(&mut bytes) => read,
        };
        if read.is_err() {
            return;
        }
        let frame = Frame::decode(&bytes);
        let last = frame.is_err();
        if frames.send(frame).await.is_err() || last {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
