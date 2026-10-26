//! The benches of the built `efr` against a fake daemon on a Unix socket, which
//! `just bench-perf` runs in release:
//!
//! - `bench_perf_cli_first_write`: from the spawn of `efr status` to the first byte
//!   that the fake daemon reads, and to the exit, with a `config.toml` of 0, 50 and 200
//!   permission rules, without and with a theme file (a palette with a `.tmTheme`
//!   code theme). It also times `efr --version` to its exit.
//! - `bench_perf_cli_memory`: the peak resident set (`VmHWM`, from zsh's `time` as
//!   `%M`), the CPU time and the wall time of `efr diff --turn` on a terminal that
//!   renders a diff of 2k, 10k and 20k lines, and of `efr send` on a terminal that
//!   follows a turn whose answer streams a list item with a nested rust fence of 30,
//!   150, 300, 600 and 1,000 lines in 16-byte drafts, one each millisecond. A run that
//!   takes more than 10 s is killed, and its ladder stops there.
//! - `bench_perf_follow_gap`: the wall time of `efr send` in a pipe whose subscription
//!   overflows after the first event and comes back as a snapshot of the newest 128
//!   events, with a gap of 300, 1,000 and 3,000 events before them. It counts the pages
//!   of `conversation.history` that efr reads and checks that each event shows once and
//!   in order.
//! - `bench_perf_cli_diff_all`: the peak resident set, the CPU time, the wall time and
//!   the pages of history of `efr diff --stat --all` on a terminal, for a history of
//!   2k, 10k and 20k events in which each tenth event ends a turn that changed a file.
//!   Budget: at most one page for each 500 events, and the peak grows less than 4 MiB
//!   from 2k to 20k events.
//!
//! `efr` gets a cleared environment with every efr root in a temp dir below cargo's
//! target temp dir and `PATH=/usr/bin:/bin`, so it never reaches the real daemon.

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    AdminStatusResult, ChangeKind, ClientFrame, ConversationDiffResult, ConversationHistoryResult,
    ConversationSnapshot, ConversationStatus, ConversationSubscribeItem, ConversationSummary,
    Draft, DraftPart, ErrorBody, Event, EventEnvelope, FileChange, FileChanges, Method,
    PROTOCOL_VERSION, PageCursor, PromptSendResult, RequestId, Seq, ServerFrame,
};
use efr_test_support::bench::{self, Report};
use serde::Serialize;

use crate::picker::hello_result;

const CONVERSATION: &str = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN: &str = "019a9b1c-3d00-7a10-8b20-000000000002";

/// A case whose run takes longer than this ends its ladder.
const STOP_AFTER: Duration = Duration::from_secs(10);

/// Temp efr roots below cargo's target temp dir.
struct Roots {
    dir: tempfile::TempDir,
}

