//! The reach of a contained call on efrd's side (v0.0.4 shell items, DS34-DS48), end to
//! end through a `TestDaemon`: the shared fake launcher of `efr-test-daemon`, which
//! starts no forwarder and no reach helper, a `FakeForwarder` that the test plays on the
//! job's forward socket with an echo server behind each port, and a `FakeReach` that the
//! test plays on the call's reach socket, with no TCP port.
//!
//! A foreground call runs through a real hidden zsh (`EFR_TEST_ZSH=1`), so most tests
//! skip without it. A job or a call that must outlive a step blocks on a FIFO; no test
//! waits on a fixed time, and the test clock moves only after the wait that it ends
//! exists. Every job turns its wakes off (`jobs.max_wakes_per_hour = 0`). The line that
//! names a port runs a fake `curl` of the test, so nothing connects to a real port.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use efr_protocol::{
    AdminConfigReload, AdminConfigReloadResult, ApprovalDecision, ApprovalRespond,
    ApprovalRespondResult, ConversationId, Event, EventEnvelope, Grant, Launch, Method, Mode,
    PromptSendResult, TurnSettings,
};
use efr_provider::Provider;
use efr_sandbox::{SandboxSpec, SpecLaunch};
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::{
    FakeForwarder, FakeLauncher, FakeReach, TTY, TestDaemon, TestDaemonBuilder, Told, command_id,
    echo_server, events_until,
};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::jobs_wake::{Answer, Recorder};
use crate::sandbox::ready;
use crate::support::zsh_enabled;

mod unforwarded;

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The terminal of a second conversation.
const OTHER_TTY: &str = "/dev/pts/efr-test-b";

/// What the start result of a job with forwards says about the reach (DS45).
const REACH_NOTE: &str = "Later contained calls of this conversation reach the forwarded \
    ports at http://127.0.0.1:<port> or localhost, with no needs.hosts entry.";

/// How long the fact of a line waits for the port of a new job (DS44).
const SETTLE: Duration = Duration::from_secs(3);

/// The two listens of a port in the call: `127.0.0.1` and `[::1]` (DS36).
fn both(port: u16) -> [SocketAddr; 2] {
    [SocketAddr::new(LOCALHOST, port), SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)]
}

/// A test's dirs, its fake launcher and the dir of its fake `curl`.
struct Setup {
    dirs: Arc<TestDirs>,
    fake: FakeLauncher,
    bin: PathBuf,
}

impl Setup {
    fn new() -> Setup {
        let dirs = Arc::new(TestDirs::new().unwrap());
        let fake = FakeLauncher::write(&dirs.root().join("fake")).unwrap();
        let bin = dirs.create_dir("bin").unwrap();
        let curl = bin.join("curl");
        std::fs::write(&curl, "#!/bin/sh\necho fake-curl\n").unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        Setup { dirs, fake, bin }
    }

    /// A daemon with a real hidden zsh, the fake launcher, a ready sandbox with no
    /// network, no wakes and a model that gives `answers` in turn.
    fn builder(&self, answers: Vec<Answer>) -> TestDaemonBuilder {
        let env = BTreeMap::from([
            ("PATH".to_owned(), format!("{}:/usr/local/bin:/usr/bin:/bin", self.bin.display())),
            ("HOME".to_owned(), self.dirs.home().display().to_string()),
        ]);
        TestDaemon::builder()
            .local_pty()
            .dirs(Arc::clone(&self.dirs))
            .sandbox_launcher(self.fake.path())
            .probe_override(ready())
            .shell_env(env.clone())
            .job_env(env)
            .config(|settings| settings.jobs.max_wakes_per_hour = 0)
            .custom_provider(Recorder::new(answers) as Arc<dyn Provider>)
    }

    /// A FIFO below the test's root, which a job or a call reads and the test writes.
    fn fifo(&self, name: &str) -> PathBuf {
        let path = self.dirs.root().join(name);
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        path
    }

    /// The specs that the fake launcher kept, in the order of their calls.
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

