//! The cost of `efr search --picker`, which fzf runs on each key of the picker of
//! `,resume`: the built `efr` against a fake daemon that answers 50 rows, from the
//! start of the process to its exit, without its wait of 50 ms (the time minus 50 ms),
//! and its peak memory (the largest resident set, as `VmHWM`). It also reports the time
//! of hello: from the accept of the connection to the search request.
//!
//! The test always prints its table. With `EFR_TEST_SEARCH_GATE=1` (which
//! `just bench-search` sets, in a release build) it fails when a budget fails: p95 at
//! most 15 ms and a peak of at most 16 MiB. The peak comes from zsh's `time` (`%M`, the
//! largest resident set of the child that the kernel reports); without zsh, the test
//! reports no peak, and the gate fails. The numbers of a busy machine are not valid.
//!
//! A second test measures `,resume <words>` up to fzf: the `PATH` walk, `fzf
//! --version`, hello and the first search, with a fake `fzf` that reads the first rows
//! and closes at once. It counts from the start of `efr resume` to its exit, so the
//! two starts of the fake `fzf` (a `sh` each) count too: an upper bound. The gate is
//! p95 at most 30 ms with a fake daemon that answers at once.
//!
//! `efr` gets a cleared environment with every efr root in a temporary directory and a
//! `PATH` without `~/.local/bin`, so it never reaches the real daemon.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    Capabilities, ClientFrame, ConversationStatus, ConversationSummary, ConversationsSearchResult,
    DaemonPaths, HelloResult, Method, PROTOCOL_VERSION, PageCursor, RecentConversation, RequestId,
    RouteScope, SearchHit, SearchKind, SearchLine, SearchRow, Seq, ServerFrame, TextRange,
};
use serde::Serialize;

/// The terminal that the picker names.
const TTY: &str = "/dev/pts/9";

/// The wait of `--picker` before it connects, which the budget leaves out.
const WAIT: Duration = Duration::from_millis(50);

/// The budget of the time from the start to the exit without the wait, at p95.
const TIME_BUDGET: Duration = Duration::from_millis(15);

/// The budget of `,resume` up to the first rows in fzf, at p95.
const RESUME_BUDGET: Duration = Duration::from_millis(30);

/// A fake `fzf` that costs as little as it can: `--version` prints 0.73.1; otherwise it
/// reads the first rows with no other process and closes as Esc does.
const QUICK_FZF: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo '0.73.1 (fake)'
  exit 0
fi
while read -r _; do :; done
printf '\nesc\n\n'
"#;

/// The budget of the peak memory, in KiB.
const PEAK_BUDGET_KIB: u64 = 16 * 1024;

/// One reload's output at most (50 rows and the header).
const OUTPUT_BUDGET: usize = 32 * 1024;

/// Writes one line of the report.
fn say(text: &str) {
    let _ = writeln!(std::io::stdout(), "search_cost: {text}");
}

fn gate() -> bool {
    std::env::var_os("EFR_TEST_SEARCH_GATE").is_some_and(|value| value == "1")
}

/// Temporary efr roots, with the commented example config, so the run reads a config
/// file as a real one does.
struct Roots {
    dir: tempfile::TempDir,
}

