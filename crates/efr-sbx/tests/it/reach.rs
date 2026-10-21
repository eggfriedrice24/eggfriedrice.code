//! The reach helper of a contained call on the real launcher: the ports it holds before
//! the child, the frames it sends, the connections it makes for the call's clients, its
//! own sandbox, and what the call's child can and cannot reach. A test listener stands
//! for efrd at the call's `$CALL/reach.sock` (`TestReach`). No test reaches a real host:
//! every port of a test is a port of the call's own network namespace, and the call's
//! clients are `self-test --dial` and its servers `self-test --echo`.

use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use efr_sandbox::{
    FRAME_LEN, Frame, FrameKind, Listen, NetworkPlan, ReachPlan, SPEC_FILE, SandboxSpec,
};

use crate::support::{
    Fixture, Run, alive, q, run_with_timeout, sandbox_or_skip, say, wait_until, write,
};

/// How long a test waits for a frame.
const WAIT: Duration = Duration::from_secs(10);

/// A forwarded port of the start of a call.
const PORT: u16 = 20101;

/// A port that comes during the call.
const LATER: u16 = 20102;

/// The errno of a bind of a port that another socket holds.
const EADDRINUSE: u8 = 98;

const V4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const V6: IpAddr = IpAddr::V6(Ipv6Addr::LOCALHOST);

/// What the test listener does with a data connection of the helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// Sends every byte back, then ends its side: a job's echo server.
    Echo,
    /// Closes the connection with no byte: no job's server for the port.
    Close,
}

/// A listener that stands for efrd at a call's `$CALL/reach.sock`, 0600, as efrd binds
/// it. The first frame of each connection says what it is: the helper's `Hello` opens
/// the control connection, whose frames the test reads and on which it sends `Reach`
/// and `Unreach`; a `Connect` opens a data connection, which the listener answers.
pub(crate) struct TestReach {
    socket: PathBuf,
    frames: Receiver<Frame>,
    connects: Receiver<Frame>,
    ended: Receiver<()>,
    control: Arc<Mutex<Option<UnixStream>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl TestReach {
    /// Binds `$CALL/reach.sock` for `call_dir` through the dir's descriptor and serves
    /// it on a thread of its own; each data connection gets `answer`.
    pub(crate) fn serve(call_dir: &Path, answer: Answer) -> TestReach {
        let socket = call_dir.join(efr_sandbox::REACH_SOCKET_FILE);
        let listener = bind_short(call_dir);
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (frames_to, frames) = channel();
        let (connects_to, connects) = channel();
        let (ended_to, ended) = channel();
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
                let sends = (frames_to.clone(), connects_to.clone(), ended_to.clone());
                let control = control.clone();
                std::thread::spawn(move || accept_one(stream, &sends, &control, answer));
            }
        });
        TestReach { socket, frames, connects, ended, control, connections, stop }
    }

    /// A socket at `$CALL/reach.sock` where nothing listens: the helper's connect fails
    /// after its setup.
    fn dead(call_dir: &Path) {
        drop(bind_short(call_dir));
        let socket = call_dir.join(efr_sandbox::REACH_SOCKET_FILE);
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

    /// The `Bound` of `listen`, at most [`WAIT`].
    fn bound(&self, seen: &mut Vec<Frame>, listen: Listen) -> Frame {
        self.wait_for(seen, |frame| frame.kind == FrameKind::Bound && frame.target() == listen)
    }

    /// Sends `bytes` on the control connection.
    fn send_bytes(&self, bytes: &[u8; FRAME_LEN]) {
        let control = self.control.lock().unwrap();
        let mut control = control.as_ref().expect("the control connection is open");
        control.write_all(bytes).unwrap();
    }

    /// Sends `frame` on the control connection.
    fn send(&self, frame: Frame) {
        self.send_bytes(&frame.encode());
    }

    /// The `Connect` frame of the next data connection, at most [`WAIT`].
    fn next_connect(&self) -> Frame {
        self.connects.recv_timeout(WAIT).expect("a data connection")
    }

    /// Waits at most [`WAIT`] for the end of the control connection.
    fn wait_for_end(&self) {
        self.ended.recv_timeout(WAIT).expect("the end of the control connection");
    }

    /// The connections that the socket accepted so far.
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for TestReach {
    fn drop(&mut self) {
        // The flag, then one connection that wakes the accept.
        self.stop.store(true, Ordering::SeqCst);
        if let Some(dir) = self.socket.parent().and_then(|parent| fs::File::open(parent).ok()) {
            let short = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()))
                .join(efr_sandbox::REACH_SOCKET_FILE);
            let _ = UnixStream::connect(short);
        }
    }
}

