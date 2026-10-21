//! The tasks of one job's forwards: the control connection, the forwarder's data
//! connections, a listener for each forwarded port and the copy of each host
//! connection. Every task ends when the job's token is cancelled.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use efr_protocol::{Event, ForwardOutcome, JobId, PortForward};
use efr_sandbox::{
    FORWARD_VERSION, FRAME_LEN, Frame, FrameKind, Listen, MAX_DAEMON_FORWARDS,
    MAX_FORWARD_CONNECTIONS, SandboxError, peer_uid, refusal, v6_holds,
};
use efr_stdx::time::Clock;
use efr_store::Batch;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{ForwardSocket, Forwards, HELLO_WAIT, JobForward, OFF_REASON, OPEN_WAIT, Taken};
use crate::jobs::{JobKey, JobTable};

/// The most refusals of one job that efrd records. The forwarder reports a port again
/// after it was gone, which efrd records once, and a forwarder that the job took over
/// could report many ports.
const MAX_REFUSALS: usize = 32;

/// The most data connections of one job that wait for their `Data` at one time.
const MAX_WAITING_DATA: usize = 64;

/// The room of the queue of `Open` frames for the control connection.
const OPENS_ROOM: usize = 64;

/// How long a listener waits after an accept failed, such as at the limit of open
/// files, before it accepts again.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// Starts the forwards of the job `key` of `table` on its bound `socket`. The job must be
/// in the table. Dropping the handle ends them.
pub(crate) fn serve(table: JobTable, key: JobKey, socket: ForwardSocket) -> JobForward {
    let forwards = table.inner.forwards.clone();
    let token = CancellationToken::new();
    let off = CancellationToken::new();
    let (opens, opens_rx) = mpsc::channel(OPENS_ROOM);
    let ForwardSocket { listener, plan, generation } = socket;
    let ctx = Arc::new(Ctx {
        clock: Arc::clone(&table.inner.clock),
        table,
        key,
        job: JobId::nth(key.number),
        forwards: forwards.clone(),
        token: token.clone(),
        proxy: plan.proxy,
        max_ports: usize::from(plan.max_ports),
        generation,
        ports: Mutex::new(BTreeMap::new()),
        pending: Mutex::new(Pending::default()),
        opens,
        connections: Arc::new(Semaphore::new(MAX_FORWARD_CONNECTIONS)),
        warned: Mutex::new(BTreeSet::new()),
    });
    let control = Control {
        ctx,
        listener,
        off: off.clone(),
        opens: Some(opens_rx),
        recorded: BTreeSet::new(),
        refusals: 0,
        limit_told: false,
    };
    tokio::spawn(control.run());
    JobForward { key, forwards, token, off }
}

/// What every task of one job's forwards shares.
struct Ctx {
    table: JobTable,
    clock: Arc<dyn Clock>,
    key: JobKey,
    job: JobId,
    forwards: Forwards,
    /// Cancelled when the job's forwards end.
    token: CancellationToken,
    /// True in a proxy job: port 3128 is the bridge's.
    proxy: bool,
    max_ports: usize,
    /// [`Forwards::generation`] before the job read its settings.
    generation: u64,
    /// The forwarded ports.
    ports: Mutex<BTreeMap<u16, Port>>,
    pending: Mutex<Pending>,
    /// The `Open` frames for the control connection.
    opens: mpsc::Sender<Frame>,
    /// The host connections of the job that run.
    connections: Arc<Semaphore>,
    /// The ports whose refused peer efrd logged.
    warned: Mutex<BTreeSet<u16>>,
}

/// A forwarded port.
#[derive(Debug, Clone, Copy)]
struct Port {
    /// The address that the forwarder connects to.
    addr: IpAddr,
    /// False after `Gone`, until the next `Listen`: a connection then closes at once.
    live: bool,
}

/// The host connections that wait for the forwarder's `Data`.
#[derive(Default)]
struct Pending {
    next: u32,
    waiting: HashMap<u32, oneshot::Sender<(u8, UnixStream)>>,
}

