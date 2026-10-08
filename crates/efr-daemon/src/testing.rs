//! Fakes for the tests of this crate: a model that always says one thing, a PTY holder
//! that never starts a shell, a raw protocol client, and a daemon on temporary
//! directories with a manual clock and a seeded generator.
//!
//! `efr-test-daemon` builds the full `TestDaemon` later; these are the few pieces the
//! crate's own tests need to drive the real startup, methods and drain.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use efr_holder::{
    ChildStatus, HolderError, PtyHandle, PtyHolder, PtyId, PtyInfo, Signal, SignalTarget, Size,
    SpawnSpec,
};
use efr_protocol::{
    Capabilities, ClientFrame, ErrorBody, Hello, HelloResult, Method, Origin, PROTOCOL_VERSION,
    RequestId, ServerFrame,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_test_support::{TestClock, TestDirs, TestRng};
use jiff::tz::TimeZone;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::connections::Connections;
use crate::screens::ScreenBackend;
use crate::{DaemonError, Deps, HostInfo, ProviderFactory, Settings};

/// What the fake model answers to every request.
pub(crate) const ANSWER: &str = "hello from the test model";

/// A model that answers every request with [`ANSWER`].
#[derive(Debug)]
pub(crate) struct OneAnswer {
    id: ProviderId,
}

#[async_trait]
impl Provider for OneAnswer {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        self.stream_answer()
    }
}

/// Builds [`OneAnswer`] for any provider id.
#[derive(Debug)]
pub(crate) struct OneAnswerFactory;

impl ProviderFactory for OneAnswerFactory {
    fn provider(&self, _id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let id = ProviderId::new("test").map_err(|source| DaemonError::Provider { source })?;
        Ok(Arc::new(OneAnswer { id }))
    }
}

/// A model that answers like [`OneAnswer`] and keeps every request it gets.
#[derive(Debug)]
pub(crate) struct Recording {
    id: ProviderId,
    requests: Arc<Mutex<Vec<Request>>>,
}

#[async_trait]
impl Provider for Recording {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(request);
        OneAnswer { id: self.id.clone() }.stream_answer()
    }
}

