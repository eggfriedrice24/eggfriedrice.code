//! The port forwards of background jobs on efrd's side (v0.0.4 live items, DL1-DL16),
//! end to end through a `TestDaemon`: the shared fake launcher of `efr-test-daemon`,
//! which starts no forwarder, and a `FakeForwarder` that the test plays on the job's
//! forward socket, with an echo server of the test behind each port. efrd binds each
//! forwarded port through the test seam, which takes a free port of `127.0.0.1` in its
//! place; only `real_host_bind` binds a real port. A job that must outlive its turn
//! blocks on a FIFO; no test waits on a fixed time.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    AdminConfigReload, AdminConfigReloadResult, ApprovalDecision, CallId, ConversationId, Event,
    EventEnvelope, ForwardOutcome, JobId, JobInfo, JobState, JobsList, JobsListResult, Method,
    Mode, NetworkMode, PortForward, SandboxStatus,
};
use efr_provider::Provider;
use efr_sandbox::{Frame, FrameKind, SandboxSpec};
use efr_test_daemon::{FakeForwarder, FakeLauncher, TestDaemon, TestDaemonBuilder, echo_server};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, ready, run_turn};

/// What the start result of a job with forwards says (DL16).
const START_NOTE: &str = "efr forwards each loopback port from 1024 up that this job \
    listens on to the same port of 127.0.0.1 on the user's machine";

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// A test's dirs and its fake launcher.
struct Setup {
    dirs: Arc<TestDirs>,
    fake: FakeLauncher,
}

impl Setup {
    fn new() -> Setup {
        let dirs = Arc::new(TestDirs::new().unwrap());
        let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
        Setup { dirs, fake }
    }

    /// A daemon with the fake launcher, a ready sandbox and a model that starts a job
    /// for each line of `lines`, all in one turn.
    fn builder(&self, lines: &[String]) -> TestDaemonBuilder {
        let env = BTreeMap::from([
            ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), self.dirs.home().display().to_string()),
        ]);
        let calls: Vec<(&str, Value)> = lines
            .iter()
            .map(|line| ("shell", json!({ "command": line, "background": true })))
            .collect();
        let model = ScriptedModel::reviewed(calls, REVIEWER_ALLOWS);
        TestDaemon::builder()
            .dirs(Arc::clone(&self.dirs))
            .sandbox_launcher(self.fake.path())
            .probe_override(ready())
            .job_env(env)
            .custom_provider(model as Arc<dyn Provider>)
    }

    /// A FIFO below the test's root, which a job reads and the test writes.
    fn fifo(&self, name: &str) -> PathBuf {
        let path = self.dirs.root().join(name);
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        path
    }

    /// The specs that the fake launcher kept, one per job, in the order of their calls.
    fn specs(&self) -> Vec<SandboxSpec> {
        let mut specs: Vec<SandboxSpec> = std::fs::read_dir(self.fake.dir())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("spec-"))
            .map(|entry| SandboxSpec::from_json(&std::fs::read(entry.path()).unwrap()).unwrap())
            .collect();
        specs.sort_by_key(|spec| spec.call);
        specs
    }

    /// The forward socket of each job that `events` started, in the order of the jobs'
    /// numbers: j1 first.
    fn sockets(&self, events: &[EventEnvelope]) -> Vec<PathBuf> {
        let mut started: Vec<(u32, CallId)> = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                Event::JobStarted { job, call_id, .. } => Some((job.id.number()?, *call_id)),
                _ => None,
            })
            .collect();
        started.sort_unstable();
        let specs = self.specs();
        started
            .into_iter()
            .map(|(_, call)| {
                let spec = specs.iter().find(|spec| spec.call == call).expect("its spec");
                spec.job.clone().unwrap().forward.expect("the job forwards").socket
            })
            .collect()
    }

    /// A forwarder on the forward socket of the only job.
    async fn forwarder(&self) -> FakeForwarder {
        let specs = self.specs();
        let [spec] = specs.as_slice() else { panic!("{specs:?}") };
        let socket = &spec.job.as_ref().unwrap().forward.as_ref().expect("the job forwards").socket;
        FakeForwarder::connect(socket).await.unwrap()
    }
}