/// Locks `mutex`; each change is whole, so a poisoned lock still holds usable data.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Ctx {
    /// Records `event` of the job's conversation.
    async fn record(&self, event: Event) {
        let batch = Batch::new().event(self.key.conversation, event);
        if let Err(error) = self.table.inner.writer.append(batch).await {
            tracing::warn!(%error, job = %self.job, "a port forward of a job could not be recorded");
        }
    }

    /// Marks the forwarded port of `target` gone: it stays the job's (DL11).
    fn gone(&self, target: Listen) {
        if let Some(port) = locked(&self.ports).get_mut(&target.port) {
            port.live = false;
        }
    }

    /// A new id for an `Open`, and the answer of its `Data`.
    fn wait_data(&self) -> (u32, oneshot::Receiver<(u8, UnixStream)>) {
        let mut pending = locked(&self.pending);
        pending.next = pending.next.wrapping_add(1).max(1);
        let id = pending.next;
        let (send, answer) = oneshot::channel();
        pending.waiting.insert(id, send);
        (id, answer)
    }

    /// True when the host process that connected from `peer` to efrd's listener on
    /// `127.0.0.1:local` runs as efrd's own user (DL9).
    async fn peer_is_own(&self, local: u16, peer: SocketAddr) -> bool {
        let host = self.forwards.ports();
        let rows = tokio::task::spawn_blocking(move || {
            let (mut rows, v6) = host.rows()?;
            rows.extend(v6);
            Ok::<_, std::io::Error>((rows, host.own_uid()))
        })
        .await;
        match rows {
            Ok(Ok((rows, own))) => peer_uid(&rows, local, peer.port()) == Some(own),
            Ok(Err(error)) => {
                tracing::warn!(%error, "the host's tcp table could not be read; a forwarded connection closes");
                false
            }
            Err(_) => false,
        }
    }

    /// One host connection to the forwarded `port`, whose listener is on `local`: the
    /// peer check, the `Open`, the wait for `Data` and the copy.
    async fn connect(&self, port: u16, local: u16, mut tcp: TcpStream, peer: SocketAddr) {
        if !self.peer_is_own(local, peer).await {
            if locked(&self.warned).insert(port) {
                tracing::warn!(job = %self.job, port, "a process of another user or an unknown process connected to a forwarded port; efrd closed the connection");
            }
            return;
        }
        let addr = match locked(&self.ports).get(&port) {
            Some(known) if known.live => known.addr,
            _ => return,
        };
        let (id, answer) = self.wait_data();
        if self.opens.send(Frame::open(id, Listen { addr, port })).await.is_err() {
            locked(&self.pending).waiting.remove(&id);
            return;
        }
        let answer = self.clock.timeout(OPEN_WAIT, answer).await;
        let (status, mut unix) = match answer {
            Ok(Ok(answer)) => answer,
            _ => {
                locked(&self.pending).waiting.remove(&id);
                tracing::debug!(job = %self.job, port, "the forwarder did not answer an open in time");
                return;
            }
        };
        if status != 0 {
            tracing::debug!(job = %self.job, port, status, "the forwarder could not connect in the job");
            return;
        }
        let copied = tokio::io::copy_bidirectional(&mut tcp, &mut unix).await;
        tracing::trace!(job = %self.job, port, ?copied, "a forwarded connection ended");
    }
}

/// Why efrd does not forward a port.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refused {
    outcome: ForwardOutcome,
    by_job: Option<JobId>,
    reason: Option<String>,
    /// True for the limit of the job, which efrd records once for each job.
    job_limit: bool,
}

impl Refused {
    fn in_use(by_job: Option<JobId>) -> Refused {
        Refused { outcome: ForwardOutcome::InUse, by_job, reason: None, job_limit: false }
    }

    fn never(reason: &str) -> Refused {
        Refused {
            outcome: ForwardOutcome::Refused,
            by_job: None,
            reason: Some(reason.to_owned()),
            job_limit: false,
        }
    }

    fn limit(reason: String, job_limit: bool) -> Refused {
        Refused { outcome: ForwardOutcome::Limit, by_job: None, reason: Some(reason), job_limit }
    }
}

