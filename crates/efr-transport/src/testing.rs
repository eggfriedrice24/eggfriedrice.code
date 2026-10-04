//! Fakes shared by the unit tests of this crate: clocks, a scripted dispatcher and a raw
//! protocol client.

use std::collections::VecDeque;
use std::future::{Future, pending, ready};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    Capabilities, ClientFrame, ConversationId, ConversationSubscribe, ConversationsList,
    DaemonPaths, ErrorBody, ErrorCode, Hello, HelloResult, Method, Origin, RequestId, ServerFrame,
};
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::oneshot;

use crate::{ConnectionContext, Dispatcher, Request, TransportError};

/// The instant every fake clock reads: 2026-10-04T12:00:00Z.
fn start() -> Timestamp {
    Timestamp::from_second(1_791_115_200).unwrap()
}

/// A clock whose sleeps never finish, so a grace period or a retry never ends on its
/// own.
#[derive(Debug)]
pub(crate) struct StoppedClock;

impl Clock for StoppedClock {
    fn now(&self) -> Timestamp {
        start()
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(pending())
    }
}

/// A clock whose sleeps finish at once.
#[derive(Debug)]
pub(crate) struct InstantClock;

impl Clock for InstantClock {
    fn now(&self) -> Timestamp {
        start()
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(ready(()))
    }
}

type HandlerFuture = Pin<Box<dyn Future<Output = Result<(), ErrorBody>> + Send>>;
type Handler = Box<dyn Fn(Request) -> HandlerFuture + Send + Sync>;

/// A dispatcher that answers hello with [`hello_result`] (or a set error) and runs every
/// other request through a closure.
pub(crate) struct FakeDispatcher {
    handler: Handler,
    hello_error: Option<ErrorBody>,
    contexts: Mutex<Vec<ConnectionContext>>,
    /// Handler futures that exist right now.
    live: Arc<AtomicUsize>,
    closed: Mutex<Vec<Closed>>,
}

/// One call of [`Dispatcher::closed`] on the fake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Closed {
    /// The context it reported.
    pub(crate) context: ConnectionContext,
    /// How many handler futures still existed when it was called.
    pub(crate) live_handlers: usize,
}

/// Counts one live handler future until it is dropped.
struct LiveHandler(Arc<AtomicUsize>);

impl LiveHandler {
    fn new(live: &Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::SeqCst);
        LiveHandler(Arc::clone(live))
    }
}

impl Drop for LiveHandler {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl FakeDispatcher {
    pub(crate) fn new<F, Fut>(handler: F) -> Arc<Self>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ErrorBody>> + Send + 'static,
    {
        Arc::new(FakeDispatcher {
            handler: Box::new(move |request| Box::pin(handler(request))),
            hello_error: None,
            contexts: Mutex::new(Vec::new()),
            live: Arc::default(),
            closed: Mutex::new(Vec::new()),
        })
    }

    /// A dispatcher that refuses every hello with `error`.
    pub(crate) fn refusing_hello(error: ErrorBody) -> Arc<Self> {
        Arc::new(FakeDispatcher {
            handler: Box::new(|_| Box::pin(ready(Ok(())))),
            hello_error: Some(error),
            contexts: Mutex::new(Vec::new()),
            live: Arc::default(),
            closed: Mutex::new(Vec::new()),
        })
    }

    /// The context of every hello it answered or refused, in order.
    pub(crate) fn contexts(&self) -> Vec<ConnectionContext> {
        self.contexts.lock().unwrap().clone()
    }

    /// Every call of `closed`, in order.
    pub(crate) fn closed_calls(&self) -> Vec<Closed> {
        self.closed.lock().unwrap().clone()
    }
}

impl Dispatcher for FakeDispatcher {
    fn hello(
        &self,
        context: &ConnectionContext,
        _hello: &Hello,
    ) -> impl Future<Output = Result<HelloResult, ErrorBody>> + Send {
        self.contexts.lock().unwrap().push(context.clone());
        ready(match &self.hello_error {
            Some(error) => Err(error.clone()),
            None => Ok(hello_result()),
        })
    }

    fn dispatch(&self, request: Request) -> impl Future<Output = Result<(), ErrorBody>> + Send {
        let live = LiveHandler::new(&self.live);
        let handler = (self.handler)(request);
        async move {
            let _live = live;
            handler.await
        }
    }

