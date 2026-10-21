//! The forwarder of a job with port forwards on the real launcher: the frames it sends
//! for the job's listens, the connections it makes for efrd's `Open`, its own sandbox,
//! and what the job's child can and cannot reach. A test listener stands for efrd at
//! the job's `$CALL/forward.sock` (`TestForward`), with short timings. No test reaches a
//! real host: every server of a job listens on port 0 of the job's own loopback, and
//! the one fixed port, 20099, is a port of the job's namespace where nothing listens.

use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use efr_sandbox::{
    FRAME_LEN, ForwardPlan, Frame, FrameKind, JobPlan, JobStop, Listen, NetworkPlan, SPEC_FILE,
    SandboxSpec,
};
use rustix::process::Signal;

use crate::support::{
    Fixture, JobRun, ProxyAnswer, Run, alive, q, sandbox_or_skip, wait_until, write,
};

/// How long a test waits for a frame.
const WAIT: Duration = Duration::from_secs(10);

/// The short timings of the tests (the plan's section 2.0).
const POLL_MS: u32 = 20;
const STABLE_MS: u32 = 40;
const EPHEMERAL_STABLE_MS: u32 = 200;

/// The wait for a port of the ephemeral range in the test of a short-lived server: 70
/// times the 30 ms life of that server, so a stall of the machine under load does not
/// make the server count.
const SHORT_LIFE_STABLE_MS: u32 = 2000;

/// A port of the job's own loopback where nothing listens.
const CLOSED_PORT: u16 = 20099;

/// A listener that stands for efrd at a job's `$CALL/forward.sock`, 0600, as efrd
/// binds it. The first frame of each connection says what it is: the forwarder's
/// `Hello` opens the control connection, whose frames the test reads and on which it
/// sends `Open`; a `Data` opens the connection of one `Open`.
pub(crate) struct TestForward {
    socket: PathBuf,
    frames: Receiver<Frame>,
    data: Receiver<(Frame, UnixStream)>,
    control: Arc<Mutex<Option<UnixStream>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl TestForward {
    /// Binds `$CALL/forward.sock` for `call_dir` through the dir's descriptor, so a deep
    /// call dir still fits a socket address, and serves it on a thread of its own.
    pub(crate) fn serve(call_dir: &Path) -> TestForward {
        let socket = call_dir.join(efr_sandbox::FORWARD_SOCKET_FILE);
        let listener = bind_short(call_dir);
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (frames_to, frames) = channel();
        let (data_to, data) = channel();
        let control = Arc::new(Mutex::new(None));
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let serving = (control.clone(), connections.clone(), stop.clone());
        std::thread::spawn(move || {
            let (control, connections, stop) = serving;
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                connections.fetch_add(1, Ordering::SeqCst);
                let (frames_to, data_to, control) =
                    (frames_to.clone(), data_to.clone(), control.clone());
                std::thread::spawn(move || accept_one(stream, &frames_to, &data_to, &control));
            }
        });
        TestForward { socket, frames, data, control, connections, stop }
    }

    /// A socket at `$CALL/forward.sock` where nothing listens: the forwarder's connect
    /// fails after its setup.
    fn dead(call_dir: &Path) {
        drop(bind_short(call_dir));
        let socket = call_dir.join(efr_sandbox::FORWARD_SOCKET_FILE);
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// The control frames until one that `wanted` takes, at most [`WAIT`]; every frame
    /// on the way is kept in `seen`.
    fn wait_for(&self, seen: &mut Vec<Frame>, wanted: impl Fn(&Frame) -> bool) -> Frame {
        let start = Instant::now();
        loop {
            let left = WAIT.saturating_sub(start.elapsed());
            let frame = self
                .frames
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("waited {WAIT:?} for a frame; got {seen:?}"));
            seen.push(frame);
            if wanted(&frame) {
                return frame;
            }
        }
    }