impl OneAnswer {
    fn stream_answer(&self) -> Result<ProviderStream, ProviderError> {
        let events = vec![
            Ok(ProviderEvent::TextDelta { text: ANSWER.to_owned() }),
            Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// Builds [`Recording`] for any provider id; its requests land in the shared list.
#[derive(Debug)]
pub(crate) struct RecordingFactory(pub(crate) Arc<Mutex<Vec<Request>>>);

impl ProviderFactory for RecordingFactory {
    fn provider(&self, _id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let id = ProviderId::new("test").map_err(|source| DaemonError::Provider { source })?;
        Ok(Arc::new(Recording { id, requests: Arc::clone(&self.0) }))
    }
}

/// A model that asks for one shell command, then answers `done` once its result is
/// back, turn after turn.
#[derive(Debug)]
pub(crate) struct RunsOneCommand {
    id: ProviderId,
    command: String,
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for RunsOneCommand {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if call.is_multiple_of(2) {
            let call_id = format!("call_{call}");
            let arguments = serde_json::json!({ "command": self.command }).to_string();
            vec![
                Ok(ProviderEvent::ToolCallStart {
                    call_id: call_id.clone(),
                    name: "shell".to_owned(),
                    freeform: false,
                }),
                Ok(ProviderEvent::ToolCallEnd { call_id, arguments }),
                Ok(ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None }),
            ]
        } else {
            vec![
                Ok(ProviderEvent::TextDelta { text: "done".to_owned() }),
                Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// Builds [`RunsOneCommand`] for any provider id.
#[derive(Debug)]
pub(crate) struct RunsOneCommandFactory(pub(crate) String);

impl ProviderFactory for RunsOneCommandFactory {
    fn provider(&self, _id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let id = ProviderId::new("test").map_err(|source| DaemonError::Provider { source })?;
        Ok(Arc::new(RunsOneCommand { id, command: self.0.clone(), calls: AtomicUsize::new(0) }))
    }
}

/// A model that asks for one call of the tool `name` with `arguments`, then answers
/// `done` once its result is back, turn after turn.
#[derive(Debug)]
pub(crate) struct CallsOneTool {
    id: ProviderId,
    name: String,
    arguments: Value,
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for CallsOneTool {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if call.is_multiple_of(2) {
            let call_id = format!("call_{call}");
            vec![
                Ok(ProviderEvent::ToolCallStart {
                    call_id: call_id.clone(),
                    name: self.name.clone(),
                    freeform: false,
                }),
                Ok(ProviderEvent::ToolCallEnd { call_id, arguments: self.arguments.to_string() }),
                Ok(ProviderEvent::Done { stop_reason: StopReason::ToolUse, provider_raw: None }),
            ]
        } else {
            vec![
                Ok(ProviderEvent::TextDelta { text: "done".to_owned() }),
                Ok(ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None }),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// Builds [`CallsOneTool`] for any provider id: the tool's name and its arguments.
#[derive(Debug)]
pub(crate) struct CallsOneToolFactory(pub(crate) String, pub(crate) Value);

impl ProviderFactory for CallsOneToolFactory {
    fn provider(&self, _id: &str) -> Result<Arc<dyn Provider>, DaemonError> {
        let id = ProviderId::new("test").map_err(|source| DaemonError::Provider { source })?;
        Ok(Arc::new(CallsOneTool {
            id,
            name: self.0.clone(),
            arguments: self.1.clone(),
            calls: AtomicUsize::new(0),
        }))
    }
}

/// A holder that never starts a shell.
#[derive(Debug)]
pub(crate) struct NoHolder;

#[async_trait]
impl PtyHolder for NoHolder {
    async fn spawn(&self, spec: SpawnSpec) -> Result<PtyHandle, HolderError> {
        Err(HolderError::NotFound { pty_id: spec.pty_id })
    }
    async fn resize(&self, pty_id: PtyId, _size: Size) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn signal(
        &self,
        pty_id: PtyId,
        _signal: Signal,
        _target: SignalTarget,
    ) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn list(&self) -> Result<Vec<PtyInfo>, HolderError> {
        Ok(Vec::new())
    }
    async fn foreground(&self, pty_id: PtyId) -> Result<Option<u32>, HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn wait(&self, pty_id: PtyId) -> Result<ChildStatus, HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
    async fn release(&self, pty_id: PtyId) -> Result<(), HolderError> {
        Err(HolderError::NotFound { pty_id })
    }
}

/// The dependencies of a daemon on `dirs`, with nothing taken from the machine.
pub(crate) fn deps(dirs: &TestDirs, clock: &TestClock) -> Deps {
    Deps::new(dirs.dirs().clone(), dirs.home(), clock.shared(), Arc::new(TestRng::new(7)))
        .with_in_memory_store()
        .with_isolated_git()
        .with_holder(Arc::new(NoHolder))
        .with_screens(ScreenBackend::Vt100.factory(), "vt100")
        .with_providers(Arc::new(OneAnswerFactory))
        .with_host(HostInfo::new(Some("testhost".to_owned()), Some("TestOS".to_owned())))
        .with_time_zone(TimeZone::UTC)
}

/// A daemon serving in a task of its own.
#[derive(Debug)]
pub(crate) struct Running {
    pub(crate) socket: std::path::PathBuf,
    pub(crate) shutdown: CancellationToken,
    pub(crate) served: JoinHandle<Result<(), DaemonError>>,
    pub(crate) connections: Arc<Connections>,
    pub(crate) settings: watch::Sender<Arc<Settings>>,
    pub(crate) engine: watch::Sender<Arc<efr_permissions::Engine>>,
}

impl Running {
    /// Stops the daemon and waits until it has drained.
    pub(crate) async fn stop(self) {
        self.shutdown.cancel();
        self.served.await.unwrap().unwrap();
    }
}

/// Starts a daemon on `dirs` and serves it.
pub(crate) async fn serve(dirs: &TestDirs, clock: &TestClock) -> Running {
    serve_with(Settings::default(), deps(dirs, clock)).await
}

/// Starts a daemon with `config` and `deps` and serves it.
pub(crate) async fn serve_with(config: Settings, deps: Deps) -> Running {
    let daemon = crate::start(config, deps).await.unwrap();
    let socket = daemon.socket_path().to_path_buf();
    let connections = daemon.connections();
    let settings = daemon.settings();
    let engine = daemon.engine();
    let shutdown = CancellationToken::new();
    let served = tokio::spawn(daemon.serve(shutdown.clone()));
    Running { socket, shutdown, served, connections, settings, engine }
}

/// A protocol client that speaks raw frames.
#[derive(Debug)]
pub(crate) struct RawClient {
    stream: UnixStream,
    decoder: efr_protocol::framing::Decoder,
    frames: VecDeque<Vec<u8>>,
    next_id: u64,
}

impl RawClient {
    pub(crate) async fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).await.unwrap();
        RawClient {
            stream,
            decoder: efr_protocol::framing::Decoder::new(),
            frames: VecDeque::new(),
            next_id: 0,
        }
    }

    /// Connects and says hello as the shell plugin of `tty`.
    pub(crate) async fn hello(socket: &Path, tty: Option<&str>) -> (Self, HelloResult) {
        RawClient::hello_as(socket, tty, Origin::Shell).await
    }

    /// Connects and says hello from `origin`.
    pub(crate) async fn hello_as(
        socket: &Path,
        tty: Option<&str>,
        origin: Origin,
    ) -> (Self, HelloResult) {
        let mut client = RawClient::connect(socket).await;
        let hello = Method::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            origin,
            client: Some("efr-daemon tests".to_owned()),
            capabilities: Capabilities::default(),
            tty: tty.map(str::to_owned),
            pid: None,
            device_id: None,
        });
        let result = client.call(hello).await.unwrap();
        (client, result)
    }

    pub(crate) async fn send(&mut self, method: Method) -> RequestId {
        self.next_id += 1;
        let id = RequestId::new(self.next_id);
        let bytes = efr_protocol::framing::encode(&ClientFrame::Request { id, method }).unwrap();
        self.stream.write_all(&bytes).await.unwrap();
        id
    }

    pub(crate) async fn recv(&mut self) -> ServerFrame {
        loop {
            if let Some(frame) = self.frames.pop_front() {
                return ServerFrame::from_json(&frame).unwrap();
            }
            let mut buffer = vec![0; 64 * 1024];
            let read = self.stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "the daemon closed the connection");
            self.frames.extend(self.decoder.push(&buffer[..read]).unwrap());
        }
    }

    /// A unary call: its one item, or its error.
    pub(crate) async fn call<T: DeserializeOwned>(
        &mut self,
        method: Method,
    ) -> Result<T, ErrorBody> {
        let id = self.send(method).await;
        let first = self.recv().await;
        match first {
            ServerFrame::Item { id: got, item } => {
                assert_eq!(got, id);
                assert!(matches!(self.recv().await, ServerFrame::End { id: end } if end == id));
                Ok(serde_json::from_value(item).unwrap())
            }
            ServerFrame::Error(frame) => Err(frame.error),
            other => panic!("unexpected frame {other:?}"),
        }
    }

    /// The next item of the stream `id`, `None` at its end, the error at its error.
    pub(crate) async fn next(&mut self, id: RequestId) -> Result<Option<Value>, ErrorBody> {
        match self.recv().await {
            ServerFrame::Item { id: got, item } if got == id => Ok(Some(item)),
            ServerFrame::End { id: got } if got == id => Ok(None),
            ServerFrame::Error(frame) if frame.id == Some(id) => Err(frame.error),
            other => panic!("unexpected frame {other:?}"),
        }
    }
}