impl Roots {
    fn new() -> Roots {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf-cli");
        std::fs::create_dir_all(&base).unwrap();
        let dir = tempfile::tempdir_in(base).unwrap();
        for name in ["home", "config", "data", "state", "runtime"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        Roots { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn socket(&self) -> PathBuf {
        self.path("runtime").join("daemon.sock")
    }

    /// `program` with a cleared environment: the roots, a 24-bit colour terminal and a
    /// `PATH` that holds no efr.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = efr_stdx::process::command(program, self.path("home")).into_std();
        command
            .env_clear()
            .current_dir(self.path("home"))
            .env("HOME", self.path("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("EFR_CONFIG_DIR", self.path("config"))
            .env("EFR_DATA_DIR", self.path("data"))
            .env("EFR_STATE_DIR", self.path("state"))
            .env("EFR_RUNTIME_DIR", self.path("runtime"))
            .stdin(Stdio::null());
        command
    }
}

/// One accepted connection of the fake daemon.
struct Conn {
    stream: UnixStream,
    decoder: Decoder,
    ready: VecDeque<Vec<u8>>,
}

impl Conn {
    /// Accepts one connection and gives the time of the first byte that it read.
    fn accept(listener: &UnixListener) -> (Conn, Instant) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0_u8; 4096];
        let read = stream.read(&mut buf).unwrap();
        let first = Instant::now();
        let mut decoder = Decoder::new();
        let ready = decoder.push(&buf[..read]).unwrap().into_iter().collect();
        (Conn { stream, decoder, ready }, first)
    }

    fn recv(&mut self) -> Option<ClientFrame> {
        loop {
            if let Some(payload) = self.ready.pop_front() {
                return Some(ClientFrame::from_json(&payload).unwrap());
            }
            let mut buf = [0_u8; 4096];
            let read = match self.stream.read(&mut buf) {
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => 0,
                Err(error) => panic!("reading from efr failed: {error}"),
            };
            if read == 0 {
                return None;
            }
            self.ready.extend(self.decoder.push(&buf[..read]).unwrap());
        }
    }

    fn request(&mut self) -> (RequestId, Method) {
        match self.recv() {
            Some(ClientFrame::Request { id, method }) => (id, method),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    fn send(&mut self, frame: &ServerFrame) {
        // NOTE: efr may close its end once it has what it needs.
        let _ = self.stream.write_all(&framing::encode(frame).unwrap());
    }

    fn item<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.send(&ServerFrame::item(id, value).unwrap());
    }

    fn reply<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.item(id, value);
        self.send(&ServerFrame::end(id));
    }

    fn hello(&mut self) {
        let (id, method) = self.request();
        assert!(matches!(method, Method::Hello(_)), "hello first, got {}", method.name());
        self.reply(id, &hello_result());
    }

    fn drain(&mut self) {
        while self.recv().is_some() {}
    }
}

/// A fake daemon that serves `runs` connections with `script` after hello, and sends
/// the time of the first byte of each.
fn serve(
    socket: &Path,
    runs: usize,
    script: impl Fn(&mut Conn) + Send + 'static,
) -> (mpsc::Receiver<Instant>, JoinHandle<()>) {
    let listener = UnixListener::bind(socket).unwrap();
    let (firsts, received) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        for _ in 0..runs {
            let (mut conn, first) = Conn::accept(&listener);
            let _ = firsts.send(first);
            conn.hello();
            script(&mut conn);
            conn.drain();
        }
    });
    (received, handle)
}

fn status_result() -> AdminStatusResult {
    AdminStatusResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        version: "0.1.0".to_owned(),
        protocol: PROTOCOL_VERSION,
        pid: 4242,
        started_at: "2026-10-04T11:00:00Z".parse().unwrap(),
        screen_backend: "vt100".to_owned(),
        conversations: 1,
        shells: 1,
        providers: Vec::new(),
        catalog: None,
        roots: None,
        config: None,
        sandbox: None,
        sandbox_paths: None,
        full_mode: None,
    }
}

/// `count` permission rules of the three effects; every third one is held to a dir.
fn rules(count: usize) -> String {
    let mut text = String::new();
    for n in 0..count {
        let effect = ["allow", "ask", "deny"][n % 3];
        let under = if n % 3 == 2 { format!(", under = \"/srv/p{n}\"") } else { String::new() };
        text.push_str(&format!(
            "[[permissions.rules]]\naction = \"execute\"\n\
             resource = {{ command = {{ program = \"tool{n}\", args = [\"sub{n}\", \"--flag\"]{under} }} }}\n\
             effect = \"{effect}\"\n\n"
        ));
    }
    text
}