/// Binds `$CALL/reach.sock` through the call dir's descriptor.
fn bind_short(call_dir: &Path) -> UnixListener {
    let flags = rustix::fs::OFlags::PATH | rustix::fs::OFlags::DIRECTORY;
    let dir = rustix::fs::open(call_dir, flags, rustix::fs::Mode::empty()).unwrap();
    let short = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()))
        .join(efr_sandbox::REACH_SOCKET_FILE);
    UnixListener::bind(&short).unwrap()
}

/// Reads the first frame of one connection and serves the connection.
fn accept_one(
    mut stream: UnixStream,
    sends: &(Sender<Frame>, Sender<Frame>, Sender<()>),
    control: &Mutex<Option<UnixStream>>,
    answer: Answer,
) {
    let (frames, connects, ended) = sends;
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
            let _ = ended.send(());
        }
        FrameKind::Connect => {
            let _ = connects.send(first);
            match answer {
                Answer::Echo => {
                    let Ok(mut back) = stream.try_clone() else { return };
                    let _ = std::io::copy(&mut stream, &mut back);
                    let _ = back.shutdown(Shutdown::Write);
                }
                Answer::Close => drop(stream),
            }
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

/// The call's reach socket inside, quoted for a line.
fn inside_socket(fixture: &Fixture) -> String {
    q(&fixture.spec.runtime.inside_reach_socket())
}

/// A call dir for `line` with a reach plan of `ports`; the spec names the call's
/// `$CALL/reach.sock`. With `answer`, a [`TestReach`] serves it; else a socket where
/// nothing listens.
pub(crate) fn prepare(
    fixture: &Fixture,
    line: &str,
    ports: &[u16],
    answer: Option<Answer>,
) -> (PathBuf, Option<TestReach>) {
    let call_dir = fixture.prepare(line);
    let reach = match answer {
        Some(answer) => Some(TestReach::serve(&call_dir, answer)),
        None => {
            TestReach::dead(&call_dir);
            None
        }
    };
    let path = call_dir.join(SPEC_FILE);
    let mut spec = SandboxSpec::from_json(&fs::read(&path).unwrap()).unwrap();
    let proxy = matches!(spec.network, NetworkPlan::Proxy { .. });
    spec.reach = Some(ReachPlan::new(spec.runtime.reach_socket(), ports.to_vec(), proxy));
    fs::write(&path, spec.to_json().unwrap()).unwrap();
    (call_dir, reach)
}

/// A call that runs on a thread of the test.
struct Running {
    call_dir: PathBuf,
    ended: JoinHandle<(i32, String, String)>,
}

impl Running {
    /// Starts the call of `call_dir`.
    fn start(fixture: &Fixture, call_dir: &Path) -> Running {
        let command = fixture.command(call_dir, &fixture.project);
        Running {
            call_dir: call_dir.to_path_buf(),
            ended: std::thread::spawn(move || run_with_timeout(command)),
        }
    }

    /// The run once the call ended.
    fn wait(self, fixture: &Fixture) -> Run {
        let ended = self.ended.join().unwrap();
        fixture.collect(&self.call_dir, ended)
    }
}

/// Runs `line` with a reach plan of `ports` and a [`TestReach`] that answers `answer`.
fn run_reach(fixture: &Fixture, line: &str, ports: &[u16], answer: Answer) -> (Run, TestReach) {
    let (call_dir, reach) = prepare(fixture, line, ports, Some(answer));
    let run = Running::start(fixture, &call_dir).wait(fixture);
    (run, reach.unwrap())
}

/// A shell loop that waits until `path` exists: the call's own wait for the test.
fn wait_for_file(path: &Path) -> String {
    format!("while [[ ! -e {} ]]; do sleep 0.02; done", q(path))
}

/// The line of `self-test --dial` to `address` with `bytes` bytes.
fn dial(fixture: &Fixture, address: &str, bytes: usize) -> String {
    format!("{} self-test --dial '{address}' --dial-bytes {bytes}", launcher(fixture))
}

/// The line of `self-test --echo` on `address` that ends at once when its bind works,
/// and prints its error with the rest of the output when it does not.
fn try_bind(fixture: &Fixture, address: &str) -> String {
    format!("{} self-test --echo '{address}' --echo-ms 1 2>&1", launcher(fixture))
}

/// The lines of `check` that a self-test or a check printed: whether the first passed,
/// and the line for the message.
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

/// The listen of `port` on `addr`.
fn at(addr: IpAddr, port: u16) -> Listen {
    Listen { addr, port }
}

/// True while a reach helper of `fixture` runs: its command line names the fixture's
/// own inside socket, which no call of another test has.
fn helper_alive(fixture: &Fixture) -> bool {
    alive(&format!("reach --socket {}", fixture.spec.runtime.inside_reach_socket().display()))
}

#[test]
fn reach_port_of_start_is_bound_before_the_child() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // The child's first command connects: the port listens before the child runs.
    let line = dial(&fixture, &format!("127.0.0.1:{PORT}"), 5);
    let (run, reach) = run_reach(&fixture, &line, &[PORT], Answer::Echo);
    run.expect_status(0);
    passed(&run, "dial");
    let connect = reach.next_connect();
    assert_eq!((connect.kind, connect.target()), (FrameKind::Connect, at(V4, PORT)));
    let mut seen = Vec::new();
    let hello = reach.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    assert_eq!(hello.status, efr_sandbox::REACH_VERSION);
    let bound = reach.bound(&mut seen, at(V4, PORT));
    assert_eq!(bound.status, 0, "{seen:?}");
}