    /// The frames that came so far, without a wait.
    fn drain(&self, seen: &mut Vec<Frame>) {
        while let Ok(frame) = self.frames.try_recv() {
            seen.push(frame);
        }
    }

    /// Sends `Open` of connection `id` to `listen` on the control connection.
    fn open(&self, id: u32, listen: Listen) {
        let control = self.control.lock().unwrap();
        let mut control = control.as_ref().expect("the control connection is open");
        control.write_all(&Frame::open(id, listen).encode()).unwrap();
    }

    /// The `Data` frame of the next data connection and the connection, at most
    /// [`WAIT`].
    fn next_data(&self) -> (Frame, UnixStream) {
        self.data.recv_timeout(WAIT).expect("a data connection")
    }

    /// The connections that the socket accepted so far.
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for TestForward {
    fn drop(&mut self) {
        // The flag, then one connection that wakes the accept.
        self.stop.store(true, Ordering::SeqCst);
        if let Some(dir) = self.socket.parent().and_then(|parent| fs::File::open(parent).ok()) {
            let short = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()))
                .join(efr_sandbox::FORWARD_SOCKET_FILE);
            let _ = UnixStream::connect(short);
        }
    }
}

/// Binds `$CALL/forward.sock` through the call dir's descriptor.
fn bind_short(call_dir: &Path) -> UnixListener {
    let flags = rustix::fs::OFlags::PATH | rustix::fs::OFlags::DIRECTORY;
    let dir = rustix::fs::open(call_dir, flags, rustix::fs::Mode::empty()).unwrap();
    let short = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()))
        .join(efr_sandbox::FORWARD_SOCKET_FILE);
    UnixListener::bind(&short).unwrap()
}

/// Reads the first frame of one connection and hands the connection on.
fn accept_one(
    mut stream: UnixStream,
    frames: &Sender<Frame>,
    data: &Sender<(Frame, UnixStream)>,
    control: &Mutex<Option<UnixStream>>,
) {
    let Some(first) = read_frame(&mut stream) else { return };
    match first.kind {
        FrameKind::Hello => {
            *control.lock().unwrap() = stream.try_clone().ok();
            let _ = frames.send(first);
            while let Some(frame) = read_frame(&mut stream) {
                if frames.send(frame).is_err() {
                    break;
                }
            }
        }
        FrameKind::Data => {
            let _ = data.send((first, stream));
        }
        _ => {}
    }
}

/// One frame, as efrd decodes it; `None` at the end or for bytes that are not a frame.
fn read_frame(stream: &mut UnixStream) -> Option<Frame> {
    let mut bytes = [0_u8; FRAME_LEN];
    stream.read_exact(&mut bytes).ok()?;
    Frame::decode(&bytes).ok()
}

/// The launcher inside the sandbox, which also runs the self-test, quoted for a line.
fn launcher(fixture: &Fixture) -> String {
    q(&fixture.spec.runtime.inside_launcher())
}

/// The job's forward socket inside, quoted for a line.
fn inside_socket(fixture: &Fixture) -> String {
    q(&fixture.spec.runtime.inside_forward_socket())
}

/// A job's call dir for `line` with a forward plan of the short timings; the spec names
/// the job's `$CALL/forward.sock`. With `serve`, a [`TestForward`] serves it; else a
/// socket where nothing listens.
pub(crate) fn prepare(
    fixture: &Fixture,
    line: &str,
    serve: bool,
) -> (PathBuf, Option<TestForward>) {
    prepare_with(fixture, line, serve, EPHEMERAL_STABLE_MS)
}

