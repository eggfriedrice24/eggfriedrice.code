//! One connection: the hello gate, the request-id table, cancellation, and cleanup on
//! close.
//!
//! A connection runs as one reader loop plus one writer task:
//!
//! - The reader decodes client frames. It answers `hello` itself (through
//!   [`Dispatcher::hello`]) before it reads the next frame, so nothing runs before the
//!   version check. Every other request gets an entry in the request-id table and a task
//!   of its own; a cancel frame cancels that entry's token.
//! - Each request task races [`Dispatcher::dispatch`] against its token, removes its
//!   entry, ends the request's response state and then queues the one frame that ends
//!   the request. A handler may have moved its responder into a task that outlives it,
//!   but every frame is queued under the state's lock and fails once the state has
//!   ended, so no item can follow the end frame. An id is free again by the time the
//!   client reads that frame.
//! - The writer drains a bounded queue of encoded frames to the socket. A client that
//!   reads slowly fills the queue and slows its own requests, nothing else.
//!
//! When the client closes its side, the stream breaks, or the daemon shuts down, every
//! request still in flight is cancelled and its handler future dropped, so a dead
//! client never pins a subscription. The dispatcher then hears of the close through
//! [`Dispatcher::closed`], if it accepted a hello, and the writer gets a short grace
//! period, on the injected clock, to flush what is queued.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use efr_protocol::{
    ClientFrame, ErrorBody, ErrorCode, Hello, Method, PROTOCOL_VERSION, ProtocolError, RequestId,
    ServerFrame,
};
use efr_stdx::time::Clock;
use futures::StreamExt as _;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, BufWriter};
use tokio::sync::mpsc;
use tokio::task::{self, JoinError, JoinSet};
use tokio_util::codec::FramedRead;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::codec::{EncodedFrame, ServerCodec};
use crate::dispatch::{Request, Responder, ResponseState, cancelled, terminal_frame};
use crate::hello::{Gate, gate};
use crate::{ConnId, ConnectionContext, Dispatcher, PeerCred, TransportError};

/// How many encoded frames wait for the writer before senders wait.
const OUTBOUND_QUEUE_FRAMES: usize = 64;

/// How long a closing connection may spend flushing queued frames to a peer that does
/// not read them.
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// The requests in flight on one connection, by the client's id.
///
/// The lock is a std mutex held for single map operations only, never across an await.
#[derive(Debug, Default)]
pub(crate) struct RequestTable {
    requests: Mutex<HashMap<RequestId, CancellationToken>>,
}

impl RequestTable {
    /// Adds a request. Returns `false`, and changes nothing, when `id` is in flight.
    pub(crate) fn insert(&self, id: RequestId, token: CancellationToken) -> bool {
        let mut requests = self.lock();
        if requests.contains_key(&id) {
            return false;
        }
        requests.insert(id, token);
        true
    }