/// Writes `config.toml` with `count` rules and, with `theme`, a palette file whose code
/// theme is the `.tmTheme` fixture of efr-render.
fn write_config(roots: &Roots, count: usize, theme: bool) {
    let mut text = String::new();
    if theme {
        let tm = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../efr-render/fixtures/sample_theme.tmTheme")
            .canonicalize()
            .unwrap();
        let palette = roots.path("config/theme.toml");
        std::fs::write(
            &palette,
            format!(
                "code_theme = \"{}\"\n\n[colors]\naccent = \"#f2c14e\"\nmuted = 8\n\
                 text = \"#e8e2d4\"\nlink = \"blue\"\n",
                tm.display()
            ),
        )
        .unwrap();
        text.push_str(&format!("[render]\npalette = \"{}\"\n\n", palette.display()));
    }
    text.push_str(&rules(count));
    std::fs::write(roots.path("config/config.toml"), text).unwrap();
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_cli_first_write() {
    const RUNS: usize = 20;
    let mut report = Report::new("bench_perf_cli_first_write");

    let roots = Roots::new();
    let version = bench::run(2, RUNS, || {
        let output = roots.command(env!("CARGO_BIN_EXE_efr")).arg("--version").output().unwrap();
        assert!(output.status.success(), "{output:?}");
    });
    report.time("efr --version to exit", &version);

    let mut first_points = Vec::new();
    for theme in [false, true] {
        for count in [0, 50, 200] {
            let roots = Roots::new();
            write_config(&roots, count, theme);
            let (firsts, server) = serve(&roots.socket(), RUNS + 2, |conn| {
                let (id, method) = conn.request();
                assert!(matches!(method, Method::AdminStatus(_)), "got {}", method.name());
                conn.reply(id, &status_result());
            });
            let mut first = Vec::new();
            let mut exit = Vec::new();
            for run in 0..RUNS + 2 {
                let spawned = Instant::now();
                let output =
                    roots.command(env!("CARGO_BIN_EXE_efr")).arg("status").output().unwrap();
                let ended = spawned.elapsed();
                // NOTE: no warning: the rules and the theme file loaded.
                assert!(
                    output.status.success() && output.stderr.is_empty(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let at = firsts.recv_timeout(Duration::from_secs(5)).unwrap();
                if run >= 2 {
                    first.push(at.duration_since(spawned));
                    exit.push(ended);
                }
            }
            server.join().unwrap();
            let label = format!("{count} rules, theme {}", if theme { "file" } else { "none" });
            let stats = report.time(&format!("{label}: spawn to first byte"), &first);
            report.time(&format!("{label}: efr status to exit"), &exit);
            if !theme {
                first_points.push((count.max(1) as u64, stats.p50));
            }
        }
    }
    report.slope("first byte over rules (1/50/200)", &first_points);
    report.finish();
}

/// A pseudo-terminal pair of 40 rows and 120 columns.
fn pty() -> (OwnedFd, OwnedFd) {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let name = ptsname(&master, Vec::new()).unwrap();
    let slave =
        rustix::fs::open(name.as_c_str(), OFlags::RDWR | OFlags::NOCTTY, Mode::empty()).unwrap();
    let size = rustix::termios::Winsize { ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 };
    rustix::termios::tcsetwinsize(&master, size).unwrap();
    (master, slave)
}

/// Reads the master until the slave side is gone, so efr never blocks on a full
/// terminal; gives what the terminal showed.
fn read_screen(master: OwnedFd) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut screen = Vec::new();
        let mut buffer = [0_u8; 65536];
        while let Ok(read) = rustix::io::read(&master, &mut buffer) {
            if read == 0 {
                break;
            }
            screen.extend_from_slice(&buffer[..read]);
        }
        screen
    })
}

