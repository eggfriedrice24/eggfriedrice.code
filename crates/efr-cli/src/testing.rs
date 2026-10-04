//! Fakes shared by the unit tests: a daemon on a temporary socket built from
//! `efr_protocol::framing`, a context on temporary directories, fixed clocks and
//! sizes, scripted keys and a Ctrl+C the test triggers.
//!
//! The fake daemon cannot come from `efr-transport`: `efr-cli` may not depend on it,
//! not even for tests. Speaking the framing directly keeps the CLI honest about the
//! bytes on the wire.

use std::collections::VecDeque;
use std::future::{pending, ready};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    CallId, Capabilities, ClientFrame, ConversationId, ConversationSubscribeItem, DaemonPaths,
    ErrorBody, Event, EventEnvelope, Hello, HelloResult, Method, PROTOCOL_VERSION, RequestId, Seq,
    ServerFrame, TurnId,
};
use efr_stdx::env::{Env, Var};
use efr_stdx::paths::Dirs;
use efr_stdx::rng::Rng;
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use serde::Serialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, mpsc};

use crate::cli::{Cli, Command};
use crate::context::{Browser, Context, Interrupt, Stop};
use crate::error::CliError;
use crate::keys::{KeyReader, Keys};
use crate::output::Output;
use crate::settings::Settings;
use crate::terminal::{Screen, Size, TermFacts};

/// The instant every fake clock reads: 2026-10-04T12:00:00Z.
pub(crate) fn now() -> Timestamp {
    Timestamp::from_second(1_791_115_200).unwrap()
}

pub(crate) const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
pub(crate) const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";
pub(crate) const CALL: &str = "019a9b1c-3d00-7a10-8b20-000000000003";

pub(crate) fn conversation() -> ConversationId {
    CONVERSATION.parse().unwrap()
}

pub(crate) fn turn() -> TurnId {
    TURN.parse().unwrap()
}

pub(crate) fn call() -> CallId {
    CALL.parse().unwrap()
}

/// Escape bytes as `\e` and carriage returns as `\r`, so snapshots of terminal output
/// stay readable and keep the carriage returns that insta would otherwise drop.
pub(crate) fn readable(painted: &str) -> String {
    painted.replace('\x1b', "\\e").replace('\r', "\\r")
}

/// Parses an `efr` command line.
pub(crate) fn command(args: &[&str]) -> Command {
    let mut argv = vec!["efr"];
    argv.extend_from_slice(args);
    match <Cli as clap::Parser>::try_parse_from(argv) {
        Ok(cli) => cli.command,
        Err(error) => panic!("{args:?} does not parse: {error}"),
    }
}

/// An event of the conversation as a subscription item.
pub(crate) fn item(seq: u64, event: Event) -> ConversationSubscribeItem {
    ConversationSubscribeItem::Event(envelope(seq, event))
}

pub(crate) fn envelope(seq: u64, event: Event) -> EventEnvelope {
    EventEnvelope { seq: Seq::new(seq), conversation_id: Some(conversation()), at: now(), event }
}

/// A clock whose sleeps never finish, so no timeout fires.
#[derive(Debug)]
pub(crate) struct StoppedClock;

impl Clock for StoppedClock {
    fn now(&self) -> Timestamp {
        now()
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
        now()
    }

    fn sleep(&self, _duration: Duration) -> Sleep {
        Box::pin(ready(()))
    }
}

/// Random bytes that count up, so ids differ and repeat across runs.
#[derive(Debug, Default)]
pub(crate) struct CountingRng(AtomicU64);

impl Rng for CountingRng {
    fn fill_bytes(&self, dest: &mut [u8]) {
        for byte in dest {
            *byte = self.0.fetch_add(1, Ordering::Relaxed).to_le_bytes()[0];
        }
    }
}

/// A screen of a fixed size.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedScreen(pub(crate) Size);

impl Screen for FixedScreen {
    fn size(&self) -> Size {
        self.0
    }
}

/// No terminal to read keys from.
#[derive(Debug)]
pub(crate) struct NoKeys;

impl Keys for NoKeys {
    fn available(&self) -> bool {
        false
    }

