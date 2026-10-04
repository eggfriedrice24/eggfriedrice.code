//! The `Dispatcher` trait that the daemon implements, and what each request hands it.
//!
//! The transport owns the request lifecycle: it checks hello, keeps the request-id
//! table, races each request against its cancellation and sends the one frame that ends
//! it. The dispatcher only answers methods. It sends results and stream items through
//! the request's [`Responder`] and returns `Ok(())` or the error to send; mapping the
//! daemon's own errors to an [`ErrorBody`] is the daemon's job.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use efr_protocol::{ErrorBody, ErrorCode, Hello, HelloResult, Method, RequestId, Seq, ServerFrame};
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::codec::EncodedFrame;
use crate::subscriptions::{Delivery, SubscriptionReceiver};
use crate::{ConnectionContext, TransportError};

/// The engine behind the protocol edge. `efr-daemon` implements it in `methods.rs` with
/// an exhaustive match over [`Method`].
///
/// The futures must be `Send` because every request runs in its own task. A future is
/// dropped at its next await point when the request is cancelled or its connection
/// closes, so handlers keep state that must survive that in an actor, not on the stack.
pub trait Dispatcher: Send + Sync + 'static {
    /// Answers `hello`, the first request of a connection.
    ///
    /// The transport has already checked the protocol version and built `context` from
    /// the kernel's peer credentials and the declared origin. The returned result is
    /// sent with its `protocol` set to the transport's
    /// [`PROTOCOL_VERSION`](efr_protocol::PROTOCOL_VERSION). An error is sent as the
    /// answer and the connection stays open, still waiting for a hello.
    ///
    /// The context carries only what every request needs. What else the hello declares
    /// (capabilities, client, tty, device) the dispatcher keeps itself, keyed by
    /// [`ConnectionContext::conn_id`], and lets go of in [`Dispatcher::closed`]. It
    /// records that state only once its hello can no longer fail: a hello that returns
    /// an error, or whose future is dropped because the connection closed first, is
    /// never followed by `closed`.
    fn hello(
        &self,
        context: &ConnectionContext,
        hello: &Hello,
    ) -> impl Future<Output = Result<HelloResult, ErrorBody>> + Send;

    /// Reports that a connection closed whose hello this dispatcher accepted, so the
    /// daemon can drop what it keeps per connection: the negotiated capabilities, the
    /// lease that `lease.report` replaces, a login the connection started.
    ///
    /// Called once per connection, with the context of the last accepted hello, even
    /// when its answer could not be sent. By then every request of the connection has
    /// ended and its handler future is dropped, so no handler of the connection runs
    /// alongside it. The transport waits for the future before it flushes the last
    /// frames and frees the connection's task, so it must be quick and must not wait for
    /// a client. The default does nothing.
    fn closed(&self, _context: &ConnectionContext) -> impl Future<Output = ()> + Send {
        async {}
    }

    /// Runs one request other than `hello`.
    ///
    /// A unary method sends exactly one item through `request.responder` and returns
    /// `Ok(())`; a streaming method sends any number. The transport then sends the end
    /// frame, or the returned error. Returning `Ok(())` from a unary method without an
    /// item is answered with `internal`.
    fn dispatch(&self, request: Request) -> impl Future<Output = Result<(), ErrorBody>> + Send;
}

/// One request, handed to [`Dispatcher::dispatch`].
///
/// Only the transport builds it; a handler takes it apart with
/// `let Request { method, responder, .. } = request;`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Request {
    /// The client's id for the request.
    pub id: RequestId,
    /// The method and its params.
    pub method: Method,
    /// Who sent it.
    pub context: ConnectionContext,
    /// Where its results and items go.
    pub responder: Responder,
    /// Cancelled when the client cancels the request, the connection closes or the
    /// daemon shuts down. The transport drops the handler's future at that moment;
    /// work that the handler spawned can watch this token to stop with it.
    pub cancelled: CancellationToken,
}

/// What the transport needs to know about a request after its handler returns.
#[derive(Debug, Default)]
pub(crate) struct ResponseState {
    /// Item frames queued so far.
    items: u64,
    /// Set when [`Responder::forward`] met an overflow.
    overflow: Option<Seq>,
}