/// `text` without its escape sequences and carriage returns.
fn bare(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() || c == '\\' || c == '\x07' {
                    break;
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

/// zsh, at a path of the system, never from `~/.local/bin`.
fn zsh() -> Option<&'static str> {
    ["/usr/bin/zsh", "/bin/zsh"].into_iter().find(|path| Path::new(path).exists())
}

/// What one run of efr on a terminal cost.
struct Ran {
    peak: u64,
    cpu: Duration,
    wall: Duration,
    screen: String,
}

/// Runs the built efr with `args` on a fresh terminal under zsh's `time`, and gives its
/// peak resident set, its CPU time, its wall time and the bare text of the screen;
/// `None` when it ran past [`STOP_AFTER`], and then its process group is killed.
fn on_terminal(roots: &Roots, zsh: &str, args: &[&str]) -> Option<Ran> {
    use std::os::unix::process::CommandExt as _;
    let (master, slave) = pty();
    let screen = read_screen(master.try_clone().unwrap());
    drop(master);
    let mut command = roots.command(zsh);
    command
        .args(["-f", "-c", "TIMEFMT='%M %uU %uS'; time \"$@\" 2>&1", "zsh"])
        .arg(env!("CARGO_BIN_EXE_efr"))
        .args(args)
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(Stdio::piped())
        .process_group(0);
    let started = Instant::now();
    let mut child = command.spawn().unwrap();
    drop(command);
    drop(slave);
    let mut stderr = child.stderr.take().unwrap();
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > STOP_AFTER {
            let group = rustix::process::Pid::from_child(&child);
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let wall = started.elapsed();
    let screen = String::from_utf8_lossy(&screen.join().unwrap()).into_owned();
    let stderr = stderr.join().unwrap();
    let status = status?;
    assert!(status.success(), "efr failed: {stderr}\n{}", bare(&screen));
    let fields: Vec<&str> = stderr.split_whitespace().collect();
    let [peak_kib, user, system] = fields[..] else {
        panic!("the line of zsh's time, got {stderr:?}");
    };
    let micros = |text: &str| -> u64 {
        text.trim_end_matches("us").parse().unwrap_or_else(|_| panic!("{stderr:?}"))
    };
    Some(Ran {
        peak: peak_kib.parse::<u64>().unwrap() * 1024,
        cpu: Duration::from_micros(micros(user) + micros(system)),
        wall,
        screen: bare(&screen),
    })
}

/// A unified diff of one file with `lines` added lines.
fn diff_text(lines: usize) -> String {
    let mut text = String::from(
        "diff --git a/src/big.rs b/src/big.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/big.rs\n",
    );
    text.push_str(&format!("@@ -0,0 +1,{lines} @@\n"));
    for n in 0..lines {
        text.push_str(&format!("+    let value_{n} = compute({n}, \"text of line {n}\");\n"));
    }
    text
}

/// The answer of the turn: a list item that holds a rust fence of `lines` lines.
fn fenced_answer(lines: usize) -> String {
    let mut text = String::from("Here is the plan:\n\n1. Add the function:\n\n   ```rust\n");
    for n in 0..lines {
        text.push_str(&format!("   fn line_{n}(x: u32) -> u32 {{ x + {n} }}\n"));
    }
    text.push_str("   ```\n2. Run the tests.\n");
    text
}

fn envelope(seq: u64, event: Event) -> ConversationSubscribeItem {
    ConversationSubscribeItem::Event(EventEnvelope {
        seq: Seq::new(seq),
        conversation_id: Some(CONVERSATION.parse().unwrap()),
        at: "2026-10-04T12:00:00Z".parse().unwrap(),
        event,
    })
}

/// The fake daemon's side of `efr send`: the prompt's receipt, then the answer in
/// 16-byte drafts, one each `gap`, then the completed message and the end of the turn.
/// efr cancels a subscription whose queue overflows and subscribes again after the
/// last event that it saw; such a subscription gets the events after that one, as
/// efrd replays them. It counts these in `resubscribes`.
fn stream_turn(conn: &mut Conn, answer: &str, gap: Duration, resubscribes: &AtomicU64) {
    let (id, method) = conn.request();
    assert!(matches!(method, Method::PromptSend(_)), "got {}", method.name());
    let turn_id = TURN.parse().unwrap();
    conn.reply(
        id,
        &PromptSendResult {
            conversation_id: CONVERSATION.parse().unwrap(),
            turn_id,
            seq: Seq::new(3),
            queued: false,
            settings: None,
            route: None,
        },
    );
    let (sub, method) = conn.request();
    assert!(matches!(method, Method::ConversationSubscribe(_)), "got {}", method.name());
    let bytes = answer.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        let mut end = (offset + 16).min(bytes.len());
        while !answer.is_char_boundary(end) {
            end += 1;
        }
        let draft = Draft {
            turn_id,
            after_seq: Seq::new(3),
            draft: DraftPart::Text {
                index: 0,
                offset: offset as u64,
                delta: answer[offset..end].to_owned(),
            },
        };
        conn.item(sub, &ConversationSubscribeItem::Draft(draft));
        offset = end;
        std::thread::sleep(gap);
    }
    let text = answer.to_owned();
    let completed = Event::TurnCompleted {
        turn_id,
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    };
    let events = [
        envelope(4, Event::AssistantMessageCompleted { turn_id, index: 0, text }),
        envelope(5, completed),
    ];
    for event in &events {
        conn.item(sub, event);
    }
    while let Some(frame) = conn.recv() {
        let ClientFrame::Request { id, method: Method::ConversationSubscribe(params) } = frame
        else {
            continue;
        };
        resubscribes.fetch_add(1, Ordering::Relaxed);
        let after = params.after_seq.map_or(0, Seq::get);
        for event in &events {
            if matches!(event, ConversationSubscribeItem::Event(envelope) if envelope.seq.get() > after)
            {
                conn.item(id, event);
            }
        }
    }
}

/// The runs of each size of a ladder of efr on a terminal.
const TERMINAL_RUNS: usize = 3;

/// Runs efr with `args` on a terminal [`TERMINAL_RUNS`] times for each of `sizes`,
/// against a fake daemon with the script that `case` makes for the size, and checks
/// that the screen holds the text that `case` gives with it. It adds the peak, the CPU
/// time and the wall time of each size and the slope of the CPU time to `report`. The
/// ladder stops at the first run that is killed.
fn terminal_ladder<S>(
    report: &mut Report,
    zsh: &str,
    name: &str,
    sizes: &[usize],
    args: &[&str],
    case: impl Fn(usize) -> (S, String),
) where
    S: Fn(&mut Conn) + Send + 'static,
{
    let mut points = Vec::new();
    for &size in sizes {
        let roots = Roots::new();
        let (script, last) = case(size);
        let (_, server) = serve(&roots.socket(), TERMINAL_RUNS, script);
        let (mut peak, mut cpu, mut wall) = (0, Vec::new(), Vec::new());
        for _ in 0..TERMINAL_RUNS {
            let Some(ran) = on_terminal(&roots, zsh, args) else { break };
            assert!(ran.screen.contains(&last), "the screen shows {last:?}");
            peak = peak.max(ran.peak);
            cpu.push(ran.cpu);
            wall.push(ran.wall);
        }
        if cpu.len() < TERMINAL_RUNS {
            report.count(&format!("{name}: stopped_at_n"), size as u64);
            break;
        }
        server.join().unwrap();
        report.bytes(&format!("{name} {size} lines: peak (VmHWM)"), peak);
        let stats = report.time(&format!("{name} {size} lines: cpu"), &cpu);
        report.time(&format!("{name} {size} lines: wall"), &wall);
        points.push((size as u64, stats.p50));
    }
    report.slope(&format!("{name} cpu over lines"), &points);
}

#[expect(clippy::print_stderr, reason = "a bench says what it skipped")]
#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_cli_memory() {
    let mut report = Report::new("bench_perf_cli_memory");
    let Some(zsh) = zsh() else {
        eprintln!("bench_perf_cli_memory: skipped: no zsh at /usr/bin or /bin for the peak");
        report.count("skipped_no_zsh", 1);
        report.finish();
        return;
    };

    let diff_args = ["diff", "--turn", TURN];
    terminal_ladder(&mut report, zsh, "diff", &[2_000, 10_000, 20_000], &diff_args, |lines| {
        let diff = diff_text(lines);
        let last = format!("value_{}", lines - 1);
        let script = move |conn: &mut Conn| {
            let (id, method) = conn.request();
            assert!(matches!(method, Method::ConversationDiff(_)), "got {}", method.name());
            let file = FileChange {
                path: "src/big.rs".to_owned(),
                kind: ChangeKind::Added,
                from: None,
                added: u32::try_from(lines).unwrap(),
                removed: 0,
                binary: false,
            };
            let result = ConversationDiffResult {
                turn_id: TURN.parse().unwrap(),
                changes: FileChanges::from_files(vec![file]),
                diff: Some(diff.clone()),
            };
            conn.reply(id, &result);
        };
        (script, last)
    });

    let fences = [30, 150, 300, 600, 1_000];
    for lines in fences {
        let drafts = fenced_answer(lines).len().div_ceil(16);
        report.count(&format!("fence {lines} lines: drafts"), drafts as u64);
    }
    let context = r#"{"pwd":"/srv","tty":"/dev/pts/9"}"#;
    let send_args = ["send", "--context-json", context, "--", "show the plan"];
    let counts = std::cell::RefCell::new(Vec::new());
    terminal_ladder(&mut report, zsh, "fence", &fences, &send_args, |lines| {
        let answer = fenced_answer(lines);
        let resubscribes = Arc::new(AtomicU64::new(0));
        counts.borrow_mut().push((lines, Arc::clone(&resubscribes)));
        let script = move |conn: &mut Conn| {
            stream_turn(conn, &answer, Duration::from_millis(1), &resubscribes);
        };
        (script, format!("fn line_{}(x", lines - 1))
    });
    let counts: Vec<(_, u64)> = counts
        .into_inner()
        .into_iter()
        .map(|(lines, count)| (lines, count.load(Ordering::Relaxed)))
        .collect();
    for (lines, count) in &counts {
        let label = format!("fence {lines} lines: subscribed again after an overflow");
        report.count(&label, *count);
    }
    report.finish();
    // The budget of the live render of efr-render: efr keeps up with a 1 ms draft
    // stream at every fence size, so its queue never overflows.
    for (lines, count) in &counts {
        assert_eq!(*count, 0, "fence {lines} lines: subscribed again after an overflow");
    }
}