/// [`prepare`] with `ephemeral_stable_ms` for the ports of the ephemeral range.
fn prepare_with(
    fixture: &Fixture,
    line: &str,
    serve: bool,
    ephemeral_stable_ms: u32,
) -> (PathBuf, Option<TestForward>) {
    let call_dir = fixture.prepare_job(line, JobPlan::new(600, 19));
    let forward = if serve {
        Some(TestForward::serve(&call_dir))
    } else {
        TestForward::dead(&call_dir);
        None
    };
    let path = call_dir.join(SPEC_FILE);
    let mut spec = SandboxSpec::from_json(&fs::read(&path).unwrap()).unwrap();
    let proxy = matches!(spec.network, NetworkPlan::Proxy { .. });
    let plan = ForwardPlan {
        socket: spec.runtime.forward_socket(),
        poll_ms: POLL_MS,
        stable_ms: STABLE_MS,
        ephemeral_stable_ms,
        max_ports: 8,
        proxy,
    };
    spec.job.as_mut().expect("a job's spec").forward = Some(plan);
    fs::write(&path, spec.to_json().unwrap()).unwrap();
    (call_dir, forward)
}

/// Starts the job of `call_dir`.
fn start(fixture: &Fixture, call_dir: &Path) -> JobRun {
    let mut run = fixture.spawn_job(call_dir);
    run.start();
    run
}

/// A shell loop that waits until `path` exists: the job's own wait for the test.
fn wait_for_file(path: &Path) -> String {
    format!("while [[ ! -e {} ]]; do sleep 0.02; done", q(path))
}

/// The addresses that the echo servers of a job printed (`echo <address>`).
fn echoes(run: &Run) -> Vec<SocketAddr> {
    run.stdout
        .lines()
        .filter_map(|line| line.strip_prefix("echo "))
        .filter_map(|address| address.parse().ok())
        .collect()
}

/// The one line of `check` that a self-test or a check printed: whether it passed, and
/// the line for the message.
fn check_line(run: &Run, check: &str) -> (bool, String) {
    let line = run
        .stdout
        .lines()
        .find(|line| line.contains(&format!("\"name\":\"{check}\"")))
        .unwrap_or_else(|| panic!("no {check} line: {run:#?}"))
        .to_owned();
    (line.contains("\"ok\":true"), line)
}

/// Fails unless `check` printed its line and passed.
fn passed(run: &Run, check: &str) {
    let (ok, line) = check_line(run, check);
    assert!(ok, "{line}\n{run:#?}");
}

/// True while a forwarder of `fixture` runs: its command line names the fixture's own
/// inside socket, which no job of another test has.
fn forwarder_alive(fixture: &Fixture) -> bool {
    alive(&format!("forward --socket {}", fixture.spec.runtime.inside_forward_socket().display()))
}

/// Opens connection `id` to `listen` and fails unless the forwarder connected.
fn connected(forward: &TestForward, id: u32, listen: Listen) -> UnixStream {
    forward.open(id, listen);
    let (data, stream) = forward.next_data();
    assert_eq!((data.kind, data.id, data.status), (FrameKind::Data, id, 0), "{data:?}");
    stream
}

/// A listen of the job's echo server on `addr`, then its `Gone` once the server ended
/// after one connection.
fn listen_then_gone(addr: IpAddr, bind: &str) {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let done = fixture.project.join("done");
    let line = format!(
        "{} self-test --echo '{bind}' --echo-once; {}",
        launcher(&fixture),
        wait_for_file(&done)
    );
    let (call_dir, forward) = prepare(&fixture, &line, true);
    let forward = forward.unwrap();
    let job = start(&fixture, &call_dir);
    let mut seen = Vec::new();
    let hello = forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    assert_eq!(hello.status, efr_sandbox::FORWARD_VERSION);
    let listen = forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Listen);
    assert_eq!(listen.addr, addr, "{seen:?}");
    assert!(listen.port >= 1024, "{seen:?}");
    // One connection ends the server: its port is gone.
    let mut stream = connected(&forward, 1, listen.target());
    stream.write_all(b"x").unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut back = Vec::new();
    stream.read_to_end(&mut back).unwrap();
    assert_eq!(back, b"x");
    let gone = forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Gone);
    assert_eq!(gone.target(), listen.target(), "{seen:?}");
    write(&done, "done");
    let run = job.wait(&fixture);
    run.expect_status(0);
    assert_eq!(echoes(&run), [SocketAddr::new(addr, listen.port)], "{run:#?}");
}