    /// The forwarder of the only job of `conversation`.
    async fn forwarder(&self, conversation: ConversationId) -> FakeForwarder {
        let specs: Vec<SandboxSpec> = self
            .specs()
            .into_iter()
            .filter(|spec| spec.conversation == conversation && spec.job.is_some())
            .collect();
        let [spec] = specs.as_slice() else { panic!("{specs:?}") };
        let plan = spec.job.as_ref().unwrap().forward.as_ref().expect("the job forwards");
        FakeForwarder::connect(&plan.socket).await.unwrap()
    }

    /// The spec of the foreground call of `conversation` that runs now, once the
    /// launcher started it.
    async fn call_spec(&self, conversation: ConversationId, nth: usize) -> SandboxSpec {
        Wait::new("the spec of the foreground call")
            .until_some(|| {
                self.specs()
                    .into_iter()
                    .filter(|spec| spec.conversation == conversation && spec.job.is_none())
                    .nth(nth)
            })
            .await
            .unwrap()
    }

    /// The reach helper of that call, as the test plays it, with the call's spec.
    async fn reach(&self, conversation: ConversationId, nth: usize) -> (SandboxSpec, FakeReach) {
        let spec = self.call_spec(conversation, nth).await;
        let plan = spec.reach.clone().expect("the call has a reach plan");
        assert_eq!(plan.socket, spec.runtime.call_dir.join("reach.sock"));
        (spec, FakeReach::connect(&plan.socket).await.unwrap())
    }
}

/// A job whose line blocks on `fifo`.
fn job(fifo: &Path) -> Answer {
    Some(("shell", json!({ "command": wait_on(fifo), "background": true })))
}

/// A foreground call whose line blocks on `fifo`.
fn call(fifo: &Path) -> Answer {
    Some(("shell", json!({ "command": wait_on(fifo) })))
}

/// A foreground call of `line`.
fn line(line: &str) -> Answer {
    Some(("shell", json!({ "command": line })))
}

fn wait_on(fifo: &Path) -> String {
    format!("read answer < {}", fifo.display())
}

/// Writes one line to `fifo`, which a job or a call reads; it waits for the reader.
async fn release(fifo: &Path) {
    let fifo = fifo.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
}

/// The turn of the prompt `text` with the command id `n` from `tty`, up to its own end,
/// with each question answered with `decision`; only the turn's own events.
async fn turn_on(
    daemon: &TestDaemon,
    tty: &str,
    n: u128,
    text: &str,
    decision: ApprovalDecision,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    let client = daemon.client_for_tty(tty).await.unwrap();
    let Method::PromptSend(mut params) = daemon.prompt(n, text, tty) else { unreachable!() };
    params.settings = TurnSettings { mode: Some(Mode::Auto), ..TurnSettings::default() };
    let sent: PromptSendResult = client.call(Method::PromptSend(params)).await.unwrap();
    let own = sent.turn_id;
    let mut follow = daemon.follow(&client, sent.conversation_id).await.unwrap();
    let mut seen = Vec::new();
    let mut answers = n * 100;
    loop {
        let events = events_until(&mut follow, |event| {
            event.turn_id() == Some(own)
                && matches!(
                    event,
                    Event::ApprovalRequested { .. }
                        | Event::TurnCompleted { .. }
                        | Event::TurnFailed { .. }
                        | Event::TurnInterrupted { .. }
                )
        })
        .await
        .unwrap();
        let last = events.last().unwrap().event.clone();
        seen.extend(events);
        let Event::ApprovalRequested { call_id, .. } = last else { break };
        answers += 1;
        let answer = Method::ApprovalRespond(ApprovalRespond {
            command_id: command_id(answers),
            conversation_id: sent.conversation_id,
            call_id,
            decision,
        });
        let _: ApprovalRespondResult = client.call(answer).await.unwrap();
    }
    seen.retain(|envelope| envelope.seq >= sent.seq);
    (sent, seen)
}

/// [`turn_on`] from the test's terminal, with a no to each question.
async fn turn(daemon: &TestDaemon, n: u128, text: &str) -> (PromptSendResult, Vec<EventEnvelope>) {
    turn_on(daemon, TTY, n, text, ApprovalDecision::Deny).await
}