/// The events in a snapshot: as many as efrd puts in one.
const SNAPSHOT_EVENTS: u64 = 128;

/// The turn's message `n`, one line of its own in a pipe.
fn gap_message(n: u64) -> EventEnvelope {
    EventEnvelope {
        seq: Seq::new(n),
        conversation_id: Some(CONVERSATION.parse().unwrap()),
        at: "2026-10-04T12:00:00Z".parse().unwrap(),
        event: Event::AssistantMessageCompleted {
            turn_id: TURN.parse().unwrap(),
            index: u32::try_from(n).unwrap(),
            text: format!("message {n}."),
        },
    }
}

/// The fake daemon's side of `efr send` after a gap of `gap` events: the prompt's
/// receipt, message 4, an overflow, and then a snapshot of the newest
/// [`SNAPSHOT_EVENTS`] messages with a cursor before them and the end of the turn. It
/// answers each `conversation.history` from the log of messages 4 to the last one, as
/// efrd pages it, and counts the pages in `pages`.
fn gap_turn(conn: &mut Conn, gap: u64, pages: &AtomicU64) {
    let (id, method) = conn.request();
    assert!(matches!(method, Method::PromptSend(_)), "got {}", method.name());
    let turn_id = TURN.parse().unwrap();
    conn.reply(
        id,
        &PromptSendResult {
            conversation_id: CONVERSATION.parse().unwrap(),
            turn_id,
            seq: Seq::new(3),
            queued: false,
            settings: None,
            route: None,
        },
    );
    let last = 4 + gap + SNAPSHOT_EVENTS;
    let log: Vec<EventEnvelope> = (4..=last).map(gap_message).collect();
    let (first, method) = conn.request();
    assert!(matches!(method, Method::ConversationSubscribe(_)), "got {}", method.name());
    conn.item(first, &ConversationSubscribeItem::Event(log[0].clone()));
    conn.send(&ServerFrame::error(Some(first), ErrorBody::overflow(Seq::new(4))));
    let (second, method) = conn.request();
    let Method::ConversationSubscribe(params) = method else {
        panic!("expected a subscribe, got {}", method.name());
    };
    assert_eq!(params.after_seq, Some(Seq::new(4)), "it subscribes after the last event shown");
    let oldest = last - SNAPSHOT_EVENTS + 1;
    let at: jiff::Timestamp = "2026-10-04T12:00:00Z".parse().unwrap();
    let snapshot = ConversationSnapshot {
        conversation: ConversationSummary {
            id: CONVERSATION.parse().unwrap(),
            title: None,
            status: ConversationStatus::Running,
            created_at: at,
            updated_at: at,
            last_seq: Seq::new(last),
            cwd: None,
            scope: None,
            tty: None,
        },
        events: log[usize::try_from(oldest - 4).unwrap()..].to_vec(),
        history_cursor: Some(PageCursor::new(format!("s:{oldest}"))),
        hwm: Seq::new(last),
        live: None,
    };
    conn.item(second, &ConversationSubscribeItem::Snapshot(snapshot));
    let completed = Event::TurnCompleted {
        turn_id,
        usage: None,
        changes: None,
        context: None,
        stop: None,
        undo: None,
    };
    conn.item(second, &envelope(last + 1, completed));
    while let Some(frame) = conn.recv() {
        let ClientFrame::Request { id, method: Method::ConversationHistory(params) } = frame else {
            continue;
        };
        pages.fetch_add(1, Ordering::Relaxed);
        let before = params
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.as_str().strip_prefix("s:"))
            .map_or(u64::MAX, |seq| seq.parse().unwrap());
        let limit = usize::try_from(params.limit.unwrap_or(100)).unwrap();
        let older: Vec<&EventEnvelope> =
            log.iter().filter(|envelope| envelope.seq.get() < before).collect();
        let start = older.len().saturating_sub(limit);
        let events: Vec<EventEnvelope> = older[start..].iter().map(|&e| e.clone()).collect();
        let next_cursor = (start > 0).then(|| PageCursor::new(format!("s:{}", events[0].seq)));
        conn.reply(id, &ConversationHistoryResult { events, next_cursor });
    }
}

