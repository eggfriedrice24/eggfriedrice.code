//! `bench_perf_stream_cadence`: how evenly the text of an answer goes from the model to
//! the frames that `efr send` writes.
//!
//! The parts are real: efrd runs in this process on the system clock with a file store
//! (`TestDaemon`), and the built `efr send` follows the turn on a pseudo-terminal of
//! 50 rows and 120 columns. Only the model is a fake: `PacedModel` hands over each
//! token at a set real time. Each token holds a marker (`zq` and five digits), so the
//! bench finds it at each point of its way, from the timing marks (`efr_stdx::mark`)
//! that efrd and `efr` append to one file:
//!
//! - `prov`: the model handed the token over (the arrival, from `PacedCall`);
//! - `stream.text`: the stream loop of the turn took it;
//! - `draft.sent`: the turn sent the draft that holds it;
//! - `sub.draft`: the task of the subscription took that draft;
//! - `conn.flushed`: efrd flushed the frame that holds it to the socket;
//! - `client.read`: `efr` decoded that frame;
//! - `follow.draft`: the follow loop of `efr` took the draft;
//! - `out.begin`, `out.end`: `efr` began and ended the write of the frame that shows it;
//! - `pty`: the read of the pseudo-terminal that holds it, in this process.
//!
//! The lines of each scenario: the latency of a token from its arrival to the end of
//! the write of its frame (p50, p95, max over all tokens of all runs), the gaps between
//! text frames, the stalls (a text frame whose oldest new token waited more than
//! 50 ms) inside the stream, the end of the turn (the last token to its frame, the last
//! token to the exit, and the runs whose end waited more than 50 ms), the
//! `record_text` awaits of the stream loop and the jobs of the store's writer, the time
//! between two points of the way, and the start of the turn. The scenario `end` runs
//! 12 short answers whose length and start move by a few ms each, because the wait at
//! the end of a turn depends on its phase against the clock of `efr`.
//!
//! The variables of a run, all optional (the bench reads them for research by hand):
//!
//! - `EFR_CADENCE_SCENARIOS`: a list with commas of `steady30`, `steady60`,
//!   `steady120`, `burst`, `reason`, `long`, `flat`, `mixed`, `code` and `end`;
//!   `steady60,steady120,burst,end` by default.
//! - `EFR_CADENCE_RUNS`: the measured runs of each scenario but `end` (3).
//! - `EFR_CADENCE_UPDATE_MS`, `EFR_CADENCE_DRAFT_MS`: `conversation.update_interval_ms`
//!   and `conversation.draft_interval_ms` of the daemon.
//! - `EFR_CADENCE_LOAD`: `none`, `second` (a second conversation streams at the same
//!   time), `sub` (a second subscriber with drafts on the same conversation), `stalled`
//!   (a second subscriber that does not read) or `backlog` (many short turns first,
//!   so the search indexer has work during the run).

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader};
use std::os::fd::OwnedFd;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::framing;
use efr_protocol::{
    Capabilities, ClientFrame, ConversationId, ConversationSubscribe, ConversationsList,
    ConversationsListResult, Hello, Method, Origin, PROTOCOL_VERSION, PromptSendResult, RequestId,
};
use efr_stdx::env::Var;
use efr_stdx::mark;
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::{TTY, TestDaemon};
use efr_test_support::bench::Report;
use efr_test_support::{PacedCall, PacedDelta, PacedModel, TestDirs};
use futures::StreamExt as _;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;

/// The terminal of the second conversation.
const OTHER_TTY: &str = "/dev/pts/cadence-other";

/// How long the model waits before its first delta.
const MODEL_FIRST: Duration = Duration::from_millis(300);

/// The rows and columns of the pseudo-terminal.
const SIZE: (u16, u16) = (50, 120);

/// The runs of the scenario `end`.
const END_RUNS: usize = 12;

/// A text frame whose oldest new token waited longer than this is a stall.
const STALL: Duration = Duration::from_millis(50);

/// The points of the way of a token, in order; `prov` and `pty` are the bench's own.
const STAGES: [&str; 10] = [
    "prov",
    "stream.text",
    "draft.sent",
    "sub.draft",
    "conn.flushed",
    "client.read",
    "follow.draft",
    "out.begin",
    "out.end",
    "pty",
];

