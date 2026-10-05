//! The client: one connection to the daemon, hello, calls, streams and cancellation.
//!
//! A [`Client`] owns a connection run by two background tasks. The writer sends frames
//! that callers queue, already encoded, so a request too large for a frame fails at
//! the caller instead of breaking the connection. The reader routes every server frame
//! by its request id to the request's own bounded queue. A consumer that falls behind
//! is not buffered without limit: its request is cancelled and its stream ends with
//! [`ClientError::StreamOverflow`] after the items already queued, mirroring what the
//! daemon does to a slow subscriber.
//!
//! The connection closes when the [`Client`] and every [`ItemStream`] are dropped:
//! the writer then shuts down its side, and the daemon cancels whatever was still in
//! flight.

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use efr_protocol::framing;
use efr_protocol::{
    Capabilities, ClientFrame, ErrorBody, ErrorCode, ErrorFrame, Hello, HelloResult, Method,
    Origin, PROTOCOL_VERSION, ProtocolError, RequestId, ServerFrame,
};
use efr_stdx::time::Clock;
use futures::{Stream, StreamExt as _};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, BufWriter};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio_util::codec::FramedRead;
use zeroize::Zeroizing;

use crate::codec::ClientCodec;
use crate::{ClientError, unix};

/// How long connecting and hello may take unless the options say otherwise.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How many encoded frames wait for the writer before callers wait.
const OUTBOUND_QUEUE_FRAMES: usize = 64;

/// How many frames of one request wait for its consumer before the request is
/// cancelled. Four times the daemon's subscriber queue, so a consumer that keeps up
/// with the daemon never overflows here.
pub(crate) const STREAM_QUEUE_FRAMES: usize = 256;

/// How to connect: the hello to send, the clock for timeouts and the timeout.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    hello: Hello,
    clock: Arc<dyn Clock>,
    timeout: Duration,
}

impl ConnectOptions {
    /// Options for a client of kind `origin`, with this process's pid in hello and a
    /// five second timeout for connecting and for hello, measured on `clock`.
    pub fn new(origin: Origin, clock: Arc<dyn Clock>) -> Self {
        ConnectOptions {
            hello: Hello {
                protocol: PROTOCOL_VERSION,
                origin,
                client: None,
                capabilities: Capabilities::default(),
                tty: None,
                pid: Some(std::process::id()),
                device_id: None,
            },
            clock,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Names the client program and its version in hello, such as `efr 0.1.0`.
    #[must_use]
    pub fn with_client(mut self, client: impl Into<String>) -> Self {
        self.hello.client = Some(client.into());
        self
    }

    /// Names the client's terminal in hello.
    #[must_use]
    pub fn with_tty(mut self, tty: impl Into<String>) -> Self {
        self.hello.tty = Some(tty.into());
        self
    }

    /// Announces what the client supports.
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.hello.capabilities = capabilities;
        self
    }

    /// Sets the timeout for connecting and, separately, for hello.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The hello that [`Client::connect`] sends. Its `protocol` is always this
    /// client's [`PROTOCOL_VERSION`].
    pub fn hello(&self) -> &Hello {
        &self.hello
    }
}

/// A connection to the daemon that has completed hello.
///
/// Requests run concurrently on the one connection: every method takes `&self`, and
/// frames are matched to requests by id.
#[derive(Debug)]
pub struct Client {
    shared: Arc<Shared>,
    hello: HelloResult,
}

impl Client {
    /// Connects to the daemon's socket and says hello.
    ///
    /// Fails with [`ClientError::DaemonNotRunning`] when nothing listens at `socket`,
    /// and with [`ClientError::ProtocolMismatch`] when the daemon speaks another
    /// protocol version, whether the daemon refuses the hello or reports another
    /// version in its answer.
    pub async fn connect(socket: &Path, options: ConnectOptions) -> Result<Client, ClientError> {
        let stream = unix::connect(socket, &options.clock, options.timeout).await?;
        Client::handshake(stream, options).await
    }

    /// Starts the connection's tasks on `stream` and says hello.
    pub(crate) async fn handshake<S>(
        stream: S,
        options: ConnectOptions,
    ) -> Result<Client, ClientError>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read_half, write_half) = tokio::io::split(stream);
        let (outbound, queue) = mpsc::channel(OUTBOUND_QUEUE_FRAMES);
        let pending = Arc::new(Mutex::new(Pending { open: true, routes: HashMap::new() }));
        tokio::spawn(write_frames(write_half, queue));
        let reader = tokio::spawn(read_frames(
            FramedRead::new(read_half, ClientCodec::new()),
            Arc::clone(&pending),
            outbound.downgrade(),
        ))
        .abort_handle();
        let shared = Arc::new(Shared { pending, outbound, next_id: AtomicU64::new(1), reader });

