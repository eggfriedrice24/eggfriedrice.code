//! The recording switch: with `EFR_RECORD_STREAMS` on at the start of efrd, the bodies
//! of model calls go to private files, so the user can record one real stream and a
//! test can use it as a fixture.
//!
//! [`StreamRecorder`] is the [`Recorder`] of the one HTTP client of the providers
//! (`run.rs`). It writes one directory for each start of efrd,
//! `<state dir>/streams/<UTC time>-<pid>/` at mode 0700, and for exchange `n` of a model
//! call, each file at mode 0600:
//!
//! - `<n>.request.json`: `{"transport": "http" | "websocket", "method", "url", "body"}`,
//!   the URL without its query, the body as sent (for a WebSocket, the message that
//!   asks for the answer);
//! - `<n>.response.sse`: the body of the answer as it came; for a WebSocket, each
//!   message as `event: <type>`, `data: <message>` and a blank line, so the file reads
//!   as the SSE fixtures of efr-providers read;
//! - one line in `index.jsonl`: `{"n", "at", "transport", "status", "end"}`, `end` being
//!   `complete`, `failed` or `dropped`.
//!
//! How it keeps every secret out: it never writes a header (of a request, a response or
//! a handshake: tokens, keys, cookies and account ids travel only there), it takes only
//! an exchange whose URL path ends in `/responses` or `/messages` (a login, a token
//! refresh or a model list is never written, also when a later change marks one
//! recorded by mistake), and it writes the URL without its query and its user. The
//! bodies hold the user's prompts, the output of tools and the answers, so the files are
//! private and efr never sends them anywhere.
//!
//! [`Recorder::record`] runs on the task of the request, so it only sends to a bounded
//! queue; a named thread writes the files.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use efr_http::{BodyEnd, Record, Recorder, Url};
use efr_stdx::StdxError;
use efr_stdx::fs::{Claim, claim_dir, create_private};
use efr_stdx::time::Clock;
use jiff::Timestamp;
use serde_json::{Value, json};

/// The directory below the state root that holds the recordings.
pub(crate) const STREAMS_DIR: &str = "streams";

/// The records that wait for the writer. A record that finds the queue full is lost,
/// with one warning: a slow disk must never slow a model call.
const QUEUE: usize = 4_096;

/// The stack of the writer thread: its loop is shallow.
const WRITER_STACK: usize = 256 * 1024;

/// The ends of the URL paths that the recorder takes: the Responses API and the
/// Messages API.
const MODEL_PATHS: [&str; 2] = ["/responses", "/messages"];

/// The WebSocket message types that end the answer of a model call.
const SOCKET_ENDS: [(&str, &str); 4] = [
    ("response.completed", "complete"),
    ("response.incomplete", "complete"),
    ("response.failed", "failed"),
    ("error", "failed"),
];

/// The recorder of the providers' HTTP client while `EFR_RECORD_STREAMS` is on.
#[derive(Debug)]
pub(crate) struct StreamRecorder {
    queue: SyncSender<Item>,
    clock: Arc<dyn Clock>,
    /// The HTTP exchanges of model calls whose answer still comes.
    wanted: Mutex<HashSet<u64>>,
    /// True once a record was lost to a full queue, so the warning comes once.
    lost: AtomicBool,
}

/// A running recording: the recorder, its directory and the thread that writes it. The
/// thread ends when the last clone of the recorder is gone.
#[derive(Debug)]
pub(crate) struct StreamRecording {
    pub(crate) recorder: Arc<StreamRecorder>,
    pub(crate) dir: PathBuf,
    pub(crate) writer: JoinHandle<()>,
}

/// One record for the writer, with only what it writes: never a header.
#[derive(Debug)]
enum Item {
    Request { exchange: u64, at: Timestamp, method: String, url: String, body: Vec<u8> },
    Response { exchange: u64, status: u16 },
    Chunk { exchange: u64, bytes: Vec<u8> },
    End { exchange: u64, end: &'static str },
    Socket { exchange: u64, at: Timestamp, url: String, sent: bool, text: String },
}

impl StreamRecorder {
    /// Makes the directory of this start below `state` and starts the writer.
    pub(crate) fn start(state: &Path, clock: Arc<dyn Clock>) -> Result<StreamRecording, StdxError> {
        let dir = run_dir(state, clock.now(), std::process::id())?;
        let (queue, items) = mpsc::sync_channel(QUEUE);
        let writer_dir = dir.clone();
        let writer = efr_stdx::thread::spawn_named("efrd-streams", WRITER_STACK, move || {
            Writer::new(writer_dir).run(&items);
        })?;
        let recorder = Arc::new(StreamRecorder {
            queue,
            clock,
            wanted: Mutex::new(HashSet::new()),
            lost: AtomicBool::new(false),
        });
        Ok(StreamRecording { recorder, dir, writer })
    }