/// The events of `conversation` from its start up to the first that `stop` accepts.
async fn until(
    daemon: &TestDaemon,
    conversation: ConversationId,
    stop: impl FnMut(&Event) -> bool,
) -> Vec<EventEnvelope> {
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    events_until(&mut follow, stop).await.unwrap()
}

/// Waits until `conversation` has `count` `job_port_forwarded` events.
async fn forwarded(daemon: &TestDaemon, conversation: ConversationId, count: usize) {
    let mut seen = 0;
    until(daemon, conversation, |event| {
        if matches!(event, Event::JobPortForwarded { .. }) {
            seen += 1;
        }
        seen == count
    })
    .await;
}

/// A daemon whose first turn started one job that forwards `port` to an echo server,
/// when `port` is some; returns the daemon, the conversation, the forwarder and the
/// job's FIFO. `answers` follow the job's turn.
async fn with_job(
    setup: &Setup,
    port: Option<u16>,
    answers: Vec<Answer>,
    builder: impl FnOnce(TestDaemonBuilder) -> TestDaemonBuilder,
) -> (TestDaemon, ConversationId, FakeForwarder, PathBuf) {
    let fifo = setup.fifo("job");
    let mut all = vec![job(&fifo), None];
    all.extend(answers);
    let daemon = builder(setup.builder(all)).start().await.unwrap();
    let (sent, events) = turn(&daemon, 1, "start the server").await;
    let started = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, .. } => Some(output.clone()),
        _ => None,
    });
    let started = started.expect("the job's start result");
    assert!(started.contains(REACH_NOTE), "the model learns of the reach (DS45): {started}");
    let conversation = sent.conversation_id;
    let mut forwarder = setup.forwarder(conversation).await;
    if let Some(port) = port {
        let echo = echo_server().await.unwrap();
        forwarder.listen(LOCALHOST, port, Some(echo)).await.unwrap();
        forwarded(&daemon, conversation, 1).await;
    }
    (daemon, conversation, forwarder, fifo)
}

/// Sends `bytes` through the call's connection to `port` and returns what came back
/// after a half close.
async fn through_reach(reach: &FakeReach, port: u16, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = reach.open(port).await?;
    stream.write_all(bytes).await?;
    stream.shutdown().await?;
    let mut back = Vec::new();
    stream.read_to_end(&mut back).await?;
    Ok(back)
}

/// True when efrd closed the call's connection to `port` with no byte.
async fn refused(reach: &FakeReach, port: u16) -> bool {
    through_reach(reach, port, b"x").await.map_or(true, |back| back.is_empty())
}

/// The launch of each tool call of `events`.
fn launches(events: &[EventEnvelope]) -> Vec<Option<Launch>> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::ToolCallStarted { launch, .. } => Some(launch.clone()),
            _ => None,
        })
        .collect()
}

fn asked(events: &[EventEnvelope]) -> bool {
    events.iter().any(|envelope| matches!(envelope.event, Event::ApprovalRequested { .. }))
}

/// Writes `config.toml` with `text` and reloads it.
async fn reload_with(setup: &Setup, daemon: &TestDaemon, text: &str) {
    crate::full::write_config(&setup.dirs, text);
    let client = daemon.client().await.unwrap();
    let reloaded: AdminConfigReloadResult =
        client.call(Method::AdminConfigReload(AdminConfigReload {})).await.unwrap();
    assert!(reloaded.applied, "{reloaded:?}");
}