#[test]
fn listen_on_v4_is_reported() {
    listen_then_gone(IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1:0");
}

#[test]
fn listen_on_v6_is_reported() {
    listen_then_gone(IpAddr::V6(Ipv6Addr::LOCALHOST), "[::1]:0");
}

#[test]
fn open_connects_and_copies() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = format!("{} self-test --echo 127.0.0.1:0", launcher(&fixture));
    let (call_dir, forward) = prepare(&fixture, &line, true);
    let forward = forward.unwrap();
    let mut job = start(&fixture, &call_dir);
    let mut seen = Vec::new();
    let listen = forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Listen);
    let stream = connected(&forward, 7, listen.target());
    // 64 KiB there while the echo comes back, then the half-close: the end of the
    // bytes reaches the server, and the server's end comes back.
    let sent: Vec<u8> = (0..64 * 1024).map(|n| u8::try_from(n % 251).unwrap()).collect();
    let mut writer = stream.try_clone().unwrap();
    let bytes = sent.clone();
    let writing = std::thread::spawn(move || {
        writer.write_all(&bytes).unwrap();
        writer.shutdown(Shutdown::Write).unwrap();
    });
    let mut back = Vec::new();
    let mut reader = stream;
    reader.read_to_end(&mut back).unwrap();
    writing.join().unwrap();
    assert_eq!(back.len(), sent.len());
    assert!(back == sent, "the echo differs from the bytes sent");
    // A second connection to the same server works too.
    let mut again = connected(&forward, 8, listen.target());
    again.write_all(b"again").unwrap();
    again.shutdown(Shutdown::Write).unwrap();
    let mut back = Vec::new();
    again.read_to_end(&mut back).unwrap();
    assert_eq!(back, b"again");
    job.close_lifeline();
    let run = job.wait(&fixture);
    assert_eq!(run.result().stop, Some(JobStop::Lifeline), "{run:#?}");
}

#[test]
fn open_to_a_closed_port_is_refused() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let done = fixture.project.join("done");
    let (call_dir, forward) = prepare(&fixture, &wait_for_file(&done), true);
    let forward = forward.unwrap();
    let job = start(&fixture, &call_dir);
    let mut seen = Vec::new();
    forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    let closed = Listen { addr: IpAddr::V4(Ipv4Addr::LOCALHOST), port: CLOSED_PORT };
    forward.open(3, closed);
    let (data, mut stream) = forward.next_data();
    assert_eq!((data.kind, data.id), (FrameKind::Data, 3), "{data:?}");
    // ECONNREFUSED: nothing listens there in the job's namespace.
    assert_eq!(data.status, 111, "{data:?}");
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "{rest:?}");
    write(&done, "done");
    job.wait(&fixture).expect_status(0);
}

#[test]
fn bridge_port_is_never_reported_or_opened() {
    let ready = sandbox_or_skip!();
    let mut fixture = Fixture::new(&ready);
    fixture.proxy(ProxyAnswer::Tunnel);
    let line = format!("{} self-test --echo 127.0.0.1:0", launcher(&fixture));
    let (call_dir, forward) = prepare(&fixture, &line, true);
    let forward = forward.unwrap();
    let mut job = start(&fixture, &call_dir);
    let mut seen = Vec::new();
    // The echo server's port counts only after the ephemeral delay; the bridge's 3128
    // listened from before the forwarder's first poll, so it would have come first.
    let listen = forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Listen);
    assert_ne!(listen.port, efr_sandbox::BRIDGE_PORT, "{seen:?}");
    // efrd never asks for 3128; if it did, the forwarder refuses with EPERM.
    let bridge = Listen { addr: IpAddr::V4(Ipv4Addr::LOCALHOST), port: efr_sandbox::BRIDGE_PORT };
    forward.open(9, bridge);
    let (data, _stream) = forward.next_data();
    assert_eq!((data.kind, data.id, data.status), (FrameKind::Data, 9, 1), "{data:?}");
    forward.drain(&mut seen);
    assert!(seen.iter().all(|frame| frame.port != efr_sandbox::BRIDGE_PORT), "{seen:?}");
    job.close_lifeline();
    job.wait(&fixture);
}