#[test]
fn connect_sends_connect_and_copies() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // 64 KiB there while the echo comes back, then the half-close: the end of the bytes
    // reaches efrd, and efrd's end comes back. A second connection works too.
    let address = format!("127.0.0.1:{PORT}");
    let line = format!(
        "{}; print -r -- \"first $?\"; {}",
        dial(&fixture, &address, 64 * 1024),
        dial(&fixture, &address, 7)
    );
    let (run, reach) = run_reach(&fixture, &line, &[PORT], Answer::Echo);
    run.expect_status(0);
    assert!(run.stdout.contains("first 0"), "{run:#?}");
    assert_eq!(run.stdout.matches("\"ok\":true").count(), 2, "{run:#?}");
    assert!(run.stdout.contains("sent 65536, got 65536 back"), "{run:#?}");
    for _ in 0..2 {
        let connect = reach.next_connect();
        assert_eq!((connect.kind, connect.target()), (FrameKind::Connect, at(V4, PORT)));
    }
}

#[test]
fn child_bind_of_reach_port_is_eaddrinuse() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = format!(
        "{}; print -r -- \"bind $?\"; {}; print -r -- \"other $?\"",
        try_bind(&fixture, &format!("127.0.0.1:{PORT}")),
        try_bind(&fixture, &format!("127.0.0.1:{LATER}"))
    );
    let (run, _reach) = run_reach(&fixture, &line, &[PORT], Answer::Echo);
    run.expect_status(0);
    assert!(run.stdout.contains("os error 98"), "{run:#?}");
    assert!(run.stdout.contains("bind 1"), "{run:#?}");
    // The control: a port that the helper does not hold binds.
    assert!(run.stdout.contains(&format!("echo 127.0.0.1:{LATER}")), "{run:#?}");
    assert!(run.stdout.contains("other 0"), "{run:#?}");
}

