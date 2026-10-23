//! The bench of the start of the built `efrd` (`bench_perf_daemon_start`): from the
//! spawn of the process to the `READY=1` datagram that it sends to `NOTIFY_SOCKET`,
//! and to the socket file. `just bench-perf` runs it in release.
//!
//! Each case gets one tree in cargo's target temp dir: `EFR_HOME`, a home, an
//! `XDG_RUNTIME_DIR` of its own and the notify socket. The first start on the tree
//! makes the database (or migrates nothing on a seeded one), the launcher copy and the
//! runtime root; then five restarts follow on the same tree. The environment is
//! cleared but for these, `PATH=/usr/bin:/bin` and `EFR_LOG=info`, so the process
//! never sees the real home, config or runtime directory, and never reaches the user's
//! daemon. The config points both providers at a closed port of `127.0.0.1`. The bench
//! checks that `daemon.json` in the temp runtime root names the temp socket and the
//! child's pid. The cases: the sandbox off and on (the real bwrap probe), `dbus.proxy`
//! off and on, and a store that is empty or seeded with search corpus S (1,000 turns)
//! or L (20,000 turns). The seeded stores are the corpus files of `search_index.rs`,
//! written through `efr-store` before the start.
//!
//! The binary runs from a private copy in the target temp dir, with the `efr-sbx` that
//! cargo built next to `efrd` (or `EFR_TEST_SBX_BIN`), so a build in the same target
//! dir cannot replace it while it runs. Without an `efr-sbx` or without bwrap the
//! cases with the sandbox on are skipped with a line that says why.
//!
//! NOTE: cargo builds this `efrd` with the test seams (the dev edge to
//! efr-test-daemon turns on `test-sandbox-fake`), so it has no system or session bus
//! of the machine. The bus filter's self-test does not use them, so its cost is the
//! same as in an installed build.
//!
//! The phases of a restart come from the times at which efrd's info lines arrive on
//! its stderr: `starting` (after the config, tracing, the lock and the daemon id),
//! `reconciled` (after the store opened and reconciled), `provider ready` (the shells,
//! the HTTP client and the providers), `sandbox launcher copied` (the engine, the
//! trusted programs and the copy), the line of the probe's result (after the probe and
//! the bus filter's self-test), and the rest up to `READY=1` (the engine again, the
//! jobs, git, the socket and the tasks).

use std::io::{BufRead as _, BufReader};
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use efr_test_support::CorpusSize;
use efr_test_support::bench::Report;
use tempfile::TempDir;

use crate::search_index::corpus_file;

/// The measured starts of each case, after one start that is not measured.
const RUNS: usize = 5;

/// The longest wait for `READY=1`: the probe alone may take 30 s.
const READY_WAIT: Duration = Duration::from_secs(45);

/// A case whose start takes longer than this stops after that start.
const STOP_AFTER: Duration = Duration::from_secs(10);

/// The bench's name in its report lines.
const BENCH: &str = "bench_perf_daemon_start";

/// The lines of efrd's stderr that end a phase, in the order of the start.
const PHASES: [(&str, &str); 5] = [
    ("starting", "efr_daemon::run: starting"),
    ("reconciled", "efr_daemon::run: reconciled"),
    ("providers", "provider ready"),
    ("launcher_copied", "sandbox launcher copied"),
    ("probe_and_bus_test", "the auto sandbox is"),
];

/// The base of every temp tree of the bench.
fn base() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("perf-start");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A private copy of `efrd` and, when one is found, of `efr-sbx` next to it.
struct Binaries {
    _dir: TempDir,
    efrd: PathBuf,
    sbx: bool,
}

impl Binaries {
    fn new() -> Binaries {
        let dir = tempfile::tempdir_in(base()).unwrap();
        let built = PathBuf::from(env!("CARGO_BIN_EXE_efrd"));
        let efrd = dir.path().join("efrd");
        std::fs::copy(&built, &efrd).unwrap();
        let source = std::env::var_os("EFR_TEST_SBX_BIN")
            .map(PathBuf::from)
            .or_else(|| built.parent().map(|dir| dir.join("efr-sbx")))
            .filter(|path| path.is_file());
        let sbx = match source {
            Some(source) => {
                std::fs::copy(source, dir.path().join("efr-sbx")).unwrap();
                true
            }
            None => false,
        };
        Binaries { _dir: dir, efrd, sbx }
    }
}

/// One fresh tree of a start.
struct Tree {
    dir: TempDir,
}

