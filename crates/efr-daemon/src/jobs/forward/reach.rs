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
//!   it until its launcher wrote the result (`SandboxService::keep_reach`). A call that
//!   the user moved into a background job loses its reach (DS25,
//!   [`ReachCall::unreach_all`]): the helper gets `Unreach` for every port first.
//! - The fact of a line ([`fact`], DS43, DS44): the forwarded ports of the conversation.
//!   When the line names a loopback port that is not forwarded yet and a job of the
//!   conversation began to forward less than [`REACH_SETTLE`] ago, efrd first waits for
//!   the port's outcome up to the rest of that time. A named port that a job of the
//!   conversation listens on but efrd does not forward (in use, refused, a limit,
//!   stopped) gives the fact a refusal ([`LineReach::refusal`]): in `auto` the call does
//!   not run, and nobody is asked for the whole network, which would reach another
//!   program of the machine, not the job.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use efr_config::JobsSettings;
use efr_permissions::network::host_and_port;
use efr_protocol::{CallId, ConversationId, Grant, Needs};
use efr_sandbox::{
    BRIDGE_PORT, FRAME_LEN, Frame, FrameKind, Listen, MAX_REACH_CONNECTIONS, MIN_PORT, NetworkPlan,
    REACH_SOCKET_FILE, REACH_VERSION, ReachPlan, SandboxError, SandboxSpec, SpecLaunch,
};
use efr_stdx::time::Clock;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use super::{Forwards, OPEN_WAIT};
use crate::DaemonError;
use crate::sandbox::proxy::bind_socket;

/// How long efrd waits for the reach helper's `Hello` before it warns once.
pub(crate) const HELLO_WAIT: Duration = super::HELLO_WAIT;

/// How long after a job of the conversation began to forward the fact of a line waits
/// for a loopback port that the line names (DS44): the forwarder reports a port once it
/// was stable for a second.
pub(crate) const REACH_SETTLE: Duration = Duration::from_secs(3);

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

/// How long the end of a call's reach at its move waits for the control connection to
/// send its `Unreach` frames.
const UNREACH_WAIT: Duration = Duration::from_secs(1);

/// The reach of one call, from its prepare to its end. Dropping it closes the reach
/// socket, the control connection and every connection of the call.
#[derive(Debug)]
pub(crate) struct ReachCall {
    token: CancellationToken,
    /// True once a helper said `Hello`.
    hello: watch::Receiver<bool>,
    /// Asks the control connection to send `Unreach` for every port it holds, and
    /// tells when it did.
    unreach: mpsc::Sender<oneshot::Sender<()>>,
    clock: Arc<dyn Clock>,
}