/// The points of [`STAGES`] that efrd marks; the others but `prov` and `pty` are of
/// `efr`.
const DAEMON_STAGES: [&str; 4] = ["stream.text", "draft.sent", "sub.draft", "conn.flushed"];

/// The bench reads its own switches.
#[expect(clippy::disallowed_methods, reason = "the bench reads its own switches")]
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn number(name: &str) -> Option<u64> {
    var(name).map(|value| value.parse().unwrap_or_else(|_| panic!("{name} is no number")))
}

/// The text of token `n` with the marker prefix `prefix`: a space before it, a full
/// stop after each 15th token, and a blank line before each `paragraph`-th token.
fn token(prefix: &str, n: usize, paragraph: usize) -> String {
    let before = if n == 0 {
        ""
    } else if paragraph > 0 && n.is_multiple_of(paragraph) {
        "\n\n"
    } else {
        " "
    };
    let after = if (n + 1).is_multiple_of(15) { "." } else { "" };
    format!("{before}{prefix}{n:05}{after}")
}

/// `count` text tokens from token `first` on, `rate` each second from `start`.
fn steady(
    prefix: &str,
    first: usize,
    count: usize,
    rate: f64,
    start: Duration,
    paragraph: usize,
) -> Vec<PacedDelta> {
    (0..count)
        .map(|n| PacedDelta {
            at: start + Duration::from_secs_f64(n as f64 / rate),
            reasoning: false,
            text: token(prefix, first + n, paragraph),
        })
        .collect()
}

/// The tokens of `texts`, 60 each second from [`MODEL_FIRST`].
fn paced(texts: Vec<String>) -> Vec<PacedDelta> {
    texts
        .into_iter()
        .enumerate()
        .map(|(n, text)| PacedDelta {
            at: MODEL_FIRST + Duration::from_secs_f64(n as f64 / 60.0),
            reasoning: false,
            text,
        })
        .collect()
}

/// Adds `count` tokens to `texts`: the first after `first`, the others after a space.
fn tokens(texts: &mut Vec<String>, prefix: &str, first: &str, count: usize) {
    for n in 0..count {
        let before = if n == 0 { first } else { " " };
        texts.push(format!("{before}{prefix}{:05}", texts.len()));
    }
}

/// An answer in Markdown: a heading, a paragraph, a list of 6 items, a code block of
/// 40 lines and a paragraph.
fn mixed(prefix: &str) -> Vec<PacedDelta> {
    let mut texts = Vec::new();
    tokens(&mut texts, prefix, "## ", 5);
    tokens(&mut texts, prefix, "\n\n", 60);
    for item in 0..6 {
        tokens(&mut texts, prefix, if item == 0 { "\n\n- " } else { "\n- " }, 12);
    }
    for line in 0..40 {
        tokens(&mut texts, prefix, if line == 0 { "\n\n```rust\n" } else { "\n" }, 8);
    }
    tokens(&mut texts, prefix, "\n```\n\n", 60);
    paced(texts)
}

/// An answer that is one code block of 100 lines, more than 4 KiB.
fn code(prefix: &str) -> Vec<PacedDelta> {
    let mut texts = Vec::new();
    for line in 0..100 {
        tokens(&mut texts, prefix, if line == 0 { "```rust\n" } else { "\n" }, 8);
    }
    tokens(&mut texts, prefix, "\n```\n\n", 20);
    paced(texts)
}