/// The task of the control connection, which owns the forward socket.
struct Control {
    ctx: Arc<Ctx>,
    listener: UnixListener,
    /// Cancelled when a reload turned `jobs.forward_ports` off.
    off: CancellationToken,
    opens: Option<mpsc::Receiver<Frame>>,
    /// The ports whose refusal efrd recorded.
    recorded: BTreeSet<u16>,
    refusals: usize,
    /// True once efrd recorded the limit of the job.
    limit_told: bool,
}

impl Control {
    async fn run(mut self) {
        let ctx = Arc::clone(&self.ctx);
        let hello = tokio::select! {
            () = ctx.token.cancelled() => None,
            () = self.off.cancelled() => None,
            hello = ctx.clock.timeout(HELLO_WAIT, first_hello(&self.listener)) => match hello {
                Ok(Ok(stream)) => Some(stream),
                Ok(Err(problem)) => {
                    tracing::warn!(conversation = %ctx.key.conversation, job = %ctx.job, problem, "a job's forwarder did not say hello; the job has no port forwards");
                    None
                }
                Err(_) => {
                    tracing::warn!(conversation = %ctx.key.conversation, job = %ctx.job, "no forwarder of a job said hello in 5 s; the job has no port forwards");
                    None
                }
            },
        };
        if let Some(stream) = hello {
            self.serve(stream).await;
        }
        self.close();
    }