/// A line that blocks until the test writes `fifo`.
fn wait_on(fifo: &Path) -> String {
    format!("read answer < {}", fifo.display())
}

/// Writes one line to `fifo`, which a job reads; it waits for the reader.
async fn release(fifo: &Path) {
    let fifo = fifo.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
}

/// The events of `conversation` from its start up to the first that `stop` accepts.
async fn until(
    daemon: &TestDaemon,
    conversation: ConversationId,
    stop: impl FnMut(&Event) -> bool,
) -> Vec<EventEnvelope> {
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    efr_test_daemon::events_until(&mut follow, stop).await.unwrap()
}

/// One `job_port_forwarded`, as the test reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Forwarded {
    job: String,
    listen: String,
    port: u16,
    outcome: ForwardOutcome,
    by_job: Option<String>,
    reason: Option<String>,
}

impl Forwarded {
    fn new(job: &str, listen: &str, port: u16, outcome: ForwardOutcome) -> Forwarded {
        Forwarded {
            job: job.to_owned(),
            listen: listen.to_owned(),
            port,
            outcome,
            by_job: None,
            reason: None,
        }
    }

    fn because(self, reason: &str) -> Forwarded {
        Forwarded { reason: Some(reason.to_owned()), ..self }
    }
}

/// The `job_port_forwarded` events of `events`.
fn forwards(events: &[EventEnvelope]) -> Vec<Forwarded> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobPortForwarded { job_id, listen, port, outcome, by_job, reason } => {
                Some(Forwarded {
                    job: job_id.to_string(),
                    listen: listen.clone(),
                    port: *port,
                    outcome: *outcome,
                    by_job: by_job.as_ref().map(JobId::to_string),
                    reason: reason.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

/// The first `count` `job_port_forwarded` events of `conversation`, once they came.
async fn forwarded(
    daemon: &TestDaemon,
    conversation: ConversationId,
    count: usize,
) -> Vec<Forwarded> {
    let mut seen = 0;
    let events = until(daemon, conversation, |event| {
        if matches!(event, Event::JobPortForwarded { .. }) {
            seen += 1;
        }
        seen == count
    })
    .await;
    forwards(&events)
}

/// The jobs of `conversation`, as `jobs.list` shows them.
async fn jobs(daemon: &TestDaemon, conversation: ConversationId) -> Vec<JobInfo> {
    let client = daemon.client().await.unwrap();
    let list: JobsListResult = client
        .call(Method::JobsList(JobsList { conversation_id: Some(conversation) }))
        .await
        .unwrap();
    list.jobs
}

/// The output of each finished tool call of `events`.
fn outputs(events: &[EventEnvelope]) -> Vec<String> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, .. } => Some(output.clone()),
            _ => None,
        })
        .collect()
}

/// Sends `bytes` through `127.0.0.1:port` and returns what came back after a half close.
async fn through(port: u16, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
    stream.write_all(bytes).await?;
    stream.shutdown().await?;
    let mut back = Vec::new();
    stream.read_to_end(&mut back).await?;
    Ok(back)
}

/// True when nothing listens on `127.0.0.1:port`.
fn refused(port: u16) -> bool {
    std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
}

/// Writes `config.toml` with `text` and reloads it.
async fn reload_with(setup: &Setup, daemon: &TestDaemon, text: &str) {
    crate::full::write_config(&setup.dirs, text);
    let client = daemon.client().await.unwrap();
    let reloaded: AdminConfigReloadResult =
        client.call(Method::AdminConfigReload(AdminConfigReload {})).await.unwrap();
    assert!(reloaded.applied, "{reloaded:?}");
}