/// The script of the scenario `name`, run `run` (from 0), or `None` for an unknown
/// name.
fn scenario(name: &str, run: usize) -> Option<Vec<PacedDelta>> {
    let prefix = "zq";
    let script = match name {
        "steady30" => steady(prefix, 0, 150, 30.0, MODEL_FIRST, 60),
        "steady60" => steady(prefix, 0, 300, 60.0, MODEL_FIRST, 60),
        "steady120" => steady(prefix, 0, 600, 120.0, MODEL_FIRST, 60),
        "long" => steady(prefix, 0, 2000, 60.0, MODEL_FIRST, 60),
        "mixed" => mixed(prefix),
        "code" => code(prefix),
        // One paragraph with no line break: the live block grows past 4 KiB.
        "flat" => steady(prefix, 0, 2000, 60.0, MODEL_FIRST, 0),
        // 20 tokens at once, each 300 ms.
        "burst" => (0..300)
            .map(|n| PacedDelta {
                at: MODEL_FIRST + Duration::from_millis(300 * (n / 20) as u64),
                reasoning: false,
                text: token(prefix, n, 60),
            })
            .collect(),
        // Reasoning for 4 s at 60 deltas a second, then text.
        "reason" => {
            let mut script = vec![PacedDelta {
                at: MODEL_FIRST,
                reasoning: true,
                text: "**Planning the answer**\n\n".to_owned(),
            }];
            script.extend((1..240).map(|n| PacedDelta {
                at: MODEL_FIRST + Duration::from_secs_f64(f64::from(n) / 60.0),
                reasoning: true,
                text: format!(" step{n}"),
            }));
            script.extend(steady(prefix, 0, 300, 60.0, MODEL_FIRST + Duration::from_secs(4), 60));
            script
        }
        // A short answer whose length and start move with the run: 30 tokens and 7
        // more for each run, from 9 ms later for each run. 12 runs cover more than the
        // 104 ms of the clock of `efr` at the end of a turn.
        "end" => {
            let start = MODEL_FIRST + Duration::from_millis(9 * run as u64);
            steady(prefix, 0, 30 + 7 * run, 60.0, start, 60)
        }
        _ => return None,
    };
    Some(script)
}

/// What one `efr send` wrote, and when.
struct Shown {
    spawn_ns: u64,
    exit_ns: u64,
    pid: u32,
    ok: bool,
    /// Each read of the pseudo-terminal: the time, the largest marker so far plus 1 in
    /// its bytes (0 without one) and the count of its bytes.
    chunks: Vec<(u64, u64, usize)>,
    bytes: Vec<u8>,
}

/// Runs `efr send` with `prompt` on a pseudo-terminal against `daemon`, with its marks
/// in `marks`, and reads what it writes until it exits.
async fn efr_send(daemon: &TestDaemon, marks: &Path, prompt: &str) -> Shown {
    let context = json!({ "pwd": daemon.cwd(), "tty": TTY }).to_string();
    let (master, slave): (OwnedFd, OwnedFd) = super::pty_of(SIZE.0, SIZE.1);
    let mut command = efr_stdx::process::command(env!("CARGO_BIN_EXE_efr"), daemon.cwd());
    let dirs = daemon.dirs().dirs();
    command
        .env_clear()
        .env("HOME", daemon.dirs().home())
        .env("PATH", "/usr/bin:/bin")
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("EFR_CONFIG_DIR", dirs.config())
        .env("EFR_DATA_DIR", dirs.data())
        .env("EFR_STATE_DIR", dirs.state())
        .env("EFR_RUNTIME_DIR", dirs.runtime())
        .env(Var::Mark.name(), marks)
        .args(["send", "--context-json", &context, "--", prompt])
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
    let spawn_ns = mark::now_ns();
    let mut child = command.spawn().unwrap();
    let pid = child.id().unwrap_or(0);
    // NOTE: with no slave open in this process, the read of the master ends when efr
    // exits.
    drop(command);
    drop(slave);
    let reader = std::thread::spawn(move || {
        let mut chunks = Vec::new();
        let mut bytes = Vec::new();
        let mut buffer = vec![0_u8; 256 * 1024];
        while let Ok(read) = rustix::io::read(&master, &mut buffer) {
            if read == 0 {
                break;
            }
            let now = mark::now_ns();
            // NOTE: a marker can lie across two reads, so the scan starts 6 bytes back.
            let from = bytes.len().saturating_sub(6);
            bytes.extend_from_slice(&buffer[..read]);
            chunks.push((now, mark::scan(&bytes[from..]), read));
        }
        (chunks, bytes)
    });
    let status = child.wait().await.unwrap();
    let exit_ns = mark::now_ns();
    let (chunks, bytes) =
        tokio::task::spawn_blocking(move || reader.join().unwrap()).await.unwrap();
    Shown { spawn_ns, exit_ns, pid, ok: status.success(), chunks, bytes }
}

/// The CPU time of this process (efrd, the model and the reads of the bench) and of
/// the children that it waited for (`efr`), in clock ticks, from `/proc/self/stat`.
fn cpu_ticks() -> (u64, u64) {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // NOTE: the fields after the command, which stands in brackets and may hold spaces.
    let fields: Vec<u64> = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().map(|field| field.parse().unwrap_or(0)).collect())
        .unwrap_or_default();
    let at = |index: usize| fields.get(index).copied().unwrap_or(0);
    // utime and stime are fields 14 and 15 of the line, cutime and cstime 16 and 17.
    (at(11) + at(12), at(13) + at(14))
}