        let ConnectOptions { mut hello, clock, timeout } = options;
        hello.protocol = PROTOCOL_VERSION;
        let answer =
            clock.timeout(timeout, unary::<HelloResult>(&shared, Method::Hello(hello))).await;
        let result = match answer {
            Err(_) => return Err(ClientError::HelloTimedOut { after: timeout }),
            Ok(Err(ClientError::Server { body })) => return Err(refused_hello(body)),
            Ok(Err(error)) => return Err(error),
            Ok(Ok(result)) => result,
        };
        if result.protocol != PROTOCOL_VERSION {
            return Err(ClientError::ProtocolMismatch {
                daemon: result.protocol,
                client: PROTOCOL_VERSION,
            });
        }
        Ok(Client { shared, hello: result })
    }

    /// The daemon's answer to hello: its identity, version, capabilities and paths.
    pub fn hello(&self) -> &HelloResult {
        &self.hello
    }

    /// Runs a unary method and returns its result as `R`, such as
    /// `ConversationsListResult` for `conversations.list`.
    ///
    /// Dropping the future cancels the request.
    pub async fn call<R: DeserializeOwned>(&self, method: Method) -> Result<R, ClientError> {
        if method.is_stream() {
            return Err(ClientError::NotUnary { method: method.name() });
        }
        unary(&self.shared, method).await
    }

    /// Starts a request and returns its items as a stream of `I`, such as
    /// `ConversationSubscribeItem` for `conversation.subscribe`.
    ///
    /// The stream ends after the daemon's end frame, or with one error item. Dropping
    /// it before then cancels the request.
    pub async fn stream<I: DeserializeOwned>(
        &self,
        method: Method,
    ) -> Result<ItemStream<I>, ClientError> {
        let method_name = method.name();
        let started = start(&self.shared, method).await?;
        Ok(ItemStream {
            method: method_name,
            guard: started.guard,
            replies: started.replies,
            overflowed: started.overflowed,
            done: false,
            item: PhantomData,
        })
    }

    /// Asks the daemon to cancel request `id`. The request then ends with a
    /// `cancelled` error, or not at all when it had already ended.
    pub async fn cancel(&self, id: RequestId) -> Result<(), ClientError> {
        self.shared.cancel(id).await
    }

    /// True once the connection has closed; every request then fails with
    /// [`ClientError::Closed`].
    pub fn is_closed(&self) -> bool {
        !self.shared.lock_pending().open
    }
}

/// The items of one request, decoded as `I`.
///
/// Yields each item, then ends after the daemon's end frame. An error from the daemon,
/// a closed connection or a local overflow is the last item.
pub struct ItemStream<I> {
    method: &'static str,
    guard: RequestGuard,
    replies: mpsc::Receiver<Reply>,
    overflowed: Arc<AtomicBool>,
    done: bool,
    item: PhantomData<fn() -> I>,
}

impl<I> ItemStream<I> {
    /// The request's id.
    pub fn id(&self) -> RequestId {
        self.guard.id
    }

    /// Asks the daemon to cancel the request. The stream then ends with a `cancelled`
    /// error, unless the request had already ended.
    pub async fn cancel(&self) -> Result<(), ClientError> {
        self.guard.shared.cancel(self.guard.id).await
    }
}

impl<I: DeserializeOwned> Stream for ItemStream<I> {
    type Item = Result<I, ClientError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        let last = match ready!(this.replies.poll_recv(cx)) {
            Some(Reply::Item(value)) => {
                let item = serde_json::from_value(value)
                    .map_err(|source| ClientError::DecodeItem { method: this.method, source });
                return Poll::Ready(Some(item));
            }
            Some(Reply::End) => None,
            Some(Reply::Error(body)) => Some(Err(ClientError::Server { body })),
            Some(Reply::Malformed(source)) => Some(Err(ClientError::Protocol { source })),
            None => Some(Err(lost(this.guard.id, &this.overflowed))),
        };
        this.done = true;
        this.guard.cancel_on_drop = false;
        Poll::Ready(last)
    }
}

impl<I> fmt::Debug for ItemStream<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ItemStream")
            .field("id", &self.guard.id)
            .field("method", &self.method)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// What the reader hands a request.