impl Tree {
    fn new(config: &str, store: Option<&Path>) -> Tree {
        let dir = tempfile::tempdir_in(base()).unwrap();
        for name in ["home", "xdg", "efr", "efr/config", "efr/data", "efr/state", "efr/runtime"] {
            std::fs::DirBuilder::new().mode(0o700).create(dir.path().join(name)).unwrap();
        }
        std::fs::write(dir.path().join("efr/config/config.toml"), config).unwrap();
        if let Some(store) = store {
            std::fs::copy(store, dir.path().join("efr/data/efr.sqlite")).unwrap();
        }
        Tree { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

/// A running efrd that gets SIGTERM, then SIGKILL, when it drops.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.0.id()).unwrap());
        let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// What one start took, from the spawn.
#[derive(Debug, Default)]
struct Start {
    socket: Option<Duration>,
    ready: Duration,
    phases: Vec<(&'static str, Duration)>,
    sandbox_available: bool,
    bus_filter_on: bool,
}

/// Starts efrd on `tree` and waits for `READY=1`.
fn start(binaries: &Binaries, tree: &Tree) -> Start {
    let notify_path = tree.path("notify.sock");
    let notify = UnixDatagram::bind(&notify_path).unwrap();
    notify.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    let runtime = tree.path("efr/runtime");
    let socket = runtime.join("daemon.sock");
    let mut command = efr_stdx::process::command(&binaries.efrd, tree.path("home")).into_std();
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", tree.path("home"))
        .env("XDG_RUNTIME_DIR", tree.path("xdg"))
        .env("EFR_HOME", tree.path("efr"))
        .env("EFR_LOG", "info")
        .env("NOTIFY_SOCKET", &notify_path)
        .current_dir(tree.path("home"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let spawned = Instant::now();
    let mut child = command.spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let running = Running(child);
    let lines: Arc<Mutex<Vec<(Duration, String)>>> = Arc::default();
    let reader = {
        let lines = Arc::clone(&lines);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                lines.lock().unwrap().push((spawned.elapsed(), plain(&line)));
            }
        })
    };
    let (seen_tx, seen_rx) = mpsc::channel();
    let watcher = {
        let socket = socket.clone();
        std::thread::spawn(move || {
            while spawned.elapsed() < READY_WAIT {
                if socket.exists() {
                    let _ = seen_tx.send(spawned.elapsed());
                    return;
                }
                std::thread::sleep(Duration::from_micros(200));
            }
        })
    };
    let mut buf = [0_u8; 4096];
    let ready = loop {
        match notify.recv(&mut buf) {
            Ok(read) => {
                let text = String::from_utf8_lossy(&buf[..read]);
                if text.lines().any(|line| line == "READY=1") {
                    break spawned.elapsed();
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("the notify socket failed: {error}"),
        }
        if spawned.elapsed() > READY_WAIT {
            drop(running);
            let logged: Vec<String> =
                lines.lock().unwrap().iter().map(|(_, line)| line.clone()).collect();
            panic!("no READY=1 within {READY_WAIT:?}; stderr:\n{}", logged.join("\n"));
        }
    };
    let socket_at = seen_rx.recv_timeout(Duration::from_secs(1)).ok();
    let _ = watcher.join();
    check_daemon_json(&runtime, running.0.id());
    if binaries.sbx {
        assert!(runtime.join("bin/efr-sbx").is_file(), "the launcher copy is in the temp runtime");
    }
    drop(running);
    let _ = reader.join();
    let lines = lines.lock().unwrap();
    let at = |needle: &str| lines.iter().find(|(_, line)| line.contains(needle)).map(|(at, _)| *at);
    let phases =
        PHASES.iter().filter_map(|(name, needle)| at(needle).map(|time| (*name, time))).collect();
    Start {
        socket: socket_at,
        ready,
        phases,
        sandbox_available: at("the auto sandbox is available").is_some(),
        bus_filter_on: at("the bus filter passed its self-test").is_some(),
    }
}

/// `line` without its colour escapes: efrd colours its stderr also on a pipe.
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `daemon.json` names the temp socket and the child, never a real root.
fn check_daemon_json(runtime: &Path, pid: u32) {
    let text = std::fs::read_to_string(runtime.join("daemon.json")).unwrap();
    let info: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(info["pid"], pid, "{text}");
    let socket = info["socket"].as_str().unwrap();
    assert_eq!(Path::new(socket), runtime.join("daemon.sock"), "{text}");
    assert!(socket.starts_with(base().to_str().unwrap()), "{text}");
}

/// The config of a case: the providers at a closed port, the sandbox and the bus
/// filter on or off.
fn config(sandbox: bool, bus: bool) -> String {
    format!(
        "[openai]\napi_base_url = \"http://127.0.0.1:9/v1\"\n\n\
         [anthropic]\nbase_url = \"http://127.0.0.1:9\"\n\n\
         [sandbox]\nenabled = {sandbox}\n\n[dbus]\nproxy = {bus}\n"
    )
}

/// One case of the ladder.
struct Case {
    sandbox: bool,
    bus: bool,
    store: Option<CorpusSize>,
}

impl Case {
    fn label(&self) -> String {
        let store = self.store.map_or("empty", CorpusSize::name);
        format!(
            "sandbox {} bus {} store {store}",
            if self.sandbox { "on" } else { "off" },
            if self.bus { "on" } else { "off" }
        )
    }
}

/// Runs every start of `case` on one tree and adds its lines to `report`: the first
/// start (the database, the launcher copy and the runtime root are new), then
/// [`RUNS`] restarts on the same tree, as after `systemctl --user restart efrd`.
fn measure(report: &mut Report, binaries: &Binaries, case: &Case, store: Option<&Path>) {
    let label = case.label();
    let tree = Tree::new(&config(case.sandbox, case.bus), store);
    let mut ready = Vec::new();
    let mut socket = Vec::new();
    let mut phases: Vec<(&str, Vec<Duration>)> = Vec::new();
    let mut available = 0;
    let mut bus_on = 0;
    for run in 0..=RUNS {
        for gone in ["notify.sock", "efr/runtime/daemon.sock", "efr/runtime/daemon.json"] {
            let _ = std::fs::remove_file(tree.path(gone));
        }
        let start = start(binaries, &tree);
        if run == 0 {
            report.time(&format!("{label}: first start to READY=1"), &[start.ready]);
            continue;
        }
        ready.push(start.ready);
        socket.extend(start.socket);
        available += u64::from(start.sandbox_available);
        bus_on += u64::from(start.bus_filter_on);
        let mut before = Duration::ZERO;
        for (name, at) in start.phases {
            let span = at.saturating_sub(before);
            before = at;
            match phases.iter_mut().find(|(seen, _)| *seen == name) {
                Some((_, samples)) => samples.push(span),
                None => phases.push((name, vec![span])),
            }
        }
        let tail = start.ready.saturating_sub(before);
        match phases.iter_mut().find(|(seen, _)| *seen == "to_ready") {
            Some((_, samples)) => samples.push(tail),
            None => phases.push(("to_ready", vec![tail])),
        }
        if start.ready > STOP_AFTER {
            report.count(&format!("{label}: stopped_at_run"), run as u64);
            break;
        }
    }
    report.time(&format!("{label}: restart to READY=1"), &ready);
    report.time(&format!("{label}: restart to socket file"), &socket);
    for (name, samples) in &phases {
        report.time(&format!("{label}: restart phase {name}"), samples);
    }
    report.count(&format!("{label}: sandbox available"), available);
    report.count(&format!("{label}: bus filter on"), bus_on);
}

#[expect(clippy::print_stderr, reason = "a bench says which cases it skipped")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "bench: run by just bench-perf"]
async fn bench_perf_daemon_start() {
    let binaries = Binaries::new();
    let bwrap = ["/usr/bin/bwrap", "/bin/bwrap"].iter().any(|path| Path::new(path).is_file());
    let mut report = Report::new(BENCH);
    let mut stores = Vec::new();
    for size in [CorpusSize::S, CorpusSize::L] {
        let built = Instant::now();
        let file = corpus_file(size).await;
        report.time(&format!("corpus {} ready (built or cached)", size.name()), &[built.elapsed()]);
        report
            .bytes(&format!("store {} file", size.name()), std::fs::metadata(&file).unwrap().len());
        stores.push((size, file));
    }
    let cases = [
        Case { sandbox: false, bus: false, store: None },
        Case { sandbox: false, bus: true, store: None },
        Case { sandbox: true, bus: false, store: None },
        Case { sandbox: true, bus: true, store: None },
        Case { sandbox: false, bus: false, store: Some(CorpusSize::S) },
        Case { sandbox: false, bus: false, store: Some(CorpusSize::L) },
        Case { sandbox: true, bus: true, store: Some(CorpusSize::S) },
        Case { sandbox: true, bus: true, store: Some(CorpusSize::L) },
    ];
    for case in &cases {
        if case.sandbox && !(bwrap && binaries.sbx) {
            let why = if bwrap { "no efr-sbx next to efrd" } else { "no bwrap" };
            eprintln!("{BENCH}: {}: skipped: {why}", case.label());
            report.count(&format!("{}: skipped", case.label()), 1);
            continue;
        }
        let store = case
            .store
            .and_then(|size| stores.iter().find(|(seen, _)| *seen == size))
            .map(|(_, file)| file.as_path());
        tokio::task::block_in_place(|| measure(&mut report, &binaries, case, store));
    }
    report.finish();
}