#[test]
fn later_reach_binds_a_port() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let go = fixture.project.join("go");
    let line =
        format!("{}; {}", wait_for_file(&go), dial(&fixture, &format!("127.0.0.1:{LATER}"), 5));
    let (call_dir, reach) = prepare(&fixture, &line, &[], Some(Answer::Echo));
    let reach = reach.unwrap();
    let call = Running::start(&fixture, &call_dir);
    let mut seen = Vec::new();
    reach.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    reach.send(Frame::reach(at(V4, LATER)));
    let bound = reach.bound(&mut seen, at(V4, LATER));
    assert_eq!(bound.status, 0, "{seen:?}");
    // A second `Reach` of a port that the helper holds changes nothing.
    reach.send(Frame::reach(at(V4, LATER)));
    let again = reach.bound(&mut seen, at(V4, LATER));
    assert_eq!(again.status, 0, "{seen:?}");
    write(&go, "go");
    let run = call.wait(&fixture);
    run.expect_status(0);
    passed(&run, "dial");
    assert_eq!(reach.next_connect().target(), at(V4, LATER));
}

#[test]
fn later_reach_of_a_port_the_child_holds_is_bound_eaddrinuse() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let (held, listening, go) = (
        fixture.project.join("held"),
        fixture.project.join("listening"),
        fixture.project.join("go"),
    );
    let line = format!(
        "{l} self-test --echo 127.0.0.1:{LATER} --echo-ms 20000 > {h} & \
         while [[ ! -s {h} ]]; do sleep 0.02; done; print ok > {s}; {w}; \
         {d}; kill $!; wait; print -r -- done",
        l = launcher(&fixture),
        h = q(&held),
        s = q(&listening),
        w = wait_for_file(&go),
        d = dial(&fixture, &format!("127.0.0.1:{LATER}"), 5),
    );
    let (call_dir, reach) = prepare(&fixture, &line, &[], Some(Answer::Echo));
    let reach = reach.unwrap();
    let call = Running::start(&fixture, &call_dir);
    let mut seen = Vec::new();
    reach.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    wait_until("the child's listen", || listening.exists());
    reach.send(Frame::reach(at(V4, LATER)));
    let bound = reach.bound(&mut seen, at(V4, LATER));
    assert_eq!(bound.status, EADDRINUSE, "{seen:?}");
    write(&go, "go");
    let run = call.wait(&fixture);
    run.expect_status(0);
    // The port stays the child's: its own server answers, and efrd gets no connection.
    passed(&run, "dial");
    assert!(run.stdout.contains("done"), "{run:#?}");
    assert!(reach.connects.try_recv().is_err());
}

#[test]
fn unreach_frees_the_port() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let go = fixture.project.join("go");
    let line = format!(
        "{}; {}; print -r -- \"bind $?\"",
        wait_for_file(&go),
        try_bind(&fixture, &format!("127.0.0.1:{PORT}"))
    );
    let (call_dir, reach) = prepare(&fixture, &line, &[PORT], Some(Answer::Echo));
    let reach = reach.unwrap();
    let call = Running::start(&fixture, &call_dir);
    let mut seen = Vec::new();
    reach.bound(&mut seen, at(V4, PORT));
    reach.bound(&mut seen, at(V6, PORT));
    reach.send(Frame::unreach(at(V4, PORT)));
    reach.send(Frame::unreach(at(V6, PORT)));
    // The helper reads its frames in order: the `Bound` of a later `Reach` comes after it
    // closed the listens above.
    reach.send(Frame::reach(at(V4, LATER)));
    assert_eq!(reach.bound(&mut seen, at(V4, LATER)).status, 0, "{seen:?}");
    write(&go, "go");
    let run = call.wait(&fixture);
    run.expect_status(0);
    assert!(run.stdout.contains(&format!("echo 127.0.0.1:{PORT}")), "{run:#?}");
    assert!(run.stdout.contains("bind 0"), "{run:#?}");
}