#[derive(Debug)]
enum Reply {
    Item(Value),
    End,
    Error(ErrorBody),
    Malformed(ProtocolError),
}

/// Where the reader sends one request's frames.
#[derive(Debug)]
struct Route {
    replies: mpsc::Sender<Reply>,
    /// Set when the consumer fell behind and the request was cancelled locally.
    overflowed: Arc<AtomicBool>,
}

/// The requests in flight, by id.
#[derive(Debug)]
struct Pending {
    /// False once the connection closed; no request can start after that.
    open: bool,
    routes: HashMap<RequestId, Route>,
}

/// What a [`Client`] and its streams share.
#[derive(Debug)]
struct Shared {
    pending: Arc<Mutex<Pending>>,
    outbound: mpsc::Sender<Frame>,
    next_id: AtomicU64,
    reader: AbortHandle,
}

impl Shared {
    fn lock_pending(&self) -> MutexGuard<'_, Pending> {
        lock(&self.pending)
    }

    fn forget(&self, id: RequestId) {
        self.lock_pending().routes.remove(&id);
    }

    async fn cancel(&self, id: RequestId) -> Result<(), ClientError> {
        let frame = encode(&ClientFrame::Cancel { id })?;
        self.outbound.send(frame).await.map_err(|_| ClientError::Closed)
    }

    /// Queues a cancel without waiting, for a destructor.
    fn cancel_now(&self, id: RequestId) {
        if let Ok(frame) = encode(&ClientFrame::Cancel { id }) {
            let _ = self.outbound.try_send(frame);
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // Nothing can receive what the reader routes any more.
        self.reader.abort();
    }
}

/// Forgets a request when its caller goes away, and cancels it on the daemon when it
/// had not ended yet.
#[derive(Debug)]
struct RequestGuard {
    shared: Arc<Shared>,
    id: RequestId,
    cancel_on_drop: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.shared.forget(self.id);
        if self.cancel_on_drop {
            self.shared.cancel_now(self.id);
        }
    }
}

/// A request that has been sent.
struct Started {
    guard: RequestGuard,
    replies: mpsc::Receiver<Reply>,
    overflowed: Arc<AtomicBool>,
}

/// Registers a route for a new request id and sends the request.
async fn start(shared: &Arc<Shared>, method: Method) -> Result<Started, ClientError> {
    let id = RequestId::new(shared.next_id.fetch_add(1, Ordering::Relaxed));
    let frame = encode(&ClientFrame::Request { id, method })?;
    let (sender, replies) = mpsc::channel(STREAM_QUEUE_FRAMES);
    let overflowed = Arc::new(AtomicBool::new(false));
    {
        let mut pending = shared.lock_pending();
        if !pending.open {
            return Err(ClientError::Closed);
        }
        pending.routes.insert(id, Route { replies: sender, overflowed: Arc::clone(&overflowed) });
    }
    let mut guard = RequestGuard { shared: Arc::clone(shared), id, cancel_on_drop: false };
    shared.outbound.send(frame).await.map_err(|_| ClientError::Closed)?;
    guard.cancel_on_drop = true;
    Ok(Started { guard, replies, overflowed })
}

/// Runs a unary request: one item, then the end frame.
async fn unary<R: DeserializeOwned>(
    shared: &Arc<Shared>,
    method: Method,
) -> Result<R, ClientError> {
    let method_name = method.name();
    let Started { mut guard, mut replies, overflowed } = start(shared, method).await?;
    let mut result = None;
    let outcome = loop {
        match replies.recv().await {
            Some(Reply::Item(value)) => {
                // A unary method sends one item; anything after the first is ignored.
                result.get_or_insert(value);
            }
            Some(Reply::End) => {
                break match result {
                    Some(value) => serde_json::from_value(value)
                        .map_err(|source| ClientError::DecodeItem { method: method_name, source }),
                    None => Err(ClientError::MissingResult { method: method_name }),
                };
            }
            Some(Reply::Error(body)) => break Err(ClientError::Server { body }),
            Some(Reply::Malformed(source)) => break Err(ClientError::Protocol { source }),
            None => break Err(lost(guard.id, &overflowed)),
        }
    };
    guard.cancel_on_drop = false;
    outcome
}

/// The error for a request whose queue closed without an end frame.
fn lost(id: RequestId, overflowed: &AtomicBool) -> ClientError {
    if overflowed.load(Ordering::Acquire) {
        ClientError::StreamOverflow { id }
    } else {
        ClientError::Closed
    }
}

