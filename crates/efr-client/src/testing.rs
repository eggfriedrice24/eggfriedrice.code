//! Fakes shared by the unit tests of this crate: clocks and a fake daemon built from
//! the transport's server codec.
//!
//! The clocks stay here instead of coming from `efr-test-support`: its `TestClock` can
//! stand in only for `StoppedClock` (a clock that nobody moves), not for a clock whose
//! sleeps end at once, and `efr-test-support` brings `efr-store` with its bundled SQLite
//! build, which about triples the time to build these tests from clean.

use std::future::{pending, ready};
use std::time::Duration;

use efr_protocol::{
    Capabilities, ClientFrame, DaemonPaths, Hello, HelloResult, Method, RequestId, ServerFrame,
};
use efr_stdx::time::{Clock, Sleep};
use efr_transport::ServerCodec;
use futures::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio_util::codec::Framed;

/// The instant every fake clock reads: 2026-10-04T12:00:00Z.
fn start() -> Timestamp {
    Timestamp::from_second(1_791_115_200).unwrap()
}

/// A clock whose sleeps never finish, so no timeout fires.
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

/// A clock whose sleeps finish at once, so a timeout fires at its first chance.
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

/// What the fake daemon answers to hello.
pub(crate) fn hello_result(protocol: u32) -> HelloResult {
    HelloResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        protocol,
        version: "0.1.0".to_owned(),
        capabilities: Capabilities::default(),
        paths: DaemonPaths::default(),
        challenge: "AAAA".to_owned(),
    }
}

/// A scripted daemon: the test reads each client frame and writes each answer.
#[derive(Debug)]
pub(crate) struct FakeServer<S> {
    frames: Framed<S, ServerCodec>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> FakeServer<S> {
    pub(crate) fn new(stream: S) -> Self {
        FakeServer { frames: Framed::new(stream, ServerCodec::new()) }
    }

    /// The next client frame, or `None` once the client closed its side.
    pub(crate) async fn recv(&mut self) -> Option<ClientFrame> {
        self.frames.next().await.map(|next| next.unwrap().unwrap())
    }

    /// The next frame, which must be a request.
    pub(crate) async fn request(&mut self) -> (RequestId, Method) {
        match self.recv().await {
            Some(ClientFrame::Request { id, method }) => (id, method),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    pub(crate) async fn send(&mut self, frame: ServerFrame) {
        self.frames.send(frame).await.unwrap();
    }

    /// Sends a payload that is not a valid frame, behind a valid length prefix.
    pub(crate) async fn send_raw(&mut self, payload: &[u8]) {
        let mut bytes = u32::try_from(payload.len()).unwrap().to_be_bytes().to_vec();
        bytes.extend_from_slice(payload);
        let stream = self.frames.get_mut();
        stream.write_all(&bytes).await.unwrap();
        stream.flush().await.unwrap();
    }

    /// Reads the hello request, answers it as a daemon of `protocol` would, and returns
    /// the hello params.
    pub(crate) async fn answer_hello_as(&mut self, protocol: u32) -> Hello {
        let (id, method) = self.request().await;
        let Method::Hello(hello) = method else {
            panic!("the first request must be hello, got {}", method.name());
        };
        self.send(ServerFrame::item(id, &hello_result(protocol)).unwrap()).await;
        self.send(ServerFrame::end(id)).await;
        hello
    }

    /// [`answer_hello_as`](Self::answer_hello_as) with this build's protocol.
    pub(crate) async fn answer_hello(&mut self) -> Hello {
        self.answer_hello_as(efr_protocol::PROTOCOL_VERSION).await
    }
}