/// The numbers of the messages in efr's output, in their order.
fn messages_shown(out: &str) -> Vec<u64> {
    out.lines()
        .filter_map(|line| line.trim().strip_prefix("message ")?.strip_suffix('.')?.parse().ok())
        .collect()
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_follow_gap() {
    const RUNS: usize = 5;
    let mut report = Report::new("bench_perf_follow_gap");
    let context = r#"{"pwd":"/srv","tty":"/dev/pts/9"}"#;
    let mut points = Vec::new();
    let mut checks = Vec::new();
    for gap in [300_u64, 1_000, 3_000] {
        let roots = Roots::new();
        let pages = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&pages);
        let (_, server) =
            serve(&roots.socket(), RUNS + 1, move |conn| gap_turn(conn, gap, &counted));
        let expected: Vec<u64> = (4..=4 + gap + SNAPSHOT_EVENTS).collect();
        let (mut walls, mut once, mut shown) = (Vec::new(), 0_u64, 0);
        for run in 0..=RUNS {
            let started = Instant::now();
            let output = roots
                .command(env!("CARGO_BIN_EXE_efr"))
                .args(["send", "--context-json", context, "--", "go on"])
                .output()
                .unwrap();
            let wall = started.elapsed();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let messages = messages_shown(&String::from_utf8_lossy(&output.stdout));
            shown = messages.len();
            once += u64::from(messages == expected);
            // The first run warms the caches.
            if run > 0 {
                walls.push(wall);
            }
            if wall > STOP_AFTER {
                break;
            }
        }
        if walls.len() < RUNS {
            report.count(&format!("gap {gap}: stopped_at_n"), gap);
            break;
        }
        server.join().unwrap();
        let runs = RUNS as u64 + 1;
        let per_run = pages.load(Ordering::Relaxed) / runs;
        report.count(&format!("gap {gap}: events shown"), shown as u64);
        report.count(&format!("gap {gap}: runs that showed each event once in order"), once);
        report.count(&format!("gap {gap}: history pages per run"), per_run);
        let stats = report.time(&format!("gap {gap}: efr send to exit"), &walls);
        points.push((gap, stats.p50));
        checks.push((gap, once == runs, per_run));
    }
    let slope = report.slope("efr send to exit over the gap (300/1k/3k)", &points);
    report.finish();
    // The budget: every event of the gap shows once and in order, efr reads the gap in
    // pages of 128 back to the last event that it showed, and no further, and the time
    // grows at most linearly with the gap.
    assert!(slope < 1.3, "the time over the gap has the slope {slope:.2}");
    for (gap, once, pages) in checks {
        assert!(once, "gap {gap}: an event did not show, or showed twice or out of order");
        let needed = (gap + 1).div_ceil(SNAPSHOT_EVENTS);
        assert_eq!(pages, needed, "gap {gap}: pages of conversation.history");
    }
}