#[tokio::test]
async fn reach_routes_to_own_job() {
    if !zsh_enabled("reach_routes_to_own_job") {
        return;
    }
    let setup = Setup::new();
    let call_fifo = setup.fifo("call");
    let (daemon, conversation, forwarder, job_fifo) =
        with_job(&setup, Some(8765), vec![call(&call_fifo), None], |builder| builder).await;
    let check = turn(&daemon, 2, "check the server");
    let side = async {
        let (spec, reach) = setup.reach(conversation, 0).await;
        let plan = spec.reach.unwrap();
        assert_eq!((plan.ports, plan.proxy), (vec![8765], false));
        let bytes: Vec<u8> = (0..64 * 1024).map(|n: u32| (n % 251) as u8).collect();
        assert_eq!(through_reach(&reach, 8765, &bytes).await.unwrap(), bytes, "64 KiB both ways");
        assert_eq!(through_reach(&reach, 8765, b"again").await.unwrap(), b"again");
        assert_eq!(forwarder.opens(), 2, "each connection is one open in the job");
        assert_eq!(reach.told(), [], "the helper holds the ports of the start");
        release(&call_fifo).await;
    };
    let ((_, events), ()) = tokio::join!(check, side);
    assert!(!asked(&events), "a reach asks nothing");
    assert_eq!(launches(&events), [Some(Launch::contained())]);
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn reach_refuses_other_conversation() {
    if !zsh_enabled("reach_refuses_other_conversation") {
        return;
    }
    let setup = Setup::new();
    let (other_job, call_fifo) = (setup.fifo("other-job"), setup.fifo("call"));
    let answers = vec![job(&other_job), None, call(&call_fifo), None];
    let (daemon, _, _forwarder, job_fifo) =
        with_job(&setup, Some(8765), answers, |builder| builder).await;
    // NOTE: the second conversation has a job of its own that forwards 9000, so its
    // call gets a reach; the first conversation's 8765 is not its own.
    let (sent, _) = turn_on(&daemon, OTHER_TTY, 2, "start mine", ApprovalDecision::Deny).await;
    let other = sent.conversation_id;
    let mut theirs = setup.forwarder(other).await;
    let echo = echo_server().await.unwrap();
    theirs.listen(LOCALHOST, 9000, Some(echo)).await.unwrap();
    forwarded(&daemon, other, 1).await;
    let check = turn_on(&daemon, OTHER_TTY, 3, "check", ApprovalDecision::Deny);
    let side = async {
        let (spec, reach) = setup.reach(other, 0).await;
        assert_eq!(spec.reach.unwrap().ports, [9000], "only its own conversation's ports");
        assert!(refused(&reach, 8765).await, "the port of another conversation");
        assert_eq!(through_reach(&reach, 9000, b"own").await.unwrap(), b"own");
        release(&call_fifo).await;
    };
    tokio::join!(check, side);
    release(&job_fifo).await;
    release(&other_job).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn reach_closes_after_gone_and_job_end() {
    if !zsh_enabled("reach_closes_after_gone_and_job_end") {
        return;
    }
    let setup = Setup::new();
    let call_fifo = setup.fifo("call");
    let (daemon, conversation, mut forwarder, job_fifo) =
        with_job(&setup, Some(8765), vec![call(&call_fifo), None], |builder| builder).await;
    let check = turn(&daemon, 2, "check the server");
    let side = async {
        let (_, mut reach) = setup.reach(conversation, 0).await;
        assert_eq!(through_reach(&reach, 8765, b"up").await.unwrap(), b"up");
        // NOTE: a gone server keeps its port and the call's listener (DS35); a
        // connection closes at once, as on the host.
        forwarder.gone(LOCALHOST, 8765).await.unwrap();
        Wait::new("the gone port closes connections")
            .until_some_async(async || refused(&reach, 8765).await.then_some(()))
            .await
            .unwrap();
        assert_eq!(reach.told(), [], "no unreach for a gone server");
        release(&job_fifo).await;
        let told = reach.told_at_least(2).await;
        assert_eq!(told, both(8765).map(Told::Unreach), "the job's end unreaches its port");
        assert!(refused(&reach, 8765).await);
        release(&call_fifo).await;
    };
    tokio::join!(check, side);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn reach_follows_new_forward_during_call() {
    if !zsh_enabled("reach_follows_new_forward_during_call") {
        return;
    }
    let setup = Setup::new();
    let call_fifo = setup.fifo("call");
    let (daemon, conversation, mut forwarder, job_fifo) =
        with_job(&setup, None, vec![call(&call_fifo), None], |builder| builder).await;
    let check = turn(&daemon, 2, "check the server");
    let side = async {
        let (spec, mut reach) = setup.reach(conversation, 0).await;
        assert_eq!(spec.reach.unwrap().ports, [0u16; 0], "a forwarding job with no port yet");
        assert!(refused(&reach, 8766).await, "no job forwards the port yet");
        let echo = echo_server().await.unwrap();
        forwarder.listen(LOCALHOST, 8766, Some(echo)).await.unwrap();
        let told = reach.told_at_least(2).await;
        assert_eq!(told, both(8766).map(Told::Reach));
        reach.bound(LOCALHOST, 8766, 0).await.unwrap();
        reach.bound(IpAddr::V6(Ipv6Addr::LOCALHOST), 8766, 98).await.unwrap();
        assert_eq!(through_reach(&reach, 8766, b"new").await.unwrap(), b"new");
        release(&call_fifo).await;
    };
    tokio::join!(check, side);
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn reload_off_unreaches() {
    if !zsh_enabled("reload_off_unreaches") {
        return;
    }
    let setup = Setup::new();
    let call_fifo = setup.fifo("call");
    let (daemon, conversation, _forwarder, job_fifo) =
        with_job(&setup, Some(8765), vec![call(&call_fifo), None], |builder| builder).await;
    let check = turn(&daemon, 2, "check the server");
    let side = async {
        let (_, mut reach) = setup.reach(conversation, 0).await;
        assert_eq!(through_reach(&reach, 8765, b"on").await.unwrap(), b"on");
        reload_with(&setup, &daemon, "[jobs]\nforward_ports = false\n").await;
        let told = reach.told_at_least(2).await;
        assert_eq!(told, both(8765).map(Told::Unreach));
        assert!(refused(&reach, 8765).await);
        release(&call_fifo).await;
    };
    tokio::join!(check, side);
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn timed_out_call_keeps_reach() {
    if !zsh_enabled("timed_out_call_keeps_reach") {
        return;
    }
    let setup = Setup::new();
    let call_fifo = setup.fifo("call");
    let slow = Some(("shell", json!({ "command": wait_on(&call_fifo), "timeout_seconds": 1 })));
    let answers = vec![slow, None, line("true"), None];
    let (daemon, conversation, _forwarder, job_fifo) =
        with_job(&setup, Some(8765), answers, |builder| builder).await;
    let check = turn(&daemon, 2, "check the server");
    let side = async {
        let (spec, reach) = setup.reach(conversation, 0).await;
        // NOTE: the clock moves only once the call's command runs, until the call
        // answered at its timeout; a real pause after each step lets the daemon act.
        Wait::new("the call answers at its timeout")
            .until_some_async(async || {
                let client = daemon.client().await.unwrap();
                let events = daemon.events(&client, conversation).await.unwrap();
                let done = events
                    .iter()
                    .filter(|envelope| matches!(envelope.event, Event::ToolCallCompleted { .. }));
                if done.count() == 2 {
                    return Some(());
                }
                daemon.clock().advance(Duration::from_millis(500));
                SystemClock.sleep(Duration::from_millis(20)).await;
                None
            })
            .await
            .unwrap();
        (spec, reach)
    };
    let ((_, events), (spec, mut reach)) = tokio::join!(check, side);
    assert!(!spec.runtime.call_dir.join("result.json").exists(), "the command still runs");
    assert!(!asked(&events), "{events:?}");
    assert_eq!(through_reach(&reach, 8765, b"later").await.unwrap(), b"later", "past its timeout");
    release(&call_fifo).await;
    Wait::new("the launcher wrote its result")
        .until(|| {
            let dir = &spec.runtime.call_dir;
            dir.join("result.json").exists() || !dir.exists()
        })
        .await
        .unwrap();
    // NOTE: the next call's prepare ends the reach of a call that ended since.
    turn(&daemon, 3, "again").await;
    reach.closed().await;
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn no_reach_without_forward_job() {
    if !zsh_enabled("no_reach_without_forward_job") {
        return;
    }
    let setup = Setup::new();
    let daemon = setup.builder(vec![line("true"), None]).start().await.unwrap();
    let (sent, _) = turn(&daemon, 1, "run it").await;
    let spec = setup.call_spec(sent.conversation_id, 0).await;
    assert_eq!(spec.reach, None, "a conversation with no forwarding job gets no reach");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn no_reach_for_job() {
    let setup = Setup::new();
    let second = setup.fifo("second");
    let (daemon, conversation, _forwarder, job_fifo) =
        with_job(&setup, Some(8765), vec![job(&second), None], |builder| builder).await;
    turn(&daemon, 2, "start another").await;
    let jobs: Vec<SandboxSpec> =
        setup.specs().into_iter().filter(|spec| spec.conversation == conversation).collect();
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().all(|spec| spec.job.is_some() && spec.reach.is_none()), "{jobs:?}");
    release(&job_fifo).await;
    release(&second).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn no_reach_for_exit_child() {
    if !zsh_enabled("no_reach_for_exit_child") {
        return;
    }
    let setup = Setup::new();
    let outside = json!({
        "command": "true",
        "needs": { "outside": true, "reason": "it looks at the user's processes" }
    });
    let (daemon, conversation, _forwarder, job_fifo) =
        with_job(&setup, Some(8765), vec![Some(("shell", outside)), None], |builder| builder).await;
    turn_on(&daemon, TTY, 2, "look outside", ApprovalDecision::Allow).await;
    let spec = setup.call_spec(conversation, 0).await;
    assert_eq!(spec.launch, SpecLaunch::Unsandboxed);
    assert_eq!(spec.reach, None, "the exit child is on the host's loopback already");
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

/// Sends `bytes` through `127.0.0.1:port` of the host and returns what came back after a
/// half close.
async fn through_host(port: u16, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
    stream.write_all(bytes).await?;
    stream.shutdown().await?;
    let mut back = Vec::new();
    stream.read_to_end(&mut back).await?;
    Ok(back)
}

#[tokio::test]
async fn call_limit_leaves_room_for_host() {
    if !zsh_enabled("call_limit_leaves_room_for_host") {
        return;
    }
    let setup = Setup::new();
    let call_fifo = setup.fifo("call");
    let (daemon, conversation, _forwarder, job_fifo) =
        with_job(&setup, Some(8765), vec![call(&call_fifo), None], |builder| builder).await;
    let check = turn(&daemon, 2, "open many");
    let side = async {
        let (_, reach) = setup.reach(conversation, 0).await;
        // NOTE: each connection echoes one byte before the next opens, so it holds its
        // place in efrd while the next one comes.
        let mut open = Vec::new();
        for _ in 0..efr_sandbox::MAX_REACH_CONNECTIONS {
            let mut stream = reach.open(8765).await.unwrap();
            stream.write_all(b"x").await.unwrap();
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).await.unwrap();
            open.push(stream);
        }
        assert!(refused(&reach, 8765).await, "the 33rd connection of the call");
        let real = daemon.host_ports().real_port(8765).unwrap();
        assert_eq!(through_host(real, b"host").await.unwrap(), b"host", "the host keeps room");
        drop(open);
        Wait::new("a place is free again")
            .until_some_async(async || {
                through_reach(&reach, 8765, b"y").await.ok().filter(|back| back == b"y")
            })
            .await
            .unwrap();
        release(&call_fifo).await;
    };
    tokio::join!(check, side);
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn fact_names_reach_ports() {
    if !zsh_enabled("fact_names_reach_ports") {
        return;
    }
    let setup = Setup::new();
    let answers = vec![
        line("curl -s http://localhost:8765/ http://[::1]:8765/x"),
        line("curl -s http://127.0.0.1:8766/"),
        None,
    ];
    let (daemon, conversation, _forwarder, job_fifo) =
        with_job(&setup, Some(8765), answers, |builder| builder).await;
    // NOTE: the job is no new job now, so the line of another port does not wait for a
    // forward of it (DS44).
    daemon.clock().advance(SETTLE);
    let (_, events) = turn_on(&daemon, TTY, 2, "check the server", ApprovalDecision::Allow).await;
    let [reached, other]: [Option<Launch>; 2] = launches(&events).try_into().unwrap();
    assert_eq!(reached, Some(Launch::contained()), "a forwarded port is no network use");
    let other = other.unwrap();
    assert!(other.grants().contains(&Grant::OpenNetwork), "another port: {other:?}");
    let calls: Vec<SandboxSpec> = setup
        .specs()
        .into_iter()
        .filter(|spec| spec.conversation == conversation && spec.job.is_none())
        .collect();
    let [contained] = calls.iter().filter(|spec| spec.grants.is_empty()).collect::<Vec<_>>()[..]
    else {
        panic!("{calls:?}")
    };
    assert_eq!(contained.reach.as_ref().unwrap().ports, [8765]);
    assert!(calls.iter().all(|spec| spec.grants.is_empty() || spec.reach.is_none()), "{calls:?}");
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

/// How many times efrd asked the test clock for the settle wait so far.
fn settle_waits(daemon: &TestDaemon) -> usize {
    daemon.clock().requested_sleeps().iter().filter(|sleep| **sleep == SETTLE).count()
}

#[tokio::test]
async fn fact_waits_for_a_forward_of_a_new_job() {
    if !zsh_enabled("fact_waits_for_a_forward_of_a_new_job") {
        return;
    }
    let setup = Setup::new();
    let answers = vec![line("curl -s http://127.0.0.1:8765/"), None];
    let (daemon, _, mut forwarder, job_fifo) =
        with_job(&setup, None, answers, |builder| builder).await;
    let before = settle_waits(&daemon);
    let check = turn(&daemon, 2, "check the server");
    let side = async {
        Wait::new("the fact waits for the new job's forward")
            .until(|| settle_waits(&daemon) > before)
            .await
            .unwrap();
        let echo = echo_server().await.unwrap();
        forwarder.listen(LOCALHOST, 8765, Some(echo)).await.unwrap();
    };
    let ((_, events), ()) = tokio::join!(check, side);
    assert!(!asked(&events), "the forward came during the wait");
    assert_eq!(launches(&events), [Some(Launch::contained())]);
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn fact_wait_ends_at_settle() {
    if !zsh_enabled("fact_wait_ends_at_settle") {
        return;
    }
    let setup = Setup::new();
    let answers = vec![line("curl -s http://127.0.0.1:8765/"), None];
    let (daemon, _, _forwarder, job_fifo) =
        with_job(&setup, None, answers, |builder| builder).await;
    let before = settle_waits(&daemon);
    let check = turn_on(&daemon, TTY, 2, "check the server", ApprovalDecision::Allow);
    let side = async {
        // NOTE: the clock moves only once the settle wait exists.
        Wait::new("the fact waits for the new job's forward")
            .until(|| settle_waits(&daemon) > before)
            .await
            .unwrap();
        daemon.clock().advance(SETTLE);
    };
    let ((_, events), ()) = tokio::join!(check, side);
    let [Some(launch)]: [Option<Launch>; 1] = launches(&events).try_into().unwrap() else {
        panic!("one call")
    };
    assert!(launch.grants().contains(&Grant::OpenNetwork), "no forward came: {launch:?}");
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn fact_does_not_wait_without_a_new_job() {
    if !zsh_enabled("fact_does_not_wait_without_a_new_job") {
        return;
    }
    let setup = Setup::new();
    let answers = vec![line("curl -s http://127.0.0.1:8765/"), None];
    let (daemon, _, _forwarder, job_fifo) =
        with_job(&setup, None, answers, |builder| builder).await;
    daemon.clock().advance(SETTLE);
    let before = settle_waits(&daemon);
    let (_, events) = turn_on(&daemon, TTY, 2, "check the server", ApprovalDecision::Allow).await;
    assert_eq!(settle_waits(&daemon), before, "the job began to forward 3 s ago");
    let [Some(launch)]: [Option<Launch>; 1] = launches(&events).try_into().unwrap() else {
        panic!("one call")
    };
    assert!(launch.grants().contains(&Grant::OpenNetwork), "{launch:?}");
    release(&job_fifo).await;
    daemon.stop().await.unwrap();
}