    fn closed(&self, context: &ConnectionContext) -> impl Future<Output = ()> + Send {
        let live_handlers = self.live.load(Ordering::SeqCst);
        self.closed.lock().unwrap().push(Closed { context: context.clone(), live_handlers });
        ready(())
    }
}

/// What the fake daemon answers to hello. Its `protocol` is deliberately wrong, so tests
/// see the transport put its own version in.
pub(crate) fn hello_result() -> HelloResult {
    HelloResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        protocol: 999,
        version: "0.1.0".to_owned(),
        capabilities: Capabilities::default(),
        paths: DaemonPaths::default(),
        challenge: "AAAA".to_owned(),
    }
}

pub(crate) fn hello_frame(id: u64, protocol: u32) -> ClientFrame {
    ClientFrame::Request {
        id: RequestId::new(id),
        method: Method::Hello(Hello {
            protocol,
            origin: Origin::Cli,
            client: Some("efr 0.1.0".to_owned()),
            capabilities: Capabilities::default(),
            tty: None,
            pid: None,
            device_id: None,
        }),
    }
}

/// A unary request.
pub(crate) fn list_frame(id: u64) -> ClientFrame {
    ClientFrame::Request {
        id: RequestId::new(id),
        method: Method::ConversationsList(ConversationsList { cursor: None, limit: None }),
    }
}

/// A streaming request.
pub(crate) fn subscribe_frame(id: u64) -> ClientFrame {
    ClientFrame::Request {
        id: RequestId::new(id),
        method: Method::ConversationSubscribe(ConversationSubscribe {
            conversation_id: "019a9b1c-3d00-7a10-8b20-000000000001"
                .parse::<ConversationId>()
                .unwrap(),
            after_seq: None,
        }),
    }
}

pub(crate) fn cancel_frame(id: u64) -> ClientFrame {
    ClientFrame::Cancel { id: RequestId::new(id) }
}

/// Maps a transport error inside a fake handler to a wire error.
pub(crate) fn internal(error: TransportError) -> ErrorBody {
    ErrorBody::new(ErrorCode::Internal, error.to_string())
}

/// Sends on a oneshot when dropped, so a test can see a handler's future dropped.
#[derive(Debug)]
pub(crate) struct DropSignal(Option<oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

pub(crate) fn drop_signal() -> (DropSignal, oneshot::Receiver<()>) {
    let (tx, rx) = oneshot::channel();
    (DropSignal(Some(tx)), rx)
}

/// A raw protocol client: encodes client frames and decodes server frames itself, so
/// the tests check the bytes on the wire and not another codec.
#[derive(Debug)]
pub(crate) struct TestClient<S> {
    stream: S,
    decoder: Decoder,
    ready: VecDeque<Vec<u8>>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> TestClient<S> {
    pub(crate) fn new(stream: S) -> Self {
        TestClient { stream, decoder: Decoder::new(), ready: VecDeque::new() }
    }

    pub(crate) async fn send(&mut self, frame: &ClientFrame) {
        self.send_raw(&framing::encode(frame).unwrap()).await;
    }

    pub(crate) async fn send_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
        self.stream.flush().await.unwrap();
    }

    /// The next frame, or `None` once the server closed the connection.
    pub(crate) async fn recv(&mut self) -> Option<ServerFrame> {
        let mut buffer = [0; 4096];
        loop {
            if let Some(payload) = self.ready.pop_front() {
                return Some(ServerFrame::from_json(&payload).unwrap());
            }
            let read = self.stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                self.decoder.finish().unwrap();
                return None;
            }
            self.ready.extend(self.decoder.push(&buffer[..read]).unwrap());
        }
    }

    /// Says hello with request id 1 and returns the daemon's answer.
    pub(crate) async fn hello(&mut self) -> HelloResult {
        self.send(&hello_frame(1, efr_protocol::PROTOCOL_VERSION)).await;
        let Some(ServerFrame::Item { id, item }) = self.recv().await else {
            panic!("hello was not answered with an item");
        };
        assert_eq!(id, RequestId::new(1));
        assert_eq!(self.recv().await, Some(ServerFrame::end(RequestId::new(1))));
        serde_json::from_value(item).unwrap()
    }

    /// Closes the client's sending side, as a client that goes away does.
    pub(crate) async fn close_write(&mut self) {
        self.stream.shutdown().await.unwrap();
    }
}