/// The files that the turns of the history of the `--all` bench change.
const ALL_FILES: usize = 50;

/// The budget of the growth of the peak of `--all` from 2k to 20k events: about
/// 0.5 MiB when efr keeps only the turn ends.
const ALL_PEAK_GROWTH: u64 = 4 * 1024 * 1024;

/// A history of `events` events for `efr diff --stat --all`: turns of 10 events, 9
/// answers of about 400 bytes and an end that changed one of [`ALL_FILES`] files.
fn all_history(events: usize) -> Vec<EventEnvelope> {
    (1..=events)
        .map(|seq| {
            let turn_id =
                format!("019a9b1c-3d00-7a10-8b20-{:012x}", 0x10_0000 + seq / 10).parse().unwrap();
            let event = if seq % 10 == 0 {
                let file = FileChange {
                    path: format!("src/f{}.rs", (seq / 10) % ALL_FILES),
                    kind: ChangeKind::Modified,
                    from: None,
                    added: 1,
                    removed: 0,
                    binary: false,
                };
                Event::TurnCompleted {
                    turn_id,
                    usage: None,
                    changes: Some(FileChanges::from_files(vec![file])),
                    context: None,
                    stop: None,
                    undo: None,
                }
            } else {
                let text = format!("answer {seq}: the build passed and the tests ran. ").repeat(8);
                Event::AssistantMessageCompleted { turn_id, index: 0, text }
            };
            let ConversationSubscribeItem::Event(envelope) = envelope(seq as u64, event) else {
                unreachable!("an event")
            };
            envelope
        })
        .collect()
}