/// Runs `line` as a job with a forwarder to its end.
fn run_forward_job(fixture: &Fixture, line: &str) -> (Run, TestForward) {
    let (call_dir, forward) = prepare(fixture, line, true);
    let run = start(fixture, &call_dir).wait(fixture);
    (run, forward.unwrap())
}

#[test]
fn child_cannot_reach_the_forward_socket() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = format!(
        "{} self-test --check unix_socket --socket {}",
        launcher(&fixture),
        inside_socket(&fixture)
    );
    let (run, forward) = run_forward_job(&fixture, &line);
    passed(&run, "unix_socket");
    // The socket got the forwarder's control connection and nothing else.
    let mut seen = Vec::new();
    forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    assert_eq!(forward.connections(), 1);
    assert!(forward.data.try_recv().is_err());
}

#[test]
fn child_cannot_signal_the_forwarder() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = format!("{} self-test --forward --check forward_signal", launcher(&fixture));
    let (run, _forward) = run_forward_job(&fixture, &line);
    passed(&run, "forward_signal");
}

#[test]
fn forwarder_proc_files_are_roots() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = format!("{} self-test --forward --check forward_environ", launcher(&fixture));
    let (run, _forward) = run_forward_job(&fixture, &line);
    passed(&run, "forward_environ");
}

#[test]
fn forwarder_cannot_bind_or_listen() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // The forwarder's setup and domain from inside the job: the job itself writes the
    // project; the forwarder's domain does not, and it cannot bind, listen or run a
    // program, while it still reads its tables and connects inside.
    let line = format!(
        "print x > \"$PWD/job-write\"; {} forward --check --socket {} --write \"$PWD\"",
        launcher(&fixture),
        inside_socket(&fixture)
    );
    let (run, _forward) = run_forward_job(&fixture, &line);
    run.expect_status(0);
    for check in ["dumpable", "write", "tables", "reopen", "bind", "listen", "exec", "connect"] {
        passed(&run, check);
    }
    assert!(fixture.project.join("job-write").exists());
    assert!(!fixture.project.join(".efr-forward-write").exists());
}

#[test]
fn failed_forwarder_setup_does_not_fail_the_job() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // A stop in each step of the server: the starter reports it with exit 1, and no
    // server of that start keeps running.
    let line = format!(
        "for step in close session name dumpable landlock seccomp; do \
         {} forward --socket {} --fail-setup $step </dev/null; print -r -- \"starter $step $?\"; \
         for f in /proc/[0-9]*/cmdline(N); do c=\"$(<$f)\"; \
         [[ $c == *fail-setup* && $c == *{}* ]] && print found; done; done",
        launcher(&fixture),
        inside_socket(&fixture),
        fixture.spec.runtime.inside_forward_socket().display()
    );
    let (run, _forward) = run_forward_job(&fixture, &line);
    for step in ["close", "session", "name", "dumpable", "landlock", "seccomp"] {
        assert!(run.stdout.contains(&format!("starter {step} 1")), "{step}: {run:#?}");
    }
    assert!(!run.stdout.contains("found"), "{run:#?}");
    // The real forwarder of a job whose socket does not answer: it gives up after its
    // setup, sends no Hello, and the job runs and ends as it would with no forwards.
    let (call_dir, _) = prepare(&fixture, "print ran", false);
    let run = start(&fixture, &call_dir).wait(&fixture);
    run.expect_status(0);
    assert_eq!(run.stdout, "ran\n", "{run:#?}");
    assert!(run.result().setup_error.is_none(), "{run:#?}");
    assert_eq!(run.result().exit_code, Some(0), "{run:#?}");
}