impl ReachCall {
    /// Ends the reach of a call that the user moved into a background job (DS25): the
    /// helper first gets `Unreach` for every port that it holds, so the moved command
    /// no longer reaches the forwarded ports of other jobs, and then the reach ends as
    /// at a drop. It waits at most [`UNREACH_WAIT`] for the frames.
    pub(crate) async fn unreach_all(self) {
        if *self.hello.borrow() {
            let (done, sent) = oneshot::channel();
            if self.unreach.try_send(done).is_ok()
                && self.clock.timeout(UNREACH_WAIT, sent).await.is_err()
            {
                tracing::debug!(
                    "the reach helper of a moved call did not take its unreach frames in time"
                );
            }
        }
    }
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
    /// The asks of [`ReachCall::unreach_all`], which the control connection takes.
    unreach: std::sync::Mutex<Option<mpsc::Receiver<oneshot::Sender<()>>>>,
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
    let (hello, said) = watch::channel(false);
    let (unreach, asks) = mpsc::channel(1);
    let shared = Arc::new(Shared {
        forwards,
        clock: Arc::clone(&clock),
        conversation,
        call,
        token: token.clone(),
        places: Arc::new(Semaphore::new(MAX_REACH_CONNECTIONS)),
        hello,
        start: start.iter().copied().collect(),
        unreach: std::sync::Mutex::new(Some(asks)),
    });
    tokio::spawn(warn_late(Arc::clone(&shared)));
    tokio::spawn(accept(shared, listener));
    ReachCall { token, hello: said, unreach, clock }
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
    let mut unreach =
        shared.unreach.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
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
            // NOTE: the call moved into a job, which gets no reach (DS25): the helper
            // closes every listener before the reach ends.
            Some(done) = next_unreach(&mut unreach) => {
                let sends: Vec<Frame> = held
                    .iter()
                    .flat_map(|port| {
                        ADDRESSES.map(|addr| Frame::unreach(Listen { addr, port: *port }))
                    })
                    .collect();
                send(shared, &mut write, &sends).await;
                tracing::debug!(call_id = %shared.call, ports = held.len(), "the reach of a call that moved into a job ended");
                let _ = done.send(());
                return;
            }
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

/// The next ask of [`ReachCall::unreach_all`]; never when the control connection has
/// none to take.
async fn next_unreach(
    asks: &mut Option<mpsc::Receiver<oneshot::Sender<()>>>,
) -> Option<oneshot::Sender<()>> {
    match asks {
        Some(asks) => asks.recv().await,
        None => std::future::pending().await,
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

/// The loopback ports that a line names (DS44): each URL of `command` and each entry of
/// `needs.hosts` on `localhost`, `127.0.0.1` or `[::1]` with a port from [`MIN_PORT`] up
/// but [`BRIDGE_PORT`]. It only predicts; the classifier reads the line itself.
pub(crate) fn named_ports(command: &str, needs: Option<&Needs>) -> Vec<u16> {
    let quotes = |c: char| c.is_whitespace() || "'\"`()<>;|&".contains(c);
    let urls = command.split(quotes).filter(|word| word.contains("://"));
    let hosts = needs.into_iter().flat_map(|needs| needs.hosts.iter().map(String::as_str));
    let mut ports = Vec::new();
    for word in urls.chain(hosts) {
        let Some((host, port)) = host_and_port(word) else { continue };
        let loopback = matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1");
        if loopback && port >= MIN_PORT && port != BRIDGE_PORT && !ports.contains(&port) {
            ports.push(port);
        }
    }
    ports
}

/// What a line knows about the loopback ports that it names (DS43, DS44).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineReach {
    /// The conversation's forwarded ports: the fact `CallFacts::reach_ports`.
    pub(crate) ports: Vec<u16>,
    /// What efr tells the model when the line names a port that a job of the
    /// conversation listens on and efrd does not forward ([`Forwards::unreached`]), one
    /// paragraph for each such port; `None` when there is none. In `auto` the call
    /// then does not run.
    pub(crate) refusal: Option<String>,
}

/// The fact of a line of `conversation` that names the loopback ports `named` (DS43,
/// DS44). When a named port is not forwarded and a job of the conversation began to
/// forward less than [`REACH_SETTLE`] ago, it first waits on `clock` for the port's
/// outcome up to the rest of that time.
pub(crate) async fn fact(
    forwards: &Forwards,
    clock: &Arc<dyn Clock>,
    conversation: ConversationId,
    named: &[u16],
) -> LineReach {
    let ports = forwards.ports_of(conversation);
    let missing: Vec<u16> = named.iter().filter(|port| !ports.contains(port)).copied().collect();
    if missing.is_empty() {
        return LineReach { ports, refusal: None };
    }
    let unsettled: Vec<u16> = missing
        .iter()
        .filter(|port| forwards.unreached(conversation, **port).is_none())
        .copied()
        .collect();
    let rest = forwards.newest_start(conversation).map_or(Duration::ZERO, |since| {
        let age = Duration::try_from(clock.now().duration_since(since)).unwrap_or(Duration::ZERO);
        REACH_SETTLE.saturating_sub(age)
    });
    let ports = if unsettled.is_empty() || rest.is_zero() {
        ports
    } else {
        tracing::debug!(%conversation, ports = ?unsettled, wait_ms = rest.as_millis(), "a line names a port of a new job; efrd waits for its outcome");
        let _ = clock.timeout(rest, forwards.wait_for(conversation, &unsettled)).await;
        forwards.ports_of(conversation)
    };
    let refusals: Vec<String> = missing
        .iter()
        .filter(|port| !ports.contains(port))
        .filter_map(|port| forwards.unreached(conversation, *port))
        .collect();
    if !refusals.is_empty() {
        tracing::debug!(%conversation, ports = ?missing, "a line names a port of a job that efrd does not forward");
    }
    let refusal = (!refusals.is_empty()).then(|| refusals.join("\n"));
    LineReach { ports, refusal }
}

#[cfg(test)]
mod tests;