    fn send(&self, item: Item) {
        match self.queue.try_send(item) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                if !self.lost.swap(true, Ordering::Relaxed) {
                    tracing::warn!("a record of a model stream was lost: the disk is too slow");
                }
            }
        }
    }

    fn wanted(&self) -> std::sync::MutexGuard<'_, HashSet<u64>> {
        self.wanted.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Recorder for StreamRecorder {
    fn record(&self, record: &Record<'_>) {
        let item = match record {
            Record::Request { exchange, method, url, body, .. } => {
                let Some(url) = model_url(url) else { return };
                self.wanted().insert(exchange.get());
                Item::Request {
                    exchange: exchange.get(),
                    at: self.clock.now(),
                    method: method.as_str().to_owned(),
                    url,
                    body: body.to_vec(),
                }
            }
            Record::Response { exchange, status, .. } => {
                if !self.wanted().contains(&exchange.get()) {
                    return;
                }
                Item::Response { exchange: exchange.get(), status: status.as_u16() }
            }
            Record::BodyChunk { exchange, bytes } => {
                if !self.wanted().contains(&exchange.get()) {
                    return;
                }
                Item::Chunk { exchange: exchange.get(), bytes: bytes.to_vec() }
            }
            Record::BodyEnd { exchange, end } => {
                if !self.wanted().remove(&exchange.get()) {
                    return;
                }
                let end = match end {
                    BodyEnd::Complete => "complete",
                    BodyEnd::Failed => "failed",
                    _ => "dropped",
                };
                Item::End { exchange: exchange.get(), end }
            }
            Record::Failed { exchange, .. } => {
                if !self.wanted().remove(&exchange.get()) {
                    return;
                }
                Item::End { exchange: exchange.get(), end: "failed" }
            }
            Record::SocketText { exchange, url, sent, text } => {
                let Some(url) = model_url(url) else { return };
                Item::Socket {
                    exchange: exchange.get(),
                    at: self.clock.now(),
                    url,
                    sent: *sent,
                    text: (*text).to_owned(),
                }
            }
            _ => return,
        };
        self.send(item);
    }
}