/// The conversation of [`TTY`].
async fn conversation(daemon: &TestDaemon) -> ConversationId {
    let client = daemon.client().await.unwrap();
    let list =
        Method::ConversationsList(ConversationsList { cursor: None, limit: None, tty: None });
    let list: ConversationsListResult = client.call(list).await.unwrap();
    list.conversations
        .iter()
        .find(|conversation| conversation.tty.as_deref() == Some(TTY))
        .expect("the warm-up made the conversation")
        .id
}

/// A second subscriber of `conversation_id` with drafts that reads each item at once,
/// until the returned task is aborted.
async fn subscriber(
    daemon: &TestDaemon,
    conversation_id: ConversationId,
) -> tokio::task::JoinHandle<()> {
    let client = daemon.client().await.unwrap();
    let params = ConversationSubscribe {
        conversation_id,
        after_seq: None,
        answers_input: false,
        drafts: true,
    };
    let mut stream = client.stream::<Value>(Method::ConversationSubscribe(params)).await.unwrap();
    tokio::spawn(async move {
        let _client = client;
        while stream.next().await.is_some() {}
    })
}

/// A second subscriber of `conversation_id` with drafts that never reads its socket.
async fn stalled_subscriber(
    daemon: &TestDaemon,
    conversation_id: ConversationId,
) -> tokio::net::UnixStream {
    let mut socket = tokio::net::UnixStream::connect(daemon.socket_path()).await.unwrap();
    let hello = Method::Hello(Hello {
        protocol: PROTOCOL_VERSION,
        origin: Origin::Cli,
        client: Some("stalled".to_owned()),
        capabilities: Capabilities::default(),
        tty: None,
        pid: None,
        device_id: None,
    });
    let subscribe = Method::ConversationSubscribe(ConversationSubscribe {
        conversation_id,
        after_seq: None,
        answers_input: false,
        drafts: true,
    });
    for (id, method) in [(1, hello), (2, subscribe)] {
        let frame = ClientFrame::Request { id: RequestId::new(id), method };
        socket.write_all(&framing::encode(&frame).unwrap()).await.unwrap();
    }
    socket
}

/// Waits until the model got call number `calls`.
async fn called(model: &PacedModel, calls: usize) {
    while model.calls().len() < calls {
        SystemClock.sleep(Duration::from_millis(2)).await;
    }
}

/// Waits until the model handed over the end of call number `call` (from 0).
async fn ended(model: &PacedModel, call: usize) {
    while model.calls().get(call).is_none_or(|call| call.done_ns == 0) {
        SystemClock.sleep(Duration::from_millis(5)).await;
    }
}

/// One mark of the file.
struct Line {
    ns: u64,
    pid: u32,
    name: String,
    a: u64,
}

/// The marks of the file at `path`, in the order of their time.
fn read_marks(path: &Path) -> Vec<Line> {
    let file = std::fs::File::open(path).unwrap();
    let mut lines: Vec<Line> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| {
            let mut parts = line.split(' ');
            let ns = parts.next()?.parse().ok()?;
            let pid = parts.next()?.parse().ok()?;
            let name = parts.next()?.to_owned();
            let a = parts.next()?.parse().ok()?;
            Some(Line { ns, pid, name, a })
        })
        .collect();
    lines.sort_by_key(|line| line.ns);
    lines
}

/// One measured run.
struct Run {
    scenario: String,
    begin_ns: u64,
    end_ns: u64,
    shown: Shown,
    call: PacedCall,
    cpu_self: u64,
    cpu_efr: u64,
}

/// For token `k` of `count`, the first time at which a record held a marker above `k`.
fn first_at(records: impl Iterator<Item = (u64, u64)>, count: usize) -> Vec<Option<u64>> {
    let mut out = vec![None; count];
    let mut best = 0_usize;
    for (ns, marker) in records {
        let marker = usize::try_from(marker).unwrap_or(usize::MAX);
        if marker > best {
            for slot in out.iter_mut().take(marker.min(count)).skip(best) {
                *slot = Some(ns);
            }
            best = marker;
        }
    }
    out
}

fn span(from: Option<u64>, to: Option<u64>) -> Option<Duration> {
    Some(Duration::from_nanos(to?.checked_sub(from?)?))
}