    fn start(&self) -> Result<KeyReader, CliError> {
        Err(CliError::Terminal { source: io::Error::other("no terminal in this test") })
    }
}

/// Keys that the test sends. Every reader gets the keys sent after it started.
#[derive(Debug, Default)]
pub(crate) struct ScriptedKeys {
    senders: Mutex<Vec<mpsc::Sender<u8>>>,
    started: Notify,
}

impl ScriptedKeys {
    /// Waits until a reader has started, then sends `key` to the newest one.
    pub(crate) async fn press(&self, key: u8) {
        loop {
            let sender = self.senders.lock().unwrap().last().cloned();
            if let Some(sender) = sender
                && sender.send(key).await.is_ok()
            {
                return;
            }
            self.started.notified().await;
        }
    }

    /// How many readers were started.
    pub(crate) fn starts(&self) -> usize {
        self.senders.lock().unwrap().len()
    }
}

impl Keys for ScriptedKeys {
    fn available(&self) -> bool {
        true
    }

    fn start(&self) -> Result<KeyReader, CliError> {
        let (sender, keys) = mpsc::channel(8);
        self.senders.lock().unwrap().push(sender);
        self.started.notify_one();
        Ok(KeyReader::from_channel(keys))
    }
}

/// A Ctrl+C that the test triggers. A trigger before anyone waits is kept.
#[derive(Debug, Default)]
pub(crate) struct TestInterrupt(Arc<Notify>);

impl TestInterrupt {
    pub(crate) fn trigger(&self) {
        self.0.notify_one();
    }
}

impl Interrupt for TestInterrupt {
    fn wait(&self) -> Stop {
        let notify = Arc::clone(&self.0);
        Box::pin(async move { notify.notified().await })
    }
}

/// A browser that only remembers the URLs it was asked to open.
#[derive(Debug, Default)]
pub(crate) struct RecordingBrowser(pub(crate) Mutex<Vec<String>>);

impl Browser for RecordingBrowser {
    fn open(&self, url: &str) -> io::Result<()> {
        self.0.lock().unwrap().push(url.to_owned());
        Ok(())
    }
}

/// An [`Output`] that keeps what was written, and the handle to read it.
pub(crate) fn capture() -> (Output, Captured) {
    let captured = Captured::default();
    let output = Output::from_writers(
        Box::new(Buffer(Arc::clone(&captured.stdout))),
        Box::new(Buffer(Arc::clone(&captured.stderr))),
    );
    (output, captured)
}