/// The fake daemon's side of `efr diff --stat --all`: each page of `history` as efrd
/// sends it (the newest page first, each page oldest first, at most 500 events), and
/// the count of the pages in `pages`.
fn serve_history(conn: &mut Conn, history: &[EventEnvelope], pages: &AtomicU64) {
    while let Some(frame) = conn.recv() {
        let ClientFrame::Request { id, method } = frame else { continue };
        let Method::ConversationHistory(params) = method else {
            panic!("got {}", method.name());
        };
        pages.fetch_add(1, Ordering::Relaxed);
        let limit = params.limit.unwrap_or(100).min(500) as usize;
        let end = params.cursor.map_or(history.len(), |cursor| cursor.as_str().parse().unwrap());
        let start = end.saturating_sub(limit);
        let next_cursor = (start > 0).then(|| PageCursor::new(start.to_string()));
        let events = history[start..end].to_vec();
        conn.reply(id, &ConversationHistoryResult { events, next_cursor });
    }
}

#[expect(clippy::print_stderr, reason = "a bench says what it skipped")]
#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_cli_diff_all() {
    let mut report = Report::new("bench_perf_cli_diff_all");
    let Some(zsh) = zsh() else {
        eprintln!("bench_perf_cli_diff_all: skipped: no zsh at /usr/bin or /bin for the peak");
        report.count("skipped_no_zsh", 1);
        report.finish();
        return;
    };
    let args = ["diff", "--stat", "--all", "--conversation", CONVERSATION];
    let (mut points, mut peaks, mut over) = (Vec::new(), Vec::new(), Vec::new());
    for events in [2_000, 10_000, 20_000] {
        let label = format!("{events} events");
        let roots = Roots::new();
        let history = Arc::new(all_history(events));
        let pages = Arc::new(AtomicU64::new(0));
        let (served, counted) = (Arc::clone(&history), Arc::clone(&pages));
        let (_, server) = serve(&roots.socket(), TERMINAL_RUNS, move |conn| {
            serve_history(conn, &served, &counted);
        });
        let last = format!("{ALL_FILES} files changed, +{}", events / 10);
        let (mut peak, mut cpu, mut wall) = (0, Vec::new(), Vec::new());
        for _ in 0..TERMINAL_RUNS {
            let Some(ran) = on_terminal(&roots, zsh, &args) else { break };
            assert!(ran.screen.contains(&last), "{label}: the screen shows {last:?}");
            peak = peak.max(ran.peak);
            cpu.push(ran.cpu);
            wall.push(ran.wall);
        }
        if cpu.len() < TERMINAL_RUNS {
            report.count("stopped_at_n", events as u64);
            break;
        }
        server.join().unwrap();
        let per_run = pages.load(Ordering::Relaxed) / TERMINAL_RUNS as u64;
        report.bytes(&format!("{label}: peak (VmHWM)"), peak);
        let stats = report.time(&format!("{label}: cpu"), &cpu);
        report.time(&format!("{label}: wall"), &wall);
        report.count(&format!("{label}: history pages of each run"), per_run);
        points.push((events as u64, stats.p50));
        peaks.push(peak);
        if per_run > (events as u64).div_ceil(500) {
            over.push(format!("{label}: {per_run} pages"));
        }
    }
    report.slope("cpu over events", &points);
    report.finish();
    // The budget (plan W2-I): efr asks for pages of 500 events, the most that efrd
    // sends; it asked for pages of 100 before. And it keeps only the ends of the turns
    // that changed files: the peak of 20k events was 16 MiB above the peak of 2k when
    // it kept every event.
    assert_eq!(points.len(), 3, "the ladder stopped early: {points:?}");
    assert!(over.is_empty(), "more pages than events / 500: {over:?}");
    let growth = peaks[2].saturating_sub(peaks[0]);
    assert!(growth < ALL_PEAK_GROWTH, "the peak grew {growth} bytes from 2k to 20k events");
}