#[test]
fn helper_refuses_3128_and_below_1024() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let go = fixture.project.join("go");
    // The starter refuses such a port of the start; the helper refuses an address that
    // is not 127.0.0.1 or ::1, and ends at a frame that efrd's decoder refuses.
    let line = format!(
        "for p in 3128 80 1023; do {l} reach --socket {s} --port $p </dev/null 2>&1; \
         print -r -- \"starter $p $?\"; done; {w}; {d}; print -r -- \"dial $?\"",
        l = launcher(&fixture),
        s = inside_socket(&fixture),
        w = wait_for_file(&go),
        d = dial(&fixture, &format!("127.0.0.1:{PORT}"), 5),
    );
    let (call_dir, reach) = prepare(&fixture, &line, &[PORT], Some(Answer::Echo));
    let reach = reach.unwrap();
    let call = Running::start(&fixture, &call_dir);
    let mut seen = Vec::new();
    reach.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    let other = at(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), LATER);
    reach.send(Frame::reach(other));
    assert_eq!(reach.bound(&mut seen, other).status, 1, "EPERM: {seen:?}");
    // A `Reach` of port 80, which no efrd sends: the helper ends, and its listens go.
    reach.send_bytes(&Frame::reach(at(V4, 80)).encode());
    reach.wait_for_end();
    // The listens close with the helper's process, a moment after its control
    // connection.
    wait_until("the helper's end", || !helper_alive(&fixture));
    write(&go, "go");
    let run = call.wait(&fixture);
    run.expect_status(0);
    for port in [3128, 80, 1023] {
        assert!(run.stdout.contains(&format!("starter {port} 1")), "{port}: {run:#?}");
    }
    let (ok, line) = check_line(&run, "dial");
    assert!(!ok && line.contains("ConnectionRefused"), "{line}\n{run:#?}");
    assert!(reach.connects.try_recv().is_err());
}

#[test]
fn child_cannot_reach_the_reach_socket() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let go = fixture.project.join("go");
    let line = format!(
        "{}; {} self-test --check unix_socket --socket {}",
        wait_for_file(&go),
        launcher(&fixture),
        inside_socket(&fixture)
    );
    let (call_dir, reach) = prepare(&fixture, &line, &[PORT], Some(Answer::Echo));
    let reach = reach.unwrap();
    let call = Running::start(&fixture, &call_dir);
    let mut seen = Vec::new();
    reach.wait_for(&mut seen, |frame| frame.kind == FrameKind::Hello);
    write(&go, "go");
    let run = call.wait(&fixture);
    passed(&run, "unix_socket");
    // The socket got the helper's control connection and nothing else.
    assert_eq!(reach.connections(), 1);
    assert!(reach.connects.try_recv().is_err());
}

#[test]
fn child_cannot_signal_the_helper() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = format!("{} self-test --reach --check reach_signal", launcher(&fixture));
    let (run, _reach) = run_reach(&fixture, &line, &[PORT], Answer::Echo);
    passed(&run, "reach_signal");
}

#[test]
fn helper_proc_files_are_roots() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = format!("{} self-test --reach --check reach_environ", launcher(&fixture));
    let (run, _reach) = run_reach(&fixture, &line, &[PORT], Answer::Echo);
    passed(&run, "reach_environ");
}

#[test]
fn helper_cannot_connect_tcp_or_exec() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // The helper's setup and domain from inside the call: the call itself writes the
    // project; the helper's domain does not, and it cannot open a file, connect over
    // TCP or run a program, while it still listens.
    let line = format!(
        "print x > \"$PWD/call-write\"; {} reach --check --socket {} --write \"$PWD\"",
        launcher(&fixture),
        inside_socket(&fixture)
    );
    let (run, _reach) = run_reach(&fixture, &line, &[PORT], Answer::Echo);
    run.expect_status(0);
    for check in ["dumpable", "write", "open", "listen", "connect", "exec"] {
        passed(&run, check);
    }
    assert!(fixture.project.join("call-write").exists());
    assert!(!fixture.project.join(".efr-reach-write").exists());
}