    /// Reads the frames of the control connection `stream` and accepts the data
    /// connections, until the job's forwards end.
    async fn serve(&mut self, stream: UnixStream) {
        let ctx = Arc::clone(&self.ctx);
        let (read, write) = stream.into_split();
        let (frames_tx, mut frames) = mpsc::channel(16);
        tokio::spawn(read_frames(read, frames_tx, ctx.token.clone()));
        if let Some(opens) = self.opens.take() {
            tokio::spawn(write_opens(write, opens, ctx.token.clone()));
        }
        let waiting = Arc::new(Semaphore::new(MAX_WAITING_DATA));
        loop {
            tokio::select! {
                () = ctx.token.cancelled() => return,
                () = self.off.cancelled() => {
                    self.stop_all().await;
                    return;
                }
                frame = frames.recv() => match frame {
                    Some(Ok(frame)) if frame.kind == FrameKind::Listen => {
                        self.listen(frame.target()).await;
                    }
                    Some(Ok(frame)) if frame.kind == FrameKind::Gone => ctx.gone(frame.target()),
                    Some(Ok(_)) => {
                        tracing::warn!(job = %ctx.job, "a job's forwarder sent a frame of another kind; efrd closed its forwards");
                        return;
                    }
                    Some(Err(error)) => {
                        tracing::warn!(job = %ctx.job, %error, "a job's forwarder sent a bad frame; efrd closed its forwards");
                        return;
                    }
                    None => {
                        tracing::debug!(job = %ctx.job, "a job's forwarder closed its connection");
                        return;
                    }
                },
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        if let Ok(permit) = Arc::clone(&waiting).try_acquire_owned() {
                            tokio::spawn(data_connection(Arc::clone(&ctx), stream, permit));
                        }
                    }
                    Err(error) => {
                        tracing::warn!(job = %ctx.job, %error, "a job's forward socket failed; efrd closed its forwards");
                        return;
                    }
                },
            }
        }
    }

    /// A `Listen` of the forwarder: the server of a port that the job holds is back, or
    /// a new port passes efrd's checks and is forwarded, or it is not, with its event.
    async fn listen(&mut self, target: Listen) {
        let ctx = Arc::clone(&self.ctx);
        if ctx.forwards.generation() != ctx.generation {
            tracing::debug!(job = %ctx.job, port = target.port, "forwards are off for this job");
            return;
        }
        let moved = match locked(&ctx.ports).get_mut(&target.port) {
            Some(port) => {
                let moved = port.addr != target.addr;
                *port = Port { addr: target.addr, live: true };
                Some(moved)
            }
            None => None,
        };
        match moved {
            Some(true) => {
                let listen = target.addr.to_string();
                ctx.table.update(ctx.key, |info| {
                    for forward in info.forwards.iter_mut().filter(|f| f.port == target.port) {
                        forward.listen = listen.clone();
                    }
                });
                return;
            }
            Some(false) => return,
            None => {}
        }
        match self.check(target).await {
            Ok(listener) => self.forwarded(target, listener).await,
            Err(refused) => self.refused(target, refused).await,
        }
    }

    /// efrd's checks of a new port (DL5, DL8) and the bind of the host port.
    async fn check(&self, target: Listen) -> Result<TcpListener, Refused> {
        let ctx = &self.ctx;
        let port = target.port;
        if let Some(why) = refusal(port, ctx.proxy) {
            return Err(Refused::never(why.reason()));
        }
        if locked(&ctx.ports).len() >= ctx.max_ports {
            let reason = format!("the job forwards {} ports already", ctx.max_ports);
            return Err(Refused::limit(reason, true));
        }
        match ctx.forwards.reserve(ctx.key, port) {
            Ok(()) => {}
            Err(Taken::By(other)) => {
                let by =
                    (other.conversation == ctx.key.conversation).then(|| JobId::nth(other.number));
                return Err(Refused::in_use(by));
            }
            Err(Taken::Full) => {
                let reason = format!("efrd forwards {MAX_DAEMON_FORWARDS} ports already");
                return Err(Refused::limit(reason, false));
            }
        }
        let host = ctx.forwards.ports();
        let bound = tokio::task::spawn_blocking(move || {
            let (_, v6) = host.rows()?;
            if v6_holds(&v6, port) {
                return Ok(None);
            }
            host.bind(port).map(Some)
        })
        .await;
        let listener = match bound {
            Ok(Ok(Some(listener))) => TcpListener::from_std(listener),
            Ok(Ok(None)) => Err(std::io::Error::from(std::io::ErrorKind::AddrInUse)),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(std::io::Error::other("the bind of a forwarded port panicked")),
        };
        listener.map_err(|error| {
            ctx.forwards.release(ctx.key, port);
            if error.kind() != std::io::ErrorKind::AddrInUse {
                tracing::warn!(%error, job = %ctx.job, port, "a forwarded port could not be bound; efrd counts it in use");
            }
            Refused::in_use(None)
        })
    }

    async fn forwarded(&self, target: Listen, listener: TcpListener) {
        let ctx = Arc::clone(&self.ctx);
        let port = target.port;
        let local = listener.local_addr().map_or(port, |addr| addr.port());
        locked(&ctx.ports).insert(port, Port { addr: target.addr, live: true });
        let listen = target.addr.to_string();
        let forward = PortForward { listen: listen.clone(), port };
        ctx.table.update(ctx.key, |info| info.forwards.push(forward));
        tokio::spawn(accept(Arc::clone(&ctx), port, local, listener));
        tracing::info!(conversation = %ctx.key.conversation, job = %ctx.job, port, "a job's port is forwarded to 127.0.0.1");
        ctx.record(Event::JobPortForwarded {
            job_id: ctx.job.clone(),
            listen,
            port,
            outcome: ForwardOutcome::Forwarded,
            by_job: None,
            reason: None,
        })
        .await;
    }

    async fn refused(&mut self, target: Listen, refused: Refused) {
        let ctx = Arc::clone(&self.ctx);
        let port = target.port;
        tracing::debug!(job = %ctx.job, port, outcome = refused.outcome.as_str(), "a job's port is not forwarded");
        if refused.job_limit && self.limit_told {
            return;
        }
        if self.refusals >= MAX_REFUSALS || !self.recorded.insert(port) {
            return;
        }
        self.refusals += 1;
        self.limit_told |= refused.job_limit;
        ctx.record(Event::JobPortForwarded {
            job_id: ctx.job.clone(),
            listen: target.addr.to_string(),
            port,
            outcome: refused.outcome,
            by_job: refused.by_job,
            reason: refused.reason,
        })
        .await;
    }

    /// Ends every forward of the job with a `stopped` event for each port, after a
    /// reload turned `jobs.forward_ports` off.
    async fn stop_all(&mut self) {
        let ctx = Arc::clone(&self.ctx);
        let ports = std::mem::take(&mut *locked(&ctx.ports));
        self.close();
        if ports.is_empty() {
            return;
        }
        tracing::info!(job = %ctx.job, ports = ports.len(), "jobs.forward_ports is off; efrd stopped the forwards of a job");
        for (port, known) in ports {
            ctx.record(Event::JobPortForwarded {
                job_id: ctx.job.clone(),
                listen: known.addr.to_string(),
                port,
                outcome: ForwardOutcome::Stopped,
                by_job: None,
                reason: Some(OFF_REASON.to_owned()),
            })
            .await;
        }
    }

    /// Closes every listener and connection of the job and frees its ports.
    fn close(&self) {
        let ctx = &self.ctx;
        ctx.token.cancel();
        ctx.forwards.release_job(ctx.key);
        locked(&ctx.ports).clear();
        ctx.table.update(ctx.key, |info| info.forwards.clear());
    }
}