/// `url` without its query, its fragment and its user, when its path ends in one of
/// [`MODEL_PATHS`]; `None` for any other URL.
pub(crate) fn model_url(url: &str) -> Option<String> {
    let mut url = Url::parse(url).ok()?;
    let path = url.path().trim_end_matches('/');
    if !MODEL_PATHS.iter().any(|end| path.ends_with(end)) {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Some(url.to_string())
}

/// Makes `<state>/streams/<UTC time>-<pid>/`, each new directory at mode 0700.
fn run_dir(state: &Path, now: Timestamp, pid: u32) -> Result<PathBuf, StdxError> {
    std::fs::create_dir_all(state)
        .map_err(|source| StdxError::CreateDir { path: state.to_path_buf(), source })?;
    let streams = state.join(STREAMS_DIR);
    // NOTE: a directory of an earlier start is taken, which is fine: each start makes
    // its own directory inside it.
    let _ = claim_dir(&streams)?;
    let stamp = now.strftime("%Y%m%dT%H%M%SZ");
    let mut dir = streams.join(format!("{stamp}-{pid}"));
    let mut extra = 1;
    // NOTE: a second start in the same second with the same pid only happens in tests.
    while matches!(claim_dir(&dir)?, Claim::Taken) {
        dir = streams.join(format!("{stamp}-{pid}-{extra}"));
        extra += 1;
    }
    Ok(dir)
}

/// One model call whose answer the writer appends to its file.
struct Open {
    n: u64,
    at: Timestamp,
    transport: &'static str,
    status: Option<u16>,
    response: Option<File>,
}

/// The writer thread's state.
struct Writer {
    dir: PathBuf,
    next: u64,
    /// The open HTTP calls, by exchange.
    http: HashMap<u64, Open>,
    /// The open call of each WebSocket, by exchange: one at a time on a socket.
    sockets: HashMap<u64, Open>,
    /// `index.jsonl`, from the end of the first call.
    index: Option<File>,
    /// True once a write failed, so the warning comes once.
    failed: bool,
}

impl Writer {
    fn new(dir: PathBuf) -> Self {
        Writer {
            dir,
            next: 1,
            http: HashMap::new(),
            sockets: HashMap::new(),
            index: None,
            failed: false,
        }
    }

    fn run(mut self, items: &Receiver<Item>) {
        while let Ok(item) = items.recv() {
            self.write(item);
        }
        let open: Vec<Open> =
            self.http.drain().chain(self.sockets.drain()).map(|(_, open)| open).collect();
        for open in open {
            self.index(&open, "dropped");
        }
    }

    fn write(&mut self, item: Item) {
        match item {
            Item::Request { exchange, at, method, url, body } => {
                let open = self.open(at, "http", &method, &url, &body);
                self.http.insert(exchange, open);
            }
            Item::Response { exchange, status } => {
                if let Some(open) = self.http.get_mut(&exchange) {
                    open.status = Some(status);
                }
            }
            Item::Chunk { exchange, bytes } => {
                if let Some(mut open) = self.http.remove(&exchange) {
                    self.append(&mut open, &bytes);
                    self.http.insert(exchange, open);
                }
            }
            Item::End { exchange, end } => {
                if let Some(open) = self.http.remove(&exchange) {
                    self.index(&open, end);
                }
            }
            Item::Socket { exchange, at, url, sent: true, text } => {
                if let Some(open) = self.sockets.remove(&exchange) {
                    self.index(&open, "dropped");
                }
                let mut open = self.open(at, "websocket", "GET", &url, text.as_bytes());
                open.status = Some(101);
                self.sockets.insert(exchange, open);
            }
            Item::Socket { exchange, sent: false, text, .. } => {
                let Some(mut open) = self.sockets.remove(&exchange) else { return };
                let kind = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|value| value.get("type")?.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "message".to_owned());
                // NOTE: one data line: a message holds no line break of its own, and
                // one that did would no longer read as one SSE event.
                let data = text.replace(['\r', '\n'], " ");
                self.append(&mut open, format!("event: {kind}\ndata: {data}\n\n").as_bytes());
                match SOCKET_ENDS.iter().find(|(ends, _)| *ends == kind) {
                    Some((_, end)) => self.index(&open, end),
                    None => {
                        self.sockets.insert(exchange, open);
                    }
                }
            }
        }
    }

    /// Starts call `n`: its request file and its empty answer file.
    fn open(
        &mut self,
        at: Timestamp,
        transport: &'static str,
        method: &str,
        url: &str,
        body: &[u8],
    ) -> Open {
        let n = self.next;
        self.next += 1;
        let body = serde_json::from_slice::<Value>(body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into_owned()));
        let request = json!({ "transport": transport, "method": method, "url": url, "body": body });
        let text = serde_json::to_string_pretty(&request).unwrap_or_default() + "\n";
        let written = create_private(&self.dir.join(format!("{n}.request.json")))
            .map_err(|error| error.to_string())
            .and_then(|mut file| {
                file.write_all(text.as_bytes()).map_err(|error| error.to_string())
            });
        self.check(written);
        let response = create_private(&self.dir.join(format!("{n}.response.sse")));
        let response = match response {
            Ok(file) => Some(file),
            Err(error) => {
                self.check(Err(error.to_string()));
                None
            }
        };
        Open { n, at, transport, status: None, response }
    }

    fn append(&mut self, open: &mut Open, bytes: &[u8]) {
        let written = match open.response.as_mut() {
            Some(file) => file.write_all(bytes).map_err(|error| error.to_string()),
            None => Ok(()),
        };
        self.check(written);
    }

    /// Adds the line of a finished call to `index.jsonl`.
    fn index(&mut self, open: &Open, end: &str) {
        let line = json!({
            "n": open.n,
            "at": open.at.to_string(),
            "transport": open.transport,
            "status": open.status,
            "end": end,
        });
        if self.index.is_none() {
            match create_private(&self.dir.join("index.jsonl")) {
                Ok(file) => self.index = Some(file),
                Err(error) => return self.check(Err(error.to_string())),
            }
        }
        let written = match self.index.as_mut() {
            Some(file) => {
                file.write_all(format!("{line}\n").as_bytes()).map_err(|error| error.to_string())
            }
            None => Ok(()),
        };
        self.check(written);
    }

    fn check(&mut self, written: Result<(), String>) {
        if let Err(error) = written
            && !self.failed
        {
            self.failed = true;
            tracing::warn!(
                error = %error,
                dir = %self.dir.display(),
                "a recorded stream could not be written"
            );
        }
    }
}

#[cfg(test)]
mod tests;