/// What the runs of one scenario measured, pooled.
#[derive(Default)]
struct Pooled {
    runs: u64,
    failed: u64,
    tokens: u64,
    lost: u64,
    latency: Vec<Duration>,
    latency_pty: Vec<Duration>,
    frame_gaps: Vec<Duration>,
    text_frames: u64,
    stream_ns: u64,
    stalls: u64,
    longest_stall: Duration,
    stall_stages: BTreeMap<String, u64>,
    end_frame: Vec<Duration>,
    end_exit: Vec<Duration>,
    end_stalls: u64,
    record: Vec<Duration>,
    jobs: Vec<Duration>,
    stages: BTreeMap<String, Vec<Duration>>,
    first_byte: Vec<Duration>,
    model_call: Vec<Duration>,
    first_frame: Vec<Duration>,
    cpu_self: Vec<u64>,
    cpu_efr: Vec<u64>,
}

impl Pooled {
    /// Adds `run`, whose marks are `marks` (all of the file, in time order).
    fn add(&mut self, run: &Run, marks: &[Line]) {
        self.runs += 1;
        if !run.shown.ok {
            self.failed += 1;
        }
        let daemon = std::process::id();
        let child = run.shown.pid;
        let lo = marks.partition_point(|line| line.ns < run.begin_ns);
        let hi = marks.partition_point(|line| line.ns <= run.end_ns);
        let window = &marks[lo..hi];
        let prov: Vec<u64> = run
            .call
            .sent
            .iter()
            .filter(|(marker, reasoning, _)| !reasoning && *marker > 0)
            .map(|(_, _, ns)| *ns)
            .collect();
        let count = prov.len();
        self.tokens += count as u64;
        let mut at: BTreeMap<&str, Vec<Option<u64>>> = BTreeMap::new();
        at.insert("prov", prov.iter().copied().map(Some).collect());
        for stage in &STAGES[1..STAGES.len() - 1] {
            let pid = if DAEMON_STAGES.contains(stage) { daemon } else { child };
            let records = window
                .iter()
                .filter(|line| line.pid == pid && line.name == *stage)
                .map(|line| (line.ns, line.a));
            at.insert(stage, first_at(records, count));
        }
        at.insert(
            "pty",
            first_at(run.shown.chunks.iter().map(|(ns, marker, _)| (*ns, *marker)), count),
        );
        let shown = &at["out.end"];
        self.lost += shown.iter().filter(|at| at.is_none()).count() as u64;
        for k in 0..count {
            if let Some(latency) = span(Some(prov[k]), shown[k]) {
                self.latency.push(latency);
            }
            if let Some(latency) = span(Some(prov[k]), at["pty"][k]) {
                self.latency_pty.push(latency);
            }
        }
        for pair in STAGES.windows(2) {
            let samples = self.stages.entry(format!("{} -> {}", pair[0], pair[1])).or_default();
            samples.extend((0..count).filter_map(|k| span(at[pair[0]][k], at[pair[1]][k])));
        }

        // The text frames: the writes that show a token for the first time, each with
        // the first token that it shows and the token after its last.
        let mut frames = Vec::new();
        let mut best = 0;
        for line in window.iter().filter(|line| line.pid == child && line.name == "out.end") {
            if line.a > best {
                frames.push((line.ns, usize::try_from(best).unwrap_or(usize::MAX)));
                best = line.a;
            }
        }
        self.text_frames += frames.len() as u64;
        if let (Some(first), Some(last)) = (frames.first(), frames.last()) {
            self.stream_ns += last.0.saturating_sub(first.0);
        }
        self.frame_gaps
            .extend(frames.windows(2).map(|pair| Duration::from_nanos(pair[1].0 - pair[0].0)));
        for (ns, oldest) in &frames {
            let Some(arrived) = prov.get(*oldest) else { continue };
            let wait = Duration::from_nanos(ns.saturating_sub(*arrived));
            // NOTE: the last three tokens belong to the end of the turn, counted below.
            if wait <= STALL || *oldest + 3 >= count {
                continue;
            }
            self.stalls += 1;
            self.longest_stall = self.longest_stall.max(wait);
            let worst = STAGES[..STAGES.len() - 1]
                .windows(2)
                .filter_map(|pair| {
                    span(at[pair[0]][*oldest], at[pair[1]][*oldest])
                        .map(|time| (time, format!("{} -> {}", pair[0], pair[1])))
                })
                .max();
            let stage = worst.map_or_else(|| "?".to_owned(), |(_, stage)| stage);
            *self.stall_stages.entry(stage).or_default() += 1;
        }
        if let Some(last) = prov.last() {
            if let Some(wait) = span(Some(*last), shown[count - 1]) {
                self.end_frame.push(wait);
                if wait > STALL {
                    self.end_stalls += 1;
                }
            }
            if let Some(exit) = span(Some(*last), Some(run.shown.exit_ns)) {
                self.end_exit.push(exit);
            }
        }

        // The awaits of `record_text` in the stream loop, and the writer's jobs.
        let mut began = None;
        for line in window.iter().filter(|line| line.pid == daemon) {
            match line.name.as_str() {
                "stream.record.begin" => began = Some(line.ns),
                "stream.record.end" => {
                    if let Some(time) = span(began.take(), Some(line.ns)) {
                        self.record.push(time);
                    }
                }
                "store.job" => self.jobs.push(Duration::from_nanos(line.a)),
                _ => {}
            }
        }

        let spawn = Some(run.shown.spawn_ns);
        if let Some(time) = span(spawn, run.shown.chunks.first().map(|chunk| chunk.0)) {
            self.first_byte.push(time);
        }
        if let Some(time) = span(spawn, Some(run.call.called_ns)) {
            self.model_call.push(time);
        }
        if let Some(time) = span(spawn, shown.first().copied().flatten()) {
            self.first_frame.push(time);
        }
        self.cpu_self.push(run.cpu_self);
        self.cpu_efr.push(run.cpu_efr);
    }