impl Roots {
    fn new() -> Roots {
        let dir = tempfile::tempdir().unwrap();
        for name in ["home", "config", "data", "state", "runtime"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        let example =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../efr-config/examples/config.toml");
        std::fs::copy(example, dir.path().join("config/config.toml")).unwrap();
        Roots { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `program` with a cleared environment: the roots, fzf's query, a terminal with
    /// 24-bit colours, and a `PATH` that holds no efr.
    fn command(&self, program: &str) -> Command {
        let mut command = efr_stdx::process::command(program, self.dir.path()).into_std();
        command
            .env_clear()
            .env("HOME", self.path("home"))
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("FZF_QUERY", "rsync lap")
            .env("EFR_CONFIG_DIR", self.path("config"))
            .env("EFR_DATA_DIR", self.path("data"))
            .env("EFR_STATE_DIR", self.path("state"))
            .env("EFR_RUNTIME_DIR", self.path("runtime"))
            .stdin(Stdio::null());
        command
    }
}

fn hello_result() -> HelloResult {
    HelloResult {
        daemon_id: "019a9b1c-3d00-7a10-8b20-000000000007".parse().unwrap(),
        protocol: PROTOCOL_VERSION,
        version: "0.1.0".to_owned(),
        capabilities: Capabilities { search: Some(true), ..Capabilities::default() },
        paths: DaemonPaths::default(),
        challenge: "AAAA".to_owned(),
    }
}

/// 50 rows as a large store answers them: titles of 60 characters, snippets of 200
/// with three matches, a holder on some.
fn fifty_rows() -> ConversationsSearchResult {
    let at: jiff::Timestamp = "2026-10-20T12:00:00Z".parse().unwrap();
    let title = "rsync the photos of the laptop to the backup disk, then check";
    let snippet = "then run rsync -a --delete ~/photos/ laptop:/srv/photos/ and check that \
        the laptop keeps its own copy; the disk of the laptop has 41% free and the backup \
        of last week took nine minutes over the slow link";
    let ranges = |pairs: &[(u32, u32)]| -> Vec<TextRange> {
        pairs.iter().map(|&(start, end)| TextRange { start, end }).collect()
    };
    let rows = (1..=50_u64)
        .map(|n| SearchRow {
            conversation: RecentConversation {
                summary: ConversationSummary {
                    id: format!("019a9b1c-3d00-7a10-8b20-{n:012x}").parse().unwrap(),
                    title: Some(title.to_owned()),
                    status: ConversationStatus::Idle,
                    created_at: at,
                    updated_at: at,
                    last_seq: Seq::new(n),
                    cwd: None,
                    scope: None,
                    tty: None,
                },
                route: RouteScope::Outside,
                project: None,
                turns: 12,
                open_in: (n % 7 == 0).then(|| "/dev/pts/4".to_owned()),
                here: n == 1,
            },
            short_id: format!("019a9b1c-3d00-7a10-8b20-{n:012x}"),
            title_ranges: ranges(&[(0, 5), (24, 27)]),
            hit: Some(SearchHit {
                kind: SearchKind::Prompt,
                seq: Seq::new(n * 10),
                turn_id: None,
                at,
                history_cursor: PageCursor::new(format!("c{n}")),
                lines: vec![SearchLine {
                    text: snippet.to_owned(),
                    ranges: ranges(&[(9, 14), (40, 43), (78, 81)]),
                }],
            }),
        })
        .collect();
    ConversationsSearchResult { rows, ..Default::default() }
}

/// One accepted connection of the fake daemon.
struct Conn {
    stream: UnixStream,
    decoder: Decoder,
    ready: VecDeque<Vec<u8>>,
}

impl Conn {
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

    fn reply<T: Serialize>(&mut self, id: RequestId, value: &T) {
        self.stream
            .write_all(&framing::encode(&ServerFrame::item(id, value).unwrap()).unwrap())
            .unwrap();
        self.stream.write_all(&framing::encode(&ServerFrame::end(id)).unwrap()).unwrap();
    }
}

/// A fake daemon on its own thread that answers `runs` connections, each with its hello
/// and 50 rows, and sends the time from each accept to the search request.
fn serve(socket: &Path, runs: usize) -> mpsc::Receiver<Duration> {
    let listener = UnixListener::bind(socket).unwrap();
    let rows = fifty_rows();
    let (times, received) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..runs {
            let (stream, _) = listener.accept().unwrap();
            let accepted = Instant::now();
            let mut conn = Conn { stream, decoder: Decoder::new(), ready: VecDeque::new() };
            let (id, method) = conn.request();
            assert!(matches!(method, Method::Hello(_)), "hello first, got {}", method.name());
            conn.reply(id, &hello_result());
            let (id, method) = conn.request();
            let hello = accepted.elapsed();
            assert!(
                matches!(&method, Method::ConversationsSearch(params) if params.query == "rsync lap" && params.limit == Some(50)),
                "conversations.search with fzf's query, got {}",
                method.name()
            );
            conn.reply(id, &rows);
            while conn.recv().is_some() {}
            let _ = times.send(hello);
        }
    });
    received
}

/// Checks what one run printed: the header and 50 rows within the output budget, and
/// nothing on stderr but the line of zsh's `time`, when zsh ran it.
fn check_output(output: &Output, time_line: bool) {
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(stdout.lines().count(), 51, "{stdout}");
    assert!(stdout.len() <= OUTPUT_BUDGET, "{} bytes", stdout.len());
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert_eq!(stderr.lines().count(), usize::from(time_line), "stderr: {stderr}");
}

fn percentile(sorted: &[Duration], share: f64) -> Duration {
    let last = sorted.len().saturating_sub(1);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "an index of a short list"
    )]
    let at = (last as f64 * share).round() as usize;
    sorted.get(at.min(last)).copied().unwrap_or_default()
}

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

/// The series as `p50, p95, max` in milliseconds.
fn line(label: &str, times: &mut [Duration]) -> (String, Duration) {
    times.sort();
    let (p50, p95) = (percentile(times, 0.5), percentile(times, 0.95));
    let max = times.last().copied().unwrap_or_default();
    let text = format!(
        "{label:<36} p50 {:>7.2} ms  p95 {:>7.2} ms  max {:>7.2} ms  ({} runs)",
        ms(p50),
        ms(p95),
        ms(max),
        times.len()
    );
    (text, p95)
}