/// The error for a refused hello: a protocol mismatch carries both versions in its data.
fn refused_hello(body: ErrorBody) -> ClientError {
    let version = |key: &str| {
        let data = body.data.as_ref()?;
        u32::try_from(data.get(key)?.as_u64()?).ok()
    };
    match (body.code, version("daemon"), version("client")) {
        (ErrorCode::ProtocolMismatch, Some(daemon), Some(client)) => {
            ClientError::ProtocolMismatch { daemon, client }
        }
        _ => ClientError::Server { body },
    }
}

fn lock(pending: &Mutex<Pending>) -> MutexGuard<'_, Pending> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An encoded client frame. A request can carry a password typed for `input.respond`,
/// so the buffer is overwritten with zeros when it is dropped: once it is written, or
/// when the connection closes before it is.
type Frame = Zeroizing<Vec<u8>>;

/// Encodes `frame` behind its length prefix into a [`Frame`].
fn encode(frame: &ClientFrame) -> Result<Frame, ProtocolError> {
    framing::encode(frame).map(Zeroizing::new)
}

/// Writes queued frames until every sender is gone, then shuts down the write side so
/// the daemon sees the client leave.
async fn write_frames<W: AsyncWrite + Unpin>(write_half: W, mut queue: mpsc::Receiver<Frame>) {
    let mut writer = BufWriter::new(write_half);
    let written: std::io::Result<()> = async {
        while let Some(frame) = queue.recv().await {
            writer.write_all(&frame).await?;
            while let Ok(frame) = queue.try_recv() {
                writer.write_all(&frame).await?;
            }
            writer.flush().await?;
        }
        writer.shutdown().await
    }
    .await;
    // A failed write leaves nothing to do: callers see the queue close, and the reader
    // sees the connection end.
    drop(written);
}

/// Routes server frames to their requests until the connection ends, then fails every
/// request still in flight.
async fn read_frames<R: AsyncRead + Unpin>(
    mut frames: FramedRead<R, ClientCodec>,
    pending: Arc<Mutex<Pending>>,
    outbound: mpsc::WeakSender<Frame>,
) {
    while let Some(next) = frames.next().await {
        match next {
            Ok(Ok(frame)) => route(&pending, &outbound, frame),
            Ok(Err(ProtocolError::Decode { id: Some(id), source })) => {
                finish(
                    &pending,
                    id,
                    Reply::Malformed(ProtocolError::Decode { id: Some(id), source }),
                );
            }
            // A frame that names no request may have been any request's end, so no
            // request can trust the connection any more.
            Ok(Err(_)) | Err(_) => break,
        }
    }
    let mut pending = lock(&pending);
    pending.open = false;
    pending.routes.clear();
}

fn route(pending: &Mutex<Pending>, outbound: &mpsc::WeakSender<Frame>, frame: ServerFrame) {
    match frame {
        ServerFrame::Item { id, item } => {
            let mut pending = lock(pending);
            let Some(route) = pending.routes.get(&id) else {
                return;
            };
            // NOTE: one slot stays free for the end frame, so a request that ends right
            // after a burst still sees how it ended.
            if route.replies.capacity() > 1 {
                if route.replies.try_send(Reply::Item(item)).is_err() {
                    pending.routes.remove(&id);
                }
                return;
            }
            route.overflowed.store(true, Ordering::Release);
            pending.routes.remove(&id);
            drop(pending);
            if let (Some(outbound), Ok(frame)) =
                (outbound.upgrade(), encode(&ClientFrame::Cancel { id }))
            {
                let _ = outbound.try_send(frame);
            }
        }
        ServerFrame::End { id } => finish(pending, id, Reply::End),
        ServerFrame::Error(ErrorFrame { id: Some(id), error }) => {
            finish(pending, id, Reply::Error(error));
        }
        ServerFrame::Error(ErrorFrame { id: None, error }) => {
            // The daemon closes the connection after an error without an id, so every
            // request ends with it.
            let mut pending = lock(pending);
            pending.open = false;
            for (_, route) in pending.routes.drain() {
                let _ = route.replies.try_send(Reply::Error(error.clone()));
            }
        }
        // Acks carry nothing a caller needs; frame shapes added later are skipped.
        _ => {}
    }
}

/// Ends request `id` with `reply`.
fn finish(pending: &Mutex<Pending>, id: RequestId, reply: Reply) {
    if let Some(route) = lock(pending).routes.remove(&id) {
        let _ = route.replies.try_send(reply);
    }
}

#[cfg(test)]
mod tests;