    fn report(&self, report: &mut Report, scenario: &str) {
        let label = |what: &str| format!("{scenario}|{what}");
        report.count(&label("runs"), self.runs);
        report.count(&label("runs that failed"), self.failed);
        report.count(&label("tokens"), self.tokens);
        report.count(&label("tokens never written"), self.lost);
        report.time(&label("arrival -> frame written"), &self.latency);
        let mut sorted = self.latency.clone();
        sorted.sort();
        let p99 = ((sorted.len().saturating_sub(1)) as f64 * 0.99).round() as usize;
        if let Some(p99) = sorted.get(p99) {
            report.time(&label("arrival -> frame written, p99 (one value)"), &[*p99]);
        }
        report.time(&label("arrival -> read of the terminal"), &self.latency_pty);
        report.time(&label("gap between text frames"), &self.frame_gaps);
        if self.stream_ns > 0 {
            let rate = self.text_frames as f64 / (self.stream_ns as f64 / 1e9);
            report.count(&label("text frames a second"), rate.round() as u64);
        }
        report.count(&label("stalls over 50 ms inside the stream"), self.stalls);
        report.time(&label("longest stall inside the stream"), &[self.longest_stall]);
        for (stage, count) in &self.stall_stages {
            report.count(&label(&format!("stalls over 50 ms, longest at {stage}")), *count);
        }
        report.time(&label("end: last token -> its frame"), &self.end_frame);
        report.time(&label("end: last token -> exit"), &self.end_exit);
        report.count(&label("end: runs whose last token waited over 50 ms"), self.end_stalls);
        report.time(&label("record_text await"), &self.record);
        report.count(
            &label("record_text awaits over 5 ms"),
            self.record.iter().filter(|time| **time > Duration::from_millis(5)).count() as u64,
        );
        report.time(&label("store writer job"), &self.jobs);
        for (stage, samples) in &self.stages {
            report.time(&label(&format!("stage {stage}")), samples);
        }
        report.time(&label("start: spawn -> first byte"), &self.first_byte);
        report.time(&label("start: spawn -> model call"), &self.model_call);
        report.time(&label("start: spawn -> first text frame"), &self.first_frame);
        let median = |values: &[u64]| {
            let mut values = values.to_vec();
            values.sort_unstable();
            values.get(values.len() / 2).copied().unwrap_or(0)
        };
        report
            .count(&label("cpu of efrd and the model, clock ticks a run"), median(&self.cpu_self));
        report.count(&label("cpu of efr, clock ticks a run"), median(&self.cpu_efr));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_stream_cadence() {
    let names =
        var("EFR_CADENCE_SCENARIOS").unwrap_or_else(|| "steady60,steady120,burst,end".to_owned());
    let runs = usize::try_from(number("EFR_CADENCE_RUNS").unwrap_or(3)).unwrap();
    let load = var("EFR_CADENCE_LOAD").unwrap_or_else(|| "none".to_owned());
    let update_ms = number("EFR_CADENCE_UPDATE_MS");
    let draft_ms = number("EFR_CADENCE_DRAFT_MS");
    let mut report = Report::new("bench_perf_stream_cadence");

    let dirs = Arc::new(TestDirs::new_in(Path::new(env!("CARGO_TARGET_TMPDIR"))).unwrap());
    let marks = dirs.root().join("marks.log");
    mark::enable_file(&marks).unwrap();
    let model = PacedModel::new();
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&dirs))
        .persistent()
        .real_time()
        .custom_provider(Arc::clone(&model) as Arc<_>)
        .config(move |settings| {
            if let Some(update_ms) = update_ms {
                settings.conversation.update_interval_ms = update_ms;
            }
            if let Some(draft_ms) = draft_ms {
                settings.conversation.draft_interval_ms = draft_ms;
            }
        })
        .start()
        .await
        .unwrap();