    /// Cancels the request `id`. Returns `false` when it is not in flight, which is not
    /// an error: it may have ended while the cancel was on its way.
    pub(crate) fn cancel(&self, id: RequestId) -> bool {
        match self.lock().get(&id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Forgets the request `id`, which frees the id for reuse.
    pub(crate) fn remove(&self, id: RequestId) {
        self.lock().remove(&id);
    }

    /// How many requests are in flight.
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RequestId, CancellationToken>> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What a connection needs besides its stream.
#[derive(Debug)]
pub(crate) struct ConnectionParams<D> {
    pub(crate) conn_id: ConnId,
    pub(crate) peer: PeerCred,
    pub(crate) dispatcher: Arc<D>,
    pub(crate) clock: Arc<dyn Clock>,
    /// Cancelled when the daemon shuts down.
    pub(crate) shutdown: CancellationToken,
}

/// Serves one connection until the client closes it, the stream breaks or `shutdown`
/// is cancelled.
pub(crate) async fn run<S, D>(stream: S, params: ConnectionParams<D>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
    D: Dispatcher,
{
    let span = tracing::info_span!(
        "connection",
        conn_id = %params.conn_id,
        uid = params.peer.uid(),
        surface = tracing::field::Empty,
    );
    // The writer task is spawned in `new`, inside the span, so its events carry it too.
    let connection = {
        let _entered = span.enter();
        Connection::new(stream, params)
    };
    connection.serve().instrument(span).await;
}

/// The reader loop's state.
struct Connection<S, D> {
    frames: FramedRead<tokio::io::ReadHalf<S>, ServerCodec>,
    outbound: mpsc::Sender<EncodedFrame>,
    writer: task::JoinHandle<()>,
    params: ConnectionParams<D>,
    /// Cancelled when this connection closes; every request token is a child of it.
    closing: CancellationToken,
    table: Arc<RequestTable>,
    tasks: JoinSet<()>,
    /// The request each running task serves, to answer it when its task panics.
    task_requests: HashMap<task::Id, RequestId>,
    /// Set by a successful hello.
    context: Option<ConnectionContext>,
    /// The context of the last hello the dispatcher accepted, which `closed` reports.
    /// It is set before the answer is sent, so a hello accepted on a connection that
    /// breaks before the answer leaves is still reported.
    accepted: Option<ConnectionContext>,
}

/// Whether the reader loop goes on after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Read,
    Close,
}

impl<S, D> Connection<S, D>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
    D: Dispatcher,
{
    fn new(stream: S, params: ConnectionParams<D>) -> Self {
        let (read_half, write_half) = tokio::io::split(stream);
        let (outbound, queue) = mpsc::channel(OUTBOUND_QUEUE_FRAMES);
        let closing = params.shutdown.child_token();
        let writer =
            tokio::spawn(write_frames(write_half, queue, closing.clone()).in_current_span());
        Connection {
            frames: FramedRead::new(read_half, ServerCodec::new()),
            outbound,
            writer,
            params,
            closing,
            table: Arc::new(RequestTable::default()),
            tasks: JoinSet::new(),
            task_requests: HashMap::new(),
            context: None,
            accepted: None,
        }
    }

    async fn serve(mut self) {
        tracing::debug!("connection opened");
        loop {
            let next = tokio::select! {
                biased;
                () = self.closing.cancelled() => break,
                Some(joined) = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                    self.reap(joined).await;
                    continue;
                }
                next = self.frames.next() => next,
            };
            let step = match next {
                None => {
                    tracing::debug!("the client closed the connection");
                    Next::Close
                }
                Some(Err(error)) => self.stream_broken(error).await,
                Some(Ok(Err(error))) => self.invalid_frame(error).await,
                Some(Ok(Ok(frame))) => self.frame(frame).await,
            };
            if step == Next::Close {
                break;
            }
        }
        self.close().await;
    }

    async fn frame(&mut self, frame: ClientFrame) -> Next {
        match frame {
            ClientFrame::Cancel { id } => {
                if !self.table.cancel(id) {
                    tracing::debug!(request_id = %id, "cancel for a request not in flight");
                }
                Next::Read
            }
            ClientFrame::Request { id, method } => self.request(id, method).await,
            // NOTE: a frame shape added to the protocol later is not something this build
            // can serve; the client learns that it is unsupported.
            _ => self.refuse(None, ErrorCode::Invalid, "the frame shape is not supported").await,
        }
    }

    async fn request(&mut self, id: RequestId, method: Method) -> Next {
        match gate(self.context.is_some(), method, PROTOCOL_VERSION) {
            Gate::Hello(hello) => self.hello(id, hello).await,
            Gate::Dispatch(method) => self.dispatch(id, method).await,
            Gate::Refuse(body) => self.send(ServerFrame::error(Some(id), body)).await,
            Gate::RefuseAndClose(body) => {
                tracing::info!(request_id = %id, error = %body.message, "refused a hello");
                self.send(ServerFrame::error(Some(id), body)).await;
                Next::Close
            }
        }
    }

    async fn hello(&mut self, id: RequestId, hello: Hello) -> Next {
        let context = ConnectionContext::new(self.params.conn_id, hello.origin, self.params.peer);
        let answer = tokio::select! {
            biased;
            () = self.closing.cancelled() => return Next::Close,
            answer = self.params.dispatcher.hello(&context, &hello) => answer,
        };
        let mut result = match answer {
            Ok(result) => result,
            Err(body) => return self.send(ServerFrame::error(Some(id), body)).await,
        };
        self.accepted = Some(context.clone());
        result.protocol = PROTOCOL_VERSION;
        let item = match ServerFrame::item(id, &result) {
            Ok(item) => item,
            Err(error) => {
                tracing::error!(error = %error, "could not encode the hello result");
                let message = "the hello result could not be encoded";
                return self.refuse(Some(id), ErrorCode::Internal, message).await;
            }
        };
        if self.send(item).await == Next::Close {
            return Next::Close;
        }
        tracing::Span::current().record("surface", tracing::field::debug(context.surface()));
        tracing::debug!(client = hello.client.as_deref(), "hello");
        self.context = Some(context);
        self.send(ServerFrame::end(id)).await
    }

    async fn dispatch(&mut self, id: RequestId, method: Method) -> Next {
        let Some(context) = self.context.clone() else {
            // The gate dispatches only after hello; answering keeps the client informed
            // even if that ever changed.
            return self.refuse(Some(id), ErrorCode::Unauthorized, "hello has not completed").await;
        };
        let token = self.closing.child_token();
        if !self.table.insert(id, token.clone()) {
            let message = format!("request id {id} is already in flight");
            return self.refuse(Some(id), ErrorCode::Invalid, message).await;
        }
        let request = RequestTask {
            dispatcher: Arc::clone(&self.params.dispatcher),
            id,
            method,
            context,
            outbound: self.outbound.clone(),
            table: Arc::clone(&self.table),
            token,
            closing: self.closing.clone(),
        };
        let handle = self.tasks.spawn(request.run().in_current_span());
        self.task_requests.insert(handle.id(), id);
        Next::Read
    }

    /// A payload that is not a valid client frame. The stream is still in step, so the
    /// request is answered when its id is readable; without an id the connection closes,
    /// as the protocol says an error without an id does.
    async fn invalid_frame(&mut self, error: ProtocolError) -> Next {
        let id = match &error {
            ProtocolError::Decode { id, .. } => *id,
            _ => None,
        };
        tracing::debug!(error = %error, request_id = ?id, "invalid frame");
        let step =
            self.refuse(id, ErrorCode::Invalid, "the frame is not a valid client frame").await;
        if id.is_none() { Next::Close } else { step }
    }

    /// The byte stream is out of step or the socket failed; nothing more can be read.
    async fn stream_broken(&mut self, error: TransportError) -> Next {
        tracing::debug!(error = %error, "the connection broke");
        if let TransportError::Protocol { source } = &error {
            let message = match source {
                ProtocolError::FrameTooLarge { .. } => "a frame is larger than the limit",
                _ => "the byte stream is out of step",
            };
            self.refuse(None, ErrorCode::Invalid, message).await;
        }
        Next::Close
    }

    /// Handles a request task that finished. A task that panicked never sent its end
    /// frame, so its request is answered here.
    async fn reap(&mut self, joined: Result<(task::Id, ()), JoinError>) {
        let (task_id, panic) = match joined {
            Ok((task_id, ())) => (task_id, false),
            Err(error) => (error.id(), error.is_panic()),
        };
        let Some(id) = self.task_requests.remove(&task_id) else {
            return;
        };
        if panic {
            tracing::error!(request_id = %id, "a request handler panicked");
            self.table.remove(id);
            self.refuse(Some(id), ErrorCode::Internal, "the request failed inside the daemon")
                .await;
        }
    }

    async fn refuse(
        &mut self,
        id: Option<RequestId>,
        code: ErrorCode,
        message: impl Into<String>,
    ) -> Next {
        self.send(ServerFrame::error(id, ErrorBody::new(code, message))).await
    }

    /// Queues a frame that the reader loop itself answers with. Closes when the writer
    /// is gone or the connection is closing.
    async fn send(&mut self, frame: ServerFrame) -> Next {
        let encoded = match EncodedFrame::new(&frame) {
            Ok(encoded) => encoded,
            Err(error) => {
                tracing::error!(error = %error, "could not encode a frame");
                return Next::Close;
            }
        };
        tokio::select! {
            biased;
            () = self.closing.cancelled() => Next::Close,
            sent = self.outbound.send(encoded) => if sent.is_ok() { Next::Read } else { Next::Close },
        }
    }

    async fn close(mut self) {
        self.closing.cancel();
        while let Some(joined) = self.tasks.join_next_with_id().await {
            if let Err(error) = joined
                && error.is_panic()
            {
                tracing::error!("a request handler panicked while its connection closed");
            }
        }
        if let Some(context) = self.accepted.take() {
            self.params.dispatcher.closed(&context).await;
        }
        drop(self.outbound);
        let grace = self.params.clock.timeout(CLOSE_GRACE, &mut self.writer).await;
        if grace.is_err() {
            tracing::debug!("the peer did not take the last frames in time");
            self.writer.abort();
        }
        tracing::debug!(in_flight = self.table.len(), "connection closed");
    }
}

/// One request in its own task.
struct RequestTask<D> {
    dispatcher: Arc<D>,
    id: RequestId,
    method: Method,
    context: ConnectionContext,
    outbound: mpsc::Sender<EncodedFrame>,
    table: Arc<RequestTable>,
    token: CancellationToken,
    closing: CancellationToken,
}

impl<D: Dispatcher> RequestTask<D> {
    async fn run(self) {
        let RequestTask { dispatcher, id, method, context, outbound, table, token, closing } = self;
        let name = method.name();
        let stream = method.is_stream();
        let state = Arc::new(Mutex::new(ResponseState::new(outbound.clone())));
        let _ended = EndOnDrop(Arc::clone(&state));
        let responder = Responder::new(id, &method, Arc::clone(&state));
        let request = Request { id, method, context, responder, cancelled: token.clone() };
        let outcome = tokio::select! {
            biased;
            () = token.cancelled() => Err(cancelled()),
            outcome = dispatcher.dispatch(request) => outcome,
        };
        table.remove(id);
        // NOTE: once the connection is closing, the end frame is best effort: waiting for
        // queue space could wait forever on a peer that stopped reading.
        let permit = if closing.is_cancelled() {
            outbound.try_reserve().ok()
        } else {
            tokio::select! {
                biased;
                () = closing.cancelled() => None,
                permit = outbound.reserve() => permit.ok(),
            }
        };
        let frame = {
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            state.end();
            terminal_frame(id, name, stream, outcome, &state)
        };
        let Some(permit) = permit else {
            return;
        };
        match EncodedFrame::new(&frame) {
            Ok(encoded) => permit.send(encoded),
            Err(error) => {
                tracing::error!(request_id = %id, error = %error, "could not encode an end frame");
            }
        }
    }
}

/// Ends a request's response state when its task stops, normally or by a panic. After a
/// panic the reader loop answers the request, and a responder that the handler leaked
/// into a task must not send after that answer.
struct EndOnDrop(Arc<Mutex<ResponseState>>);

impl Drop for EndOnDrop {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).end();
    }
}

/// Writes queued frames until every sender is gone, batching whatever is already queued
/// into one flush. A write error closes the connection.
async fn write_frames<W: AsyncWrite + Unpin>(
    write_half: W,
    mut queue: mpsc::Receiver<EncodedFrame>,
    closing: CancellationToken,
) {
    let mut writer = BufWriter::new(write_half);
    let result: io::Result<()> = async {
        while let Some(frame) = queue.recv().await {
            writer.write_all(frame.as_bytes()).await?;
            while let Ok(frame) = queue.try_recv() {
                writer.write_all(frame.as_bytes()).await?;
            }
            writer.flush().await?;
        }
        writer.shutdown().await
    }
    .await;
    if let Err(error) = result {
        tracing::debug!(error = %error, "writing to the connection failed");
        closing.cancel();
    }
}

#[cfg(test)]
mod tests;