/// Sends the result or the stream items of one request.
#[derive(Debug)]
pub struct Responder {
    id: RequestId,
    method: &'static str,
    stream: bool,
    outbound: mpsc::Sender<EncodedFrame>,
    state: Arc<Mutex<ResponseState>>,
}

impl Responder {
    pub(crate) fn new(
        id: RequestId,
        method: &Method,
        outbound: mpsc::Sender<EncodedFrame>,
        state: Arc<Mutex<ResponseState>>,
    ) -> Self {
        Responder { id, method: method.name(), stream: method.is_stream(), outbound, state }
    }

    /// The request this responder answers.
    pub fn id(&self) -> RequestId {
        self.id
    }

    /// Sends one item: the result of a unary method, or the next item of a stream.
    ///
    /// Waits while the connection's outbound queue is full, so a client that reads
    /// slowly slows its own requests and nothing else. Fails without sending when the
    /// item cannot be encoded or is larger than the frame limit, when a unary method
    /// already sent its result, and when the connection is closed.
    pub async fn item<T: Serialize + ?Sized>(&self, item: &T) -> Result<(), TransportError> {
        let frame = EncodedFrame::new(&ServerFrame::item(self.id, item)?)?;
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !self.stream && state.items > 0 {
                return Err(TransportError::ResultAlreadySent { method: self.method });
            }
            state.items += 1;
        }
        self.queue(frame).await
    }

    /// Sends `{ack: id}`, for streaming methods that document an acknowledgement before
    /// their first item.
    pub async fn ack(&self) -> Result<(), TransportError> {
        if !self.stream {
            return Err(TransportError::NotAStream { method: self.method });
        }
        self.queue(EncodedFrame::new(&ServerFrame::Ack { id: self.id })?).await
    }

    /// Sends every item of `subscription` until its producer finishes, which returns
    /// `Ok(())`.
    ///
    /// When the subscriber falls behind, the items queued before the overflow are sent
    /// first and then this returns [`TransportError::Overflow`]. The request then ends
    /// with an `overflow` error carrying `last_seq`, whatever the handler returns, so
    /// the client can subscribe again from there.
    pub async fn forward<T: Serialize>(
        &self,
        mut subscription: SubscriptionReceiver<T>,
    ) -> Result<(), TransportError> {
        if !self.stream {
            return Err(TransportError::NotAStream { method: self.method });
        }
        while let Some(delivery) = subscription.recv().await {
            match delivery {
                Delivery::Item { item, .. } => self.item(&item).await?,
                Delivery::Overflowed { last_seq } => {
                    self.state.lock().unwrap_or_else(PoisonError::into_inner).overflow =
                        Some(last_seq);
                    return Err(TransportError::Overflow { last_seq });
                }
            }
        }
        Ok(())
    }

    async fn queue(&self, frame: EncodedFrame) -> Result<(), TransportError> {
        self.outbound.send(frame).await.map_err(|_| TransportError::Closed)
    }
}

/// The frame that ends a request, from the handler's outcome and what it sent. A
/// cancelled request arrives here as an `Err` with the `cancelled` body.
pub(crate) fn terminal_frame(
    id: RequestId,
    method: &'static str,
    stream: bool,
    outcome: Result<(), ErrorBody>,
    state: &ResponseState,
) -> ServerFrame {
    if let Some(last_seq) = state.overflow {
        return ServerFrame::error(Some(id), ErrorBody::overflow(last_seq));
    }
    match outcome {
        Err(body) => ServerFrame::error(Some(id), body),
        Ok(()) if !stream && state.items == 0 => ServerFrame::error(
            Some(id),
            ErrorBody::new(ErrorCode::Internal, format!("{method} finished without a result")),
        ),
        Ok(()) => ServerFrame::end(id),
    }
}

/// The body of the error that ends a cancelled request.
pub(crate) fn cancelled() -> ErrorBody {
    ErrorBody::new(ErrorCode::Cancelled, "the request was cancelled")
}

#[cfg(test)]
mod tests;