/// What a captured [`Output`] received.
#[derive(Debug, Clone, Default)]
pub(crate) struct Captured {
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl Captured {
    pub(crate) fn stdout(&self) -> String {
        String::from_utf8(self.stdout.lock().unwrap().clone()).unwrap()
    }

    pub(crate) fn stderr(&self) -> String {
        String::from_utf8(self.stderr.lock().unwrap().clone()).unwrap()
    }
}

struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Facts for a terminal on all three streams, with 16 colours.
pub(crate) fn terminal_facts() -> TermFacts {
    TermFacts {
        no_color: false,
        term: Some("xterm-256color".to_owned()),
        colorterm: None,
        stdout_tty: true,
        stderr_tty: true,
        stdin_tty: true,
    }
}

/// Temporary efr roots, and contexts and fake daemons on them.
#[derive(Debug)]
pub(crate) struct TestEnv {
    _dir: tempfile::TempDir,
    pub(crate) dirs: Dirs,
}

impl TestEnv {
    pub(crate) fn new() -> TestEnv {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["config", "data", "state", "runtime"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        let dirs = Dirs::new(
            root.join("config"),
            root.join("data"),
            root.join("state"),
            root.join("runtime"),
        )
        .unwrap();
        TestEnv { _dir: dir, dirs }
    }

    pub(crate) fn socket(&self) -> PathBuf {
        self.dirs.socket_path()
    }

    /// A fake daemon listening on the default socket.
    pub(crate) fn listen(&self) -> FakeDaemon {
        FakeDaemon { listener: UnixListener::bind(self.socket()).unwrap() }
    }

    /// A context on these roots: not a terminal, no keys, no environment variables,
    /// the stopped clock, and a working directory of `/home/user/project`.
    pub(crate) fn context(&self) -> Context {
        Context {
            dirs: self.dirs.clone(),
            env: Env::fixed(Vec::<(Var, String)>::new()),
            term: TermFacts::default(),
            settings: Settings::default(),
            clock: Arc::new(StoppedClock),
            rng: Arc::new(CountingRng::default()),
            screen: Arc::new(FixedScreen(Size::default())),
            keys: Arc::new(NoKeys),
            interrupt: Arc::new(TestInterrupt::default()),
            browser: Arc::new(RecordingBrowser::default()),
            cwd: Some(PathBuf::from("/home/user/project")),
            tty: None,
        }
    }
}

/// What the fake daemon answers to hello.
pub(crate) fn hello_result() -> HelloResult {
    HelloResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        protocol: PROTOCOL_VERSION,
        version: "0.1.0".to_owned(),
        capabilities: Capabilities { admin: Some(true), ..Capabilities::default() },
        paths: DaemonPaths::default(),
        challenge: "AAAA".to_owned(),
    }
}

/// A daemon that the test scripts frame by frame.
#[derive(Debug)]
pub(crate) struct FakeDaemon {
    listener: UnixListener,
}

impl FakeDaemon {
    /// Accepts the next connection and answers its hello.
    pub(crate) async fn accept(&self) -> Conn {
        let (stream, _) = self.listener.accept().await.unwrap();
        let mut conn =
            Conn { stream, decoder: Decoder::new(), ready: VecDeque::new(), hello: None };
        let (id, method) = conn.request().await;
        let Method::Hello(hello) = method else {
            panic!("the first request must be hello, got {}", method.name());
        };
        conn.reply(id, &hello_result()).await;
        conn.hello = Some(hello);
        conn
    }
}

/// One accepted connection.
#[derive(Debug)]
pub(crate) struct Conn {
    stream: UnixStream,
    decoder: Decoder,
    ready: VecDeque<Vec<u8>>,
    hello: Option<Hello>,
}

impl Conn {
    /// The hello the client sent.
    pub(crate) fn hello(&self) -> &Hello {
        self.hello.as_ref().unwrap()
    }

    /// The next client frame, or `None` once the client closed its side.
    pub(crate) async fn recv(&mut self) -> Option<ClientFrame> {
        loop {
            if let Some(payload) = self.ready.pop_front() {
                return Some(ClientFrame::from_json(&payload).unwrap());
            }
            let mut buf = [0_u8; 4096];
            let read = match self.stream.read(&mut buf).await {
                Ok(read) => read,
                // A client that exits with frames unread resets the connection.
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => 0,
                Err(error) => panic!("reading from the client failed: {error}"),
            };
            if read == 0 {
                self.decoder.finish().unwrap();
                return None;
            }
            self.ready.extend(self.decoder.push(&buf[..read]).unwrap());
        }
    }

    /// The next frame, which must be a request.
    pub(crate) async fn request(&mut self) -> (RequestId, Method) {
        match self.recv().await {
            Some(ClientFrame::Request { id, method }) => (id, method),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    pub(crate) async fn send(&mut self, frame: &ServerFrame) {
        let bytes = framing::encode(frame).unwrap();
        self.stream.write_all(&bytes).await.unwrap();
        self.stream.flush().await.unwrap();
    }

    /// One item of request `id`.
    pub(crate) async fn item<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.send(&ServerFrame::item(id, value).unwrap()).await;
    }

    pub(crate) async fn end(&mut self, id: RequestId) {
        self.send(&ServerFrame::end(id)).await;
    }

    /// The result of a unary request: its item and its end.
    pub(crate) async fn reply<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.item(id, value).await;
        self.end(id).await;
    }

    pub(crate) async fn fail(&mut self, id: RequestId, body: ErrorBody) {
        self.send(&ServerFrame::error(Some(id), body)).await;
    }

    /// Reads until the client closes the connection and returns the frames it sent on
    /// the way, such as the cancel of a stream it dropped.
    pub(crate) async fn until_closed(&mut self) -> Vec<ClientFrame> {
        let mut frames = Vec::new();
        while let Some(frame) = self.recv().await {
            frames.push(frame);
        }
        frames
    }
}