    // The first prompt after the start makes the conversation and the hidden shell.
    model.push(steady("zq", 0, 5, 60.0, MODEL_FIRST, 60));
    let shown = efr_send(&daemon, &marks, "warm up").await;
    assert!(shown.ok, "{}", String::from_utf8_lossy(&shown.bytes));
    let conversation_id = conversation(&daemon).await;
    let mut prompts = 0_u64;

    if load == "backlog" {
        // Short turns one after the other: the indexer writes at most one page a second
        // while a turn runs, so it is behind when the measured run starts.
        let turns = number("EFR_CADENCE_BACKLOG").unwrap_or(400);
        let client = daemon.client_for_tty(OTHER_TTY).await.unwrap();
        for n in 0..turns {
            let text = "The build is fine and the tests pass. ".repeat(40);
            model.push(vec![PacedDelta { at: Duration::ZERO, reasoning: false, text }]);
            let before = model.calls().len();
            let prompt = daemon.prompt(u128::from(1000 + n), "how is the build?", OTHER_TTY);
            let _: PromptSendResult = client.call(prompt).await.unwrap();
            called(&model, before + 1).await;
            ended(&model, before).await;
        }
        SystemClock.sleep(Duration::from_millis(300)).await;
    }

    let mut measured = Vec::new();
    for name in names.split(',').map(str::trim) {
        let count = if name == "end" { END_RUNS } else { runs };
        for run in 0..count {
            let script = scenario(name, run).unwrap_or_else(|| panic!("no scenario {name}"));
            let length = script.last().map_or(Duration::ZERO, |delta| delta.at);
            let mut other = None;
            let mut helpers = Vec::new();
            let mut stalled = None;
            match load.as_str() {
                "second" => {
                    let client = daemon.client_for_tty(OTHER_TTY).await.unwrap();
                    let before = model.calls().len();
                    let count = ((length + Duration::from_secs(3)).as_secs_f64() * 60.0) as usize;
                    model.push(steady("zx", 0, count, 60.0, MODEL_FIRST, 60));
                    prompts += 1;
                    let prompt = daemon.prompt(u128::from(prompts), "other", OTHER_TTY);
                    let sent: PromptSendResult = client.call(prompt).await.unwrap();
                    helpers.push(subscriber(&daemon, sent.conversation_id).await);
                    called(&model, before + 1).await;
                    other = Some(before);
                    drop(client);
                }
                "sub" => helpers.push(subscriber(&daemon, conversation_id).await),
                "stalled" => stalled = Some(stalled_subscriber(&daemon, conversation_id).await),
                _ => {}
            }
            let main_call = model.calls().len();
            model.push(script);
            let begin_ns = mark::now_ns();
            let cpu_before = cpu_ticks();
            let shown = efr_send(&daemon, &marks, &format!("{name} {run}")).await;
            let end_ns = mark::now_ns();
            let cpu_after = cpu_ticks();
            let call = model.calls().get(main_call).cloned().unwrap_or_default();
            measured.push(Run {
                scenario: name.to_owned(),
                begin_ns,
                end_ns,
                shown,
                call,
                cpu_self: cpu_after.0 - cpu_before.0,
                cpu_efr: cpu_after.1 - cpu_before.1,
            });
            if let Some(other) = other {
                ended(&model, other).await;
            }
            for helper in helpers {
                helper.abort();
            }
            drop(stalled);
            // The end of the turn, the debounce of the indexer and its round.
            SystemClock.sleep(Duration::from_millis(1500)).await;
        }
    }
    daemon.stop().await.unwrap();