/// zsh, at a path of the system, never from `~/.local/bin`.
fn zsh() -> Option<&'static str> {
    ["/usr/bin/zsh", "/bin/zsh"].into_iter().find(|path| Path::new(path).exists())
}

/// The largest resident set of `efr search --picker` in KiB, from zsh's `time`.
fn peak_kib(roots: &Roots, zsh: &str) -> u64 {
    let output = roots
        .command(zsh)
        .args(["-f", "-c", "TIMEFMT=%M; time \"$@\"", "zsh", env!("CARGO_BIN_EXE_efr")])
        .args(["search", "--picker", "--tty", TTY])
        .output()
        .unwrap();
    check_output(&output, true);
    let stderr = String::from_utf8(output.stderr).unwrap();
    stderr.trim().parse().unwrap_or_else(|_| panic!("a size in KiB, got {stderr:?}"))
}

#[test]
fn bench_search_picker_rows() {
    let gate = gate();
    let runs = if gate { 60 } else { 12 };
    let peaks = 3;
    let roots = Roots::new();
    let has_zsh = zsh();
    let served = runs + peaks * usize::from(has_zsh.is_some());
    let hellos = serve(&roots.path("runtime").join("daemon.sock"), served);
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    say(&format!("load average {}", load.split_whitespace().take(3).collect::<Vec<_>>().join(" ")));

    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        let output = roots
            .command(env!("CARGO_BIN_EXE_efr"))
            .args(["search", "--picker", "--tty", TTY])
            .output()
            .unwrap();
        times.push(started.elapsed().saturating_sub(WAIT));
        check_output(&output, false);
    }
    let mut hello: Vec<Duration> = hellos.iter().take(runs).collect();
    let peak = has_zsh.map(|zsh| (0..peaks).map(|_| peak_kib(&roots, zsh)).max().unwrap_or(0));

    let (run_line, p95) = line("start to exit, without the 50 ms wait", &mut times);
    let (hello_line, _) = line("hello (accept to the request)", &mut hello);
    let mut report = String::new();
    let _ = writeln!(report, "{run_line}; budget p95 {} ms", TIME_BUDGET.as_millis());
    let _ = writeln!(report, "{hello_line}");
    match peak {
        Some(kib) => {
            let _ = writeln!(report, "peak memory (VmHWM) {kib} KiB; budget {PEAK_BUDGET_KIB} KiB");
        }
        None => report.push_str("peak memory not measured: no zsh at /usr/bin or /bin\n"),
    }
    for line in report.lines() {
        say(line);
    }
    if gate {
        assert!(p95 <= TIME_BUDGET, "p95 {:.2} ms is over the budget", ms(p95));
        let kib = peak.expect("the gate needs zsh for the peak memory");
        assert!(kib <= PEAK_BUDGET_KIB, "the peak of {kib} KiB is over the budget");
    }
}

#[test]
fn bench_search_resume_first_rows() {
    use std::os::unix::fs::PermissionsExt as _;
    let gate = gate();
    let runs = if gate { 60 } else { 12 };
    let roots = Roots::new();
    let bin = roots.path("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("fzf"), QUICK_FZF).unwrap();
    std::fs::set_permissions(bin.join("fzf"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let hellos = serve(&roots.path("runtime").join("daemon.sock"), runs);
    let context = serde_json::json!({ "pwd": roots.path("home"), "tty": TTY }).to_string();
    let (master, slave) = super::picker::pty();
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    say(&format!("load average {}", load.split_whitespace().take(3).collect::<Vec<_>>().join(" ")));

    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let mut command = roots.command(env!("CARGO_BIN_EXE_efr"));
        command
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("EFR_CONTEXT", &context)
            .env("EFR_RESUME_WORDS", "rsync lap")
            .arg("resume")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let output = command.output().unwrap();
        times.push(started.elapsed());
        assert!(output.status.success(), "{output:?}");
        assert_eq!(super::picker::screen_of(&master), "", "Esc prints nothing");
    }
    let mut hello: Vec<Duration> = hellos.iter().take(runs).collect();

    let (run_line, p95) = line(",resume <words> to fzf and back", &mut times);
    let (hello_line, _) = line("hello (accept to the request)", &mut hello);
    say(&format!("{run_line}; budget p95 {} ms", RESUME_BUDGET.as_millis()));
    say(&hello_line);
    if gate {
        assert!(p95 <= RESUME_BUDGET, "p95 {:.2} ms is over the budget", ms(p95));
    }
}