/// The first connection to the forward socket, once it sent a `Hello` of this version.
async fn first_hello(listener: &UnixListener) -> Result<UnixStream, &'static str> {
    let (mut stream, _) =
        listener.accept().await.map_err(|_| "the forward socket could not accept")?;
    let mut bytes = [0u8; FRAME_LEN];
    stream.read_exact(&mut bytes).await.map_err(|_| "its connection closed before a frame")?;
    match Frame::decode(&bytes) {
        Ok(frame) if frame.kind == FrameKind::Hello && frame.status == FORWARD_VERSION => {
            Ok(stream)
        }
        Ok(_) => Err("its first frame was no hello of version 1"),
        Err(_) => Err("its first frame was bad"),
    }
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

/// Writes each `Open` of `opens` to the control connection.
async fn write_opens(
    mut write: OwnedWriteHalf,
    mut opens: mpsc::Receiver<Frame>,
    token: CancellationToken,
) {
    loop {
        let frame = tokio::select! {
            () = token.cancelled() => return,
            frame = opens.recv() => match frame {
                Some(frame) => frame,
                None => return,
            },
        };
        let bytes = frame.encode();
        let written = tokio::select! {
            () = token.cancelled() => return,
            written = write.write_all(&bytes) => written,
        };
        if written.is_err() {
            return;
        }
    }
}

/// A data connection of the forwarder: its first frame must be the `Data` of an `Open`
/// that waits.
async fn data_connection(ctx: Arc<Ctx>, mut stream: UnixStream, permit: OwnedSemaphorePermit) {
    let mut bytes = [0u8; FRAME_LEN];
    let read = tokio::select! {
        () = ctx.token.cancelled() => return,
        read = ctx.clock.timeout(OPEN_WAIT, stream.read_exact(&mut bytes)) => read,
    };
    drop(permit);
    if !matches!(read, Ok(Ok(_))) {
        return;
    }
    let frame = match Frame::decode(&bytes) {
        Ok(frame) if frame.kind == FrameKind::Data => frame,
        _ => {
            tracing::debug!(job = %ctx.job, "a data connection of a forwarder sent no data frame");
            return;
        }
    };
    let waiter = locked(&ctx.pending).waiting.remove(&frame.id);
    if let Some(waiter) = waiter {
        let _ = waiter.send((frame.status, stream));
    }
}

/// Accepts the host connections of the forwarded `port`, whose listener is on `local`.
async fn accept(ctx: Arc<Ctx>, port: u16, local: u16, listener: TcpListener) {
    loop {
        let accepted = tokio::select! {
            () = ctx.token.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, peer)) => {
                tokio::spawn(connection(Arc::clone(&ctx), port, local, stream, peer));
            }
            Err(error) => {
                tracing::warn!(%error, job = %ctx.job, port, "a forwarded port could not accept");
                tokio::select! {
                    () = ctx.token.cancelled() => return,
                    () = ctx.clock.sleep(ACCEPT_PAUSE) => {}
                }
            }
        }
    }
}

/// One host connection, within the job's limit of connections.
async fn connection(ctx: Arc<Ctx>, port: u16, local: u16, stream: TcpStream, peer: SocketAddr) {
    let Ok(permit) = Arc::clone(&ctx.connections).try_acquire_owned() else {
        tracing::debug!(job = %ctx.job, port, "a job has as many forwarded connections as it may");
        return;
    };
    tokio::select! {
        () = ctx.token.cancelled() => {}
        () = ctx.connect(port, local, stream, peer) => {}
    }
    drop(permit);
}