    let marks = read_marks(&marks);
    let mut pooled: Vec<(String, Pooled)> = Vec::new();
    for run in &measured {
        let at = match pooled.iter().position(|(name, _)| *name == run.scenario) {
            Some(at) => at,
            None => {
                pooled.push((run.scenario.clone(), Pooled::default()));
                pooled.len() - 1
            }
        };
        pooled[at].1.add(run, &marks);
    }
    report.count(
        "load (0 none, 1 second, 2 sub, 3 stalled, 4 backlog)",
        match load.as_str() {
            "second" => 1,
            "sub" => 2,
            "stalled" => 3,
            "backlog" => 4,
            _ => 0,
        },
    );
    for (name, pooled) in &pooled {
        pooled.report(&mut report, name);
    }
    report.finish();
}

/// `bench_perf_draft_passes`: the CPU time of the passes that each text draft takes on
/// its way: efrd turns the item into a JSON value and the frame into bytes
/// (`Responder::item`), and `efr` splits the bytes into a payload, reads the frame with
/// the item as a JSON value, and reads the item from that value (the client's read
/// task and `Follower::item`). The draft holds a delta of 8, 64 and 512 bytes.
#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_draft_passes() {
    use efr_protocol::framing::Decoder;
    use efr_protocol::{ConversationSubscribeItem, Draft, DraftPart, Seq, ServerFrame};
    use efr_test_support::bench;

    const BATCH: u32 = 1000;
    let mut report = Report::new("bench_perf_draft_passes");
    let turn_id: efr_protocol::TurnId = "019a9b1c-3d00-7a10-8b20-000000000002".parse().unwrap();
    let per = |samples: Vec<Duration>| -> Vec<Duration> {
        samples.into_iter().map(|sample| sample / BATCH).collect()
    };
    for size in [8_usize, 64, 512] {
        let item = ConversationSubscribeItem::Draft(Draft {
            turn_id,
            after_seq: Seq::new(123_456),
            draft: DraftPart::Text { index: 0, offset: 4096, delta: "zq00000 ".repeat(size / 8) },
        });
        let id = RequestId::new(3);
        let bytes = framing::encode(&ServerFrame::item(id, &item).unwrap()).unwrap();
        report.bytes(&format!("delta {size} frame"), bytes.len() as u64);
        let to_value = bench::run(20, 200, || {
            for _ in 0..BATCH {
                std::hint::black_box(ServerFrame::item(id, std::hint::black_box(&item)).unwrap());
            }
        });
        report.time(&format!("delta {size} efrd item to value"), &per(to_value));
        let frame = ServerFrame::item(id, &item).unwrap();
        let to_bytes = bench::run(20, 200, || {
            for _ in 0..BATCH {
                std::hint::black_box(framing::encode(std::hint::black_box(&frame)).unwrap());
            }
        });
        report.time(&format!("delta {size} efrd frame to bytes"), &per(to_bytes));
        let split = bench::run(20, 200, || {
            for _ in 0..BATCH {
                let mut decoder = Decoder::new();
                std::hint::black_box(decoder.push(std::hint::black_box(&bytes)).unwrap());
            }
        });
        report.time(&format!("delta {size} efr bytes to payload"), &per(split));
        let payload = Decoder::new().push(&bytes).unwrap().remove(0);
        let to_frame = bench::run(20, 200, || {
            for _ in 0..BATCH {
                std::hint::black_box(
                    ServerFrame::from_json(std::hint::black_box(&payload)).unwrap(),
                );
            }
        });
        report.time(&format!("delta {size} efr payload to frame"), &per(to_frame));
        let ServerFrame::Item { item: value, .. } = ServerFrame::from_json(&payload).unwrap()
        else {
            panic!("an item frame");
        };
        let to_item = bench::run(20, 200, || {
            for _ in 0..BATCH {
                let value = std::hint::black_box(&value).clone();
                std::hint::black_box(
                    serde_json::from_value::<ConversationSubscribeItem>(value).unwrap(),
                );
            }
        });
        report.time(&format!("delta {size} efr value to item (with the clone)"), &per(to_item));
    }
    report.finish();
}
