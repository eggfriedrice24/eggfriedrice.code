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
    AdminStatusResult, ChangeKind, ClientFrame, ConversationDiffResult, ConversationSubscribeItem,
    Draft, DraftPart, Event, EventEnvelope, FileChange, FileChanges, Method, PROTOCOL_VERSION,
    PromptSendResult, RequestId, Seq, ServerFrame,
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
    for (lines, count) in counts.into_inner() {
        let label = format!("fence {lines} lines: subscribed again after an overflow");
        report.count(&label, count.load(Ordering::Relaxed));
    }
    report.finish();
}