#[test]
fn failed_helper_does_not_fail_the_call() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    // A stop in each step of the server: the starter reports it with exit 1, and no
    // server of that start keeps running.
    let line = format!(
        "for step in close session name dumpable landlock seccomp; do \
         {} reach --socket {} --fail-setup $step </dev/null; print -r -- \"starter $step $?\"; \
         for f in /proc/[0-9]*/cmdline(N); do c=\"$(<$f)\"; \
         [[ $c == *fail-setup* && $c == *{}* ]] && print found; done; done",
        launcher(&fixture),
        inside_socket(&fixture),
        fixture.spec.runtime.inside_reach_socket().display()
    );
    let (run, _reach) = run_reach(&fixture, &line, &[PORT], Answer::Echo);
    for step in ["close", "session", "name", "dumpable", "landlock", "seccomp"] {
        assert!(run.stdout.contains(&format!("starter {step} 1")), "{step}: {run:#?}");
    }
    assert!(!run.stdout.contains("found"), "{run:#?}");
    // The real helper of a call whose socket does not answer: it gives up after its
    // setup, sends no Hello, and the call runs and ends as it would with no reach.
    let (call_dir, _) = prepare(&fixture, "print ran", &[PORT], None);
    let run = Running::start(&fixture, &call_dir).wait(&fixture);
    run.expect_status(0);
    assert_eq!(run.stdout, "ran\n", "{run:#?}");
    assert!(run.result().setup_error.is_none(), "{run:#?}");
    assert_eq!(run.result().exit_code, Some(0), "{run:#?}");
    assert!(!helper_alive(&fixture), "a reach helper outlived its call");
}

#[test]
fn v6_listener_matches_the_namespace() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let go = fixture.project.join("go");
    // The call reads whether its own loopback has ::1, and the test asserts the listen
    // for that state.
    let v6 = format!("[::1]:{PORT}");
    let line = format!(
        "{w}; if [[ -r /proc/net/if_inet6 && \"$(</proc/net/if_inet6)\" == \
         *00000000000000000000000000000001*lo* ]]; then print -r -- 'v6 on'; \
         else print -r -- 'v6 off'; fi; {b}; print -r -- \"bind $?\"; {d}",
        w = wait_for_file(&go),
        b = try_bind(&fixture, &v6),
        d = dial(&fixture, &v6, 5),
    );
    let (call_dir, reach) = prepare(&fixture, &line, &[PORT], Some(Answer::Echo));
    let reach = reach.unwrap();
    let call = Running::start(&fixture, &call_dir);
    let mut seen = Vec::new();
    let bound_v4 = reach.bound(&mut seen, at(V4, PORT));
    let bound_v6 = reach.bound(&mut seen, at(V6, PORT));
    write(&go, "go");
    let run = call.wait(&fixture);
    run.expect_status(0);
    assert_eq!(bound_v4.status, 0, "{seen:?}");
    let (dialled, dial_line) = check_line(&run, "dial");
    let on = run.stdout.contains("v6 on");
    say(&format!("the call's loopback has ::1: {on}"));
    if on {
        assert_eq!(bound_v6.status, 0, "{seen:?}");
        assert!(run.stdout.contains("os error 98") && run.stdout.contains("bind 1"), "{run:#?}");
        assert!(dialled, "{dial_line}");
        assert_eq!(reach.next_connect().target(), at(V6, PORT));
    } else {
        assert!(run.stdout.contains("v6 off"), "{run:#?}");
        assert_ne!(bound_v6.status, 0, "{seen:?}");
        assert!(!run.stdout.contains("os error 98"), "{run:#?}");
        assert!(!dialled, "{dial_line}");
    }
}

#[test]
fn refused_route_resets_the_client() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let line = dial(&fixture, &format!("127.0.0.1:{PORT}"), 5);
    let (run, reach) = run_reach(&fixture, &line, &[PORT], Answer::Close);
    assert_eq!(reach.next_connect().target(), at(V4, PORT));
    let (ok, line) = check_line(&run, "dial");
    assert!(!ok && line.contains("ConnectionReset"), "{line}\n{run:#?}");
}

#[test]
fn no_helper_without_plan() {
    let ready = sandbox_or_skip!();
    let fixture = Fixture::new(&ready);
    let run =
        fixture.run(&format!("{} self-test --reach --check reach_signal", launcher(&fixture)));
    let (ok, line) = check_line(&run, "reach_signal");
    assert!(!ok && line.contains("no efr-reach of this sandbox"), "{line}\n{run:#?}");
    assert!(!run.call_dir.join(efr_sandbox::REACH_SOCKET_FILE).exists());
}