/// A daemon with one job that blocks on a FIFO, its conversation, the FIFO and the
/// job's start result.
async fn one_job(
    setup: &Setup,
    builder: impl FnOnce(TestDaemonBuilder) -> TestDaemonBuilder,
) -> (TestDaemon, ConversationId, PathBuf, String) {
    let fifo = setup.fifo("go");
    let daemon = builder(setup.builder(&[wait_on(&fifo)])).start().await.unwrap();
    let (sent, events) =
        run_turn(&daemon, "start the server", Mode::Auto, ApprovalDecision::Deny).await;
    let [output] = outputs(&events).try_into().unwrap();
    (daemon, sent.conversation_id, fifo, output)
}

#[tokio::test]
async fn forward_line_and_record() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, output) = one_job(&setup, |builder| builder).await;
    assert!(output.contains(START_NOTE), "{output}");
    let [spec] = setup.specs().try_into().unwrap();
    let plan = spec.job.unwrap().forward.unwrap();
    assert_eq!(plan.socket, spec.runtime.call_dir.join("forward.sock"));
    assert!(!plan.proxy, "network.mode = none");
    let mut forwarder = setup.forwarder().await;
    let echo = echo_server().await.unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    let seen = forwarded(&daemon, conversation, 1).await;
    assert_eq!(seen, [Forwarded::new("j1", "127.0.0.1", 8765, ForwardOutcome::Forwarded)]);
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!(job.state, JobState::Running);
    assert_eq!(job.forwards, [PortForward { listen: "127.0.0.1".to_owned(), port: 8765 }]);
    let client = daemon.client().await.unwrap();
    let stored = daemon.events(&client, conversation).await.unwrap();
    assert_eq!(forwards(&stored), seen, "the history has the record");
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn start_note_names_forwarding() {
    let setup = Setup::new();
    let (daemon, _, fifo, output) = one_job(&setup, |builder| builder).await;
    assert!(output.contains("give the user the URL, such as http://localhost:8765"), "{output}");
    assert!(output.contains("well-known service ports are not forwarded"), "{output}");
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn late_hello_still_forwards() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, _) = one_job(&setup, |builder| builder).await;
    // NOTE: efrd warns once when no hello came in 5 s and waits on (DL7). The clock
    // moves only once that wait exists.
    let hello_wait = Duration::from_secs(5);
    Wait::new("efrd waits for the forwarder's hello")
        .until(|| daemon.clock().requested_sleeps().contains(&hello_wait))
        .await
        .unwrap();
    daemon.clock().advance(hello_wait);
    let mut forwarder = setup.forwarder().await;
    let echo = echo_server().await.unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    let seen = forwarded(&daemon, conversation, 1).await;
    assert_eq!(seen, [Forwarded::new("j1", "127.0.0.1", 8765, ForwardOutcome::Forwarded)]);
    let real = daemon.host_ports().real_port(8765).unwrap();
    assert_eq!(through(real, b"late").await.unwrap(), b"late");
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn forwarded_port_copies_bytes() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, _) = one_job(&setup, |builder| builder).await;
    let mut forwarder = setup.forwarder().await;
    let echo = echo_server().await.unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    forwarded(&daemon, conversation, 1).await;
    let real = daemon.host_ports().real_port(8765).unwrap();
    let bytes: Vec<u8> = (0..64 * 1024).map(|n: u32| (n % 251) as u8).collect();
    assert_eq!(through(real, &bytes).await.unwrap(), bytes, "64 KiB both ways, then a half close");
    assert_eq!(through(real, b"again").await.unwrap(), b"again");
    assert_eq!(forwarder.opens(), 2);
    // NOTE: a gone server keeps the port; a connection then closes at once (DL11).
    forwarder.gone(LOCALHOST, 8765).await.unwrap();
    Wait::new("the gone port closes connections")
        // NOTE: efrd closes with the byte unread, so the close may come as a reset.
        .until_some_async(async || {
            through(real, b"x").await.map_or(true, |b| b.is_empty()).then_some(())
        })
        .await
        .unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    Wait::new("the port is back")
        .until_some_async(async || through(real, b"back").await.ok().filter(|b| b == b"back"))
        .await
        .unwrap();
    assert_eq!(records(&daemon, conversation).await, 1, "the port stayed the job's");
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn port_in_use_is_not_forwarded() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, _) = one_job(&setup, |builder| builder).await;
    daemon.host_ports().set_in_use(8765);
    daemon.host_ports().set_v6_listen(8766);
    let mut forwarder = setup.forwarder().await;
    forwarder.listen(LOCALHOST, 8765, None).await.unwrap();
    forwarder.listen(IpAddr::V6(Ipv6Addr::LOCALHOST), 8766, None).await.unwrap();
    let seen = forwarded(&daemon, conversation, 2).await;
    assert_eq!(
        seen,
        [
            Forwarded::new("j1", "127.0.0.1", 8765, ForwardOutcome::InUse),
            Forwarded::new("j1", "::1", 8766, ForwardOutcome::InUse),
        ]
    );
    assert_eq!(daemon.host_ports().real_port(8766), None, "a server on ::1 holds it: no bind");
    // NOTE: the same port again after a gone gets no second record.
    forwarder.gone(LOCALHOST, 8765).await.unwrap();
    forwarder.listen(LOCALHOST, 8765, None).await.unwrap();
    forwarder.listen(LOCALHOST, 8767, None).await.unwrap();
    let seen = forwarded(&daemon, conversation, 3).await;
    assert_eq!(seen[2], Forwarded::new("j1", "127.0.0.1", 8767, ForwardOutcome::Forwarded));
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!(job.forwards, [PortForward { listen: "127.0.0.1".to_owned(), port: 8767 }]);
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn port_of_another_job_is_in_use_by_it() {
    let setup = Setup::new();
    let (first, second) = (setup.fifo("first"), setup.fifo("second"));
    let daemon = setup.builder(&[wait_on(&first), wait_on(&second)]).start().await.unwrap();
    let (sent, events) = run_turn(&daemon, "start both", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets(&events);
    let [one, two] = sockets.as_slice() else { panic!("{sockets:?}") };
    let mut one = FakeForwarder::connect(one).await.unwrap();
    let mut two = FakeForwarder::connect(two).await.unwrap();
    one.listen(LOCALHOST, 8765, None).await.unwrap();
    forwarded(&daemon, sent.conversation_id, 1).await;
    two.listen(LOCALHOST, 8765, None).await.unwrap();
    let seen = forwarded(&daemon, sent.conversation_id, 2).await;
    let in_use = Forwarded {
        by_job: Some("j1".to_owned()),
        ..Forwarded::new("j2", "127.0.0.1", 8765, ForwardOutcome::InUse)
    };
    assert_eq!(seen[1], in_use);
    release(&first).await;
    release(&second).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn refusals_are_rechecked_in_efrd() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, _) = one_job(&setup, |builder| builder).await;
    let mut forwarder = setup.forwarder().await;
    for port in [1455, 3128, 5432] {
        forwarder.listen(LOCALHOST, port, None).await.unwrap();
    }
    // NOTE: a service port again after a gone gets no second record.
    forwarder.gone(LOCALHOST, 1455).await.unwrap();
    forwarder.listen(LOCALHOST, 1455, None).await.unwrap();
    forwarder.listen(LOCALHOST, 8765, None).await.unwrap();
    let seen = forwarded(&daemon, conversation, 4).await;
    let service = |port| {
        Forwarded::new("j1", "127.0.0.1", port, ForwardOutcome::Refused).because("a service port")
    };
    assert_eq!(
        seen,
        [
            service(1455),
            service(3128),
            service(5432),
            Forwarded::new("j1", "127.0.0.1", 8765, ForwardOutcome::Forwarded),
        ]
    );
    for port in [1455, 3128, 5432] {
        assert_eq!(daemon.host_ports().real_port(port), None, "no bind of {port}");
    }
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn bridge_port_of_a_proxy_job_is_refused() {
    let setup = Setup::new();
    let proxy = SandboxStatus { proxy: true, ..ready() };
    let (daemon, conversation, fifo, _) =
        one_job(&setup, |builder| builder.probe_override(proxy).network_mode(NetworkMode::Proxy))
            .await;
    let [spec] = setup.specs().try_into().unwrap();
    assert!(spec.job.unwrap().forward.unwrap().proxy, "a proxy job's plan says so");
    let mut forwarder = setup.forwarder().await;
    forwarder.listen(LOCALHOST, 3128, None).await.unwrap();
    let seen = forwarded(&daemon, conversation, 1).await;
    assert_eq!(
        seen,
        [Forwarded::new("j1", "127.0.0.1", 3128, ForwardOutcome::Refused)
            .because("the port of the proxy bridge")]
    );
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn limits() {
    let setup = Setup::new();
    let fifos: Vec<PathBuf> = (1..=5).map(|n| setup.fifo(&format!("go-{n}"))).collect();
    let lines: Vec<String> = fifos.iter().map(|fifo| wait_on(fifo)).collect();
    let daemon = setup
        .builder(&lines)
        .config(|settings| settings.jobs.max_running = 5)
        .start()
        .await
        .unwrap();
    let (sent, events) = run_turn(&daemon, "start five", Mode::Auto, ApprovalDecision::Deny).await;
    let conversation = sent.conversation_id;
    let sockets = setup.sockets(&events);
    assert_eq!(sockets.len(), 5);
    let mut forwarders = Vec::new();
    for socket in &sockets {
        forwarders.push(FakeForwarder::connect(socket).await.unwrap());
    }
    // NOTE: the 9th port of j1 reaches the job's limit once; the 10th gets no record.
    for port in 20_001..=20_010 {
        forwarders[0].listen(LOCALHOST, port, None).await.unwrap();
    }
    let seen = forwarded(&daemon, conversation, 9).await;
    assert!(seen[..8].iter().all(|seen| seen.outcome == ForwardOutcome::Forwarded), "{seen:?}");
    assert_eq!(
        seen[8],
        Forwarded::new("j1", "127.0.0.1", 20_009, ForwardOutcome::Limit)
            .because("the job forwards 8 ports already")
    );
    // NOTE: j2 to j4 forward 8 ports each: efrd holds 32, and the 33rd is over its limit.
    for (n, forwarder) in forwarders[1..4].iter_mut().enumerate() {
        let first = 21_000 + u16::try_from(n).unwrap() * 100;
        for port in first..first + 8 {
            forwarder.listen(LOCALHOST, port, None).await.unwrap();
        }
    }
    let seen = forwarded(&daemon, conversation, 33).await;
    assert_eq!(seen.iter().filter(|seen| seen.outcome == ForwardOutcome::Forwarded).count(), 32);
    forwarders[4].listen(LOCALHOST, 22_000, None).await.unwrap();
    let seen = forwarded(&daemon, conversation, 34).await;
    assert_eq!(
        seen[33],
        Forwarded::new("j5", "127.0.0.1", 22_000, ForwardOutcome::Limit)
            .because("efrd forwards 32 ports already")
    );
    for fifo in &fifos {
        release(fifo).await;
    }
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn other_uid_is_closed() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, _) = one_job(&setup, |builder| builder).await;
    let mut forwarder = setup.forwarder().await;
    let echo = echo_server().await.unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    forwarded(&daemon, conversation, 1).await;
    let own = rustix::process::geteuid().as_raw();
    daemon.host_ports().set_peer_uid(own + 1);
    let real = daemon.host_ports().real_port(8765).unwrap();
    assert_eq!(through(real, b"secret").await.unwrap_or_default(), b"", "closed with no byte");
    assert_eq!(forwarder.opens(), 0, "efrd sent no open");
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn bad_frame_closes_the_forwarder() {
    let setup = Setup::new();
    let (first, second) = (setup.fifo("first"), setup.fifo("second"));
    let daemon = setup.builder(&[wait_on(&first), wait_on(&second)]).start().await.unwrap();
    let (sent, events) = run_turn(&daemon, "start both", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets(&events);
    let [one, two] = sockets.as_slice() else { panic!("{sockets:?}") };
    let mut reserved = FakeForwarder::connect(one).await.unwrap();
    let mut bytes = Frame::listen(efr_sandbox::Listen { addr: LOCALHOST, port: 8765 }).encode();
    bytes[3] = 1;
    reserved.send_raw(bytes).await.unwrap();
    reserved.closed().await;
    // NOTE: a port below 1024 never decodes: the forwarder is closed, and nothing binds.
    let mut low = FakeForwarder::connect(two).await.unwrap();
    let frame = Frame { kind: FrameKind::Listen, status: 0, id: 0, addr: LOCALHOST, port: 80 };
    low.send_raw(frame.encode()).await.unwrap();
    low.closed().await;
    assert_eq!(daemon.host_ports().real_port(80), None);
    assert_eq!(daemon.host_ports().real_port(8765), None);
    let listed = jobs(&daemon, sent.conversation_id).await;
    assert!(listed.iter().all(|job| job.state == JobState::Running), "the jobs run on");
    assert!(listed.iter().all(|job| job.forwards.is_empty()));
    let client = daemon.client().await.unwrap();
    let stored = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert_eq!(forwards(&stored), [], "no record");
    release(&first).await;
    release(&second).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn job_end_closes_listeners() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, _) = one_job(&setup, |builder| builder).await;
    let mut forwarder = setup.forwarder().await;
    let echo = echo_server().await.unwrap();
    forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    forwarded(&daemon, conversation, 1).await;
    let real = daemon.host_ports().real_port(8765).unwrap();
    assert!(!refused(real));
    release(&fifo).await;
    until(&daemon, conversation, |event| matches!(event, Event::JobEnded { .. })).await;
    Wait::new("the listener closes").until(|| refused(real)).await.unwrap();
    forwarder.closed().await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn efrd_stop_closes_listeners() {
    let setup = Setup::new();
    let (daemon, conversation, _fifo, _) = one_job(&setup, |builder| builder).await;
    let mut forwarder = setup.forwarder().await;
    forwarder.listen(LOCALHOST, 8765, None).await.unwrap();
    forwarded(&daemon, conversation, 1).await;
    let real = daemon.host_ports().real_port(8765).unwrap();
    assert!(!refused(real));
    daemon.stop().await.unwrap();
    assert!(refused(real), "nothing listens after efrd stopped");
    forwarder.closed().await;
}

/// A daemon with one job that forwards port 8765, and a reload that turns forwards
/// off; returns the daemon, the conversation, the job's FIFO, the forwarder and the
/// free port that stood for 8765.
async fn forwarded_then_off(
    setup: &Setup,
) -> (TestDaemon, ConversationId, PathBuf, FakeForwarder, u16) {
    let (daemon, conversation, fifo, _) = one_job(setup, |builder| builder).await;
    let mut forwarder = setup.forwarder().await;
    forwarder.listen(LOCALHOST, 8765, None).await.unwrap();
    forwarded(&daemon, conversation, 1).await;
    let real = daemon.host_ports().real_port(8765).unwrap();
    reload_with(setup, &daemon, "[jobs]\nforward_ports = false\n").await;
    (daemon, conversation, fifo, forwarder, real)
}

#[tokio::test]
async fn reload_off_stops_forwards() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, mut forwarder, real) = forwarded_then_off(&setup).await;
    let seen = forwarded(&daemon, conversation, 2).await;
    assert_eq!(
        seen[1],
        Forwarded::new("j1", "127.0.0.1", 8765, ForwardOutcome::Stopped)
            .because("jobs.forward_ports is off")
    );
    assert!(refused(real), "the listener closed before the record");
    forwarder.closed().await;
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!((job.state, job.forwards), (JobState::Running, Vec::new()), "the job runs on");
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn reload_on_does_not_reopen() {
    let setup = Setup::new();
    let (daemon, conversation, fifo, mut forwarder, real) = forwarded_then_off(&setup).await;
    forwarded(&daemon, conversation, 2).await;
    forwarder.closed().await;
    reload_with(&setup, &daemon, "[jobs]\nforward_ports = true\n").await;
    // NOTE: the forwarder's connection is gone for good; a new one is no control
    // connection, and the job keeps no forward.
    assert!(FakeForwarder::connect(forwarder.socket()).await.is_err(), "the socket closed");
    assert!(refused(real));
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!(job.forwards, Vec::new());
    let client = daemon.client().await.unwrap();
    let stored = daemon.events(&client, conversation).await.unwrap();
    assert_eq!(forwards(&stored).len(), 2, "forwarded, then stopped; nothing more");
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn no_forward_when_off() {
    let setup = Setup::new();
    let (daemon, _, fifo, output) =
        one_job(&setup, |builder| builder.config(|settings| settings.jobs.forward_ports = false))
            .await;
    assert!(!output.contains(START_NOTE), "{output}");
    let [spec] = setup.specs().try_into().unwrap();
    assert_eq!(spec.job.unwrap().forward, None);
    assert!(!spec.runtime.call_dir.join("forward.sock").exists());
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn no_forward_for_exit_child_job() {
    // NOTE: outside `auto` an approved job runs in the exit child, on the host's
    // loopback already.
    let setup = Setup::new();
    let fifo = setup.fifo("go");
    let daemon = setup.builder(&[wait_on(&fifo)]).start().await.unwrap();
    let (_, events) = run_turn(&daemon, "start it", Mode::Manual, ApprovalDecision::Allow).await;
    let [output] = outputs(&events).try_into().unwrap();
    assert!(!output.contains(START_NOTE), "{output}");
    let [spec] = setup.specs().try_into().unwrap();
    assert_eq!(spec.launch, efr_sandbox::SpecLaunch::Unsandboxed);
    assert_eq!(spec.job.unwrap().forward, None);
    assert!(!spec.runtime.call_dir.join("forward.sock").exists());
    release(&fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn no_forward_with_netview() {
    // NOTE: a network view job runs in the host's network namespace and cannot listen.
    let setup = Setup::new();
    let netview = SandboxStatus { netview: true, ..ready() };
    let daemon =
        setup.builder(&["ss -tln".to_owned()]).probe_override(netview).start().await.unwrap();
    run_turn(&daemon, "show the ports", Mode::Auto, ApprovalDecision::Deny).await;
    let [spec] = setup.specs().try_into().unwrap();
    assert!(spec.grants.contains(&efr_protocol::Grant::NetView), "{:?}", spec.grants);
    assert_eq!(spec.job.unwrap().forward, None);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn real_host_bind() {
    // NOTE: the one test that binds a real port of the machine, through the system's
    // host ports: a free port below 32768, so no ephemeral port of another program,
    // tried again on another port when a program took it meanwhile.
    let setup = Setup::new();
    let (daemon, conversation, fifo, _) =
        one_job(&setup, TestDaemonBuilder::system_host_ports).await;
    let mut forwarder = setup.forwarder().await;
    let echo = echo_server().await.unwrap();
    let base = 20_000 + u16::try_from(std::process::id() % 5_000).unwrap() * 2;
    let mut port = None;
    for (attempt, candidate) in (base..base + 10).enumerate() {
        let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, candidate)).is_ok();
        if !free {
            continue;
        }
        let before = records(&daemon, conversation).await;
        forwarder.listen(LOCALHOST, candidate, Some(echo)).await.unwrap();
        let seen = forwarded(&daemon, conversation, before + 1).await;
        let last = seen.last().unwrap().clone();
        assert_eq!(last.port, candidate, "attempt {attempt}");
        if last.outcome == ForwardOutcome::Forwarded {
            port = Some(candidate);
            break;
        }
    }
    let port = port.expect("a free port below 32768");
    assert_eq!(through(port, b"real").await.unwrap(), b"real");
    release(&fifo).await;
    until(&daemon, conversation, |event| matches!(event, Event::JobEnded { .. })).await;
    Wait::new("the real port is free again").until(|| refused(port)).await.unwrap();
    daemon.stop().await.unwrap();
}

/// How many `job_port_forwarded` events `conversation` has so far.
async fn records(daemon: &TestDaemon, conversation: ConversationId) -> usize {
    let client = daemon.client().await.unwrap();
    forwards(&daemon.events(&client, conversation).await.unwrap()).len()
}