#[test]
fn stop_spares_the_forwarder_and_does_not_name_it() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let started = fixture.project.join("started");
    let line = format!("print ok > {}; sleep 9877.41", q(&started));
    let (call_dir, forward) = prepare(&fixture, &line, true);
    let forward = forward.unwrap();
    let job = start(&fixture, &call_dir);
    let mut seen = Vec::new();
    forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    wait_until("the job's start", || started.exists());
    wait_until("the job's sleep", || alive("sleep 9877.41"));
    assert!(forwarder_alive(&fixture), "the job has no forwarder");
    job.signal(Signal::TERM);
    let run = job.wait(&fixture);
    assert_eq!(run.result().stop, Some(JobStop::Signal), "{run:#?}");
    let stopped = &run.result().summary.background_stopped;
    // The stop names the job's own processes and never the forwarder, which ended with
    // the namespace.
    assert!(stopped.contains(&"sleep".to_owned()), "{run:#?}");
    assert!(!stopped.contains(&"efr-forward".to_owned()), "{run:#?}");
    assert!(run.result().summary.survivors.is_empty(), "{run:#?}");
    assert!(!alive("sleep 9877.41"));
    assert!(!forwarder_alive(&fixture), "a forwarder outlived its job");
}

#[test]
fn short_lived_port_zero_server_is_not_reported() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // A server that lives 30 ms, then one that stays: only the second counts.
    // NOTE: the forwarder's debounce runs on real time, so no marker can bound the
    // first server's life; the long wait of the ephemeral range leaves a wide margin.
    let line = format!(
        "{l} self-test --echo 127.0.0.1:0 --echo-ms 30; {l} self-test --echo 127.0.0.1:0",
        l = launcher(&fixture)
    );
    let (call_dir, forward) = prepare_with(&fixture, &line, true, SHORT_LIFE_STABLE_MS);
    let forward = forward.unwrap();
    let mut job = start(&fixture, &call_dir);
    let mut seen = Vec::new();
    let listen = forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Listen);
    job.close_lifeline();
    let run = job.wait(&fixture);
    forward.drain(&mut seen);
    let servers = echoes(&run);
    assert_eq!(servers.len(), 2, "{run:#?}");
    assert_eq!(listen.port, servers[1].port(), "{seen:?} {servers:?}");
    let listens: Vec<u16> =
        seen.iter().filter(|frame| frame.kind == FrameKind::Listen).map(|f| f.port).collect();
    assert_eq!(listens, [servers[1].port()], "{seen:?}");
    assert!(seen.iter().all(|frame| frame.kind != FrameKind::Gone), "{seen:?}");
}

#[test]
fn udp_is_not_reported() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // The UDP socket is bound before the TCP listener, so a report of it would come
    // first.
    let line =
        format!("{} self-test --echo 127.0.0.1:0 --echo-udp 127.0.0.1:0", launcher(&fixture));
    let (call_dir, forward) = prepare(&fixture, &line, true);
    let forward = forward.unwrap();
    let mut job = start(&fixture, &call_dir);
    let mut seen = Vec::new();
    let listen = forward.wait_for(&mut seen, |frame| frame.kind == FrameKind::Listen);
    job.close_lifeline();
    let run = job.wait(&fixture);
    forward.drain(&mut seen);
    let udp: Vec<SocketAddr> = run
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("udp "))
        .filter_map(|address| address.parse().ok())
        .collect();
    assert_eq!(udp.len(), 1, "{run:#?}");
    assert_eq!(echoes(&run), [SocketAddr::new(listen.addr, listen.port)], "{run:#?}");
    let reported: Vec<u16> = seen.iter().filter(|f| f.kind.has_listen()).map(|f| f.port).collect();
    assert_eq!(reported, [listen.port], "{seen:?}");
}
