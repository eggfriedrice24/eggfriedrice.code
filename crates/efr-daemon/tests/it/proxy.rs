//! The network proxy (phase D2) through efrd with the shared fake launcher of
//! `efr-test-daemon`: background jobs (so no zsh is needed) with `network.mode =
//! "proxy"` and a probe whose proxy self-test passed. The fake launcher keeps the spec
//! of each call, which names the call's proxy socket; the test connects to that socket
//! itself in place of a bridge. The proxy runs with the test seams: no name resolves
//! unless a test maps it, so a host that a call may reach gets 502 and a host that it
//! may not reach gets 403, and no test reaches the network. A job that must run while
//! the test connects blocks on a FIFO that the test writes; no test waits on a fixed
//! time.

use std::collections::BTreeMap;
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};

use efr_protocol::{
    AdminConfigReload, AdminConfigReloadResult, ApprovalDecision, BlockReason, ConversationId,
    Event, EventEnvelope, Method, Mode, NetworkMode, SandboxExplain, SandboxExplainResult,
    SandboxStatus,
};
use efr_sandbox::{NetworkPlan, SandboxSpec};
use efr_test_daemon::{FakeLauncher, TestDaemon, events_until};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;

use crate::sandbox::{REVIEWER_ALLOWS, kinds, ready, run_turn};

/// A model that calls `shell` with each step's input in turn and ends its turn at each
/// step that has none; the reviewer allows every exit.
#[derive(Debug)]
struct StepModel {
    id: ProviderId,
    steps: Vec<Option<Value>>,
    asked: AtomicUsize,
}

impl StepModel {
    fn new(steps: Vec<Option<Value>>) -> Arc<StepModel> {
        Arc::new(StepModel {
            id: ProviderId::new("test").unwrap(),
            steps,
            asked: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Provider for StepModel {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, request: Request) -> Result<ProviderStream, ProviderError> {
        let done = |text: &str| {
            vec![
                ProviderEvent::TextDelta { text: text.to_owned() },
                ProviderEvent::Done { stop_reason: StopReason::EndTurn, provider_raw: None },
            ]
        };
        let events = if request.one_shot {
            done(REVIEWER_ALLOWS)
        } else {
            let n = self.asked.fetch_add(1, Ordering::SeqCst);
            match self.steps.get(n) {
                Some(Some(input)) => {
                    let call_id = format!("call_{n}");
                    vec![
                        ProviderEvent::ToolCallStart {
                            call_id: call_id.clone(),
                            name: "shell".to_owned(),
                            freeform: false,
                        },
                        ProviderEvent::ToolCallEnd { call_id, arguments: input.to_string() },
                        ProviderEvent::Done {
                            stop_reason: StopReason::ToolUse,
                            provider_raw: None,
                        },
                    ]
                }
                _ => done("done"),
            }
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// A ready sandbox whose proxy self-test passed.
fn proxy_ready() -> SandboxStatus {
    SandboxStatus { proxy: true, ..ready() }
}

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

    /// A daemon with the proxy on and the model `steps` (`None` ends a turn), whose
    /// reviewer allows every exit.
    async fn daemon(&self, steps: Vec<Option<Value>>) -> TestDaemon {
        self.daemon_with(steps, NetworkMode::Proxy, proxy_ready()).await
    }

    async fn daemon_with(
        &self,
        steps: Vec<Option<Value>>,
        mode: NetworkMode,
        status: SandboxStatus,
    ) -> TestDaemon {
        let env = BTreeMap::from([
            ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), self.dirs.home().display().to_string()),
        ]);
        let model = StepModel::new(steps);
        TestDaemon::builder()
            .dirs(Arc::clone(&self.dirs))
            .sandbox_launcher(self.fake.path())
            .probe_override(status)
            .network_mode(mode)
            .job_env(env)
            .custom_provider(model as Arc<dyn Provider>)
            .start()
            .await
            .unwrap()
    }

    /// A FIFO below the test's root, which a job reads and the test writes.
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

    /// The proxy socket of each spec that the fake launcher kept, in call order.
    fn sockets(&self) -> Vec<Option<PathBuf>> {
        self.specs().iter().map(socket_of).collect()
    }
}

/// The proxy socket of `spec`.
fn socket_of(spec: &SandboxSpec) -> Option<PathBuf> {
    match &spec.network {
        NetworkPlan::Proxy { socket } => Some(socket.clone()),
        _ => None,
    }
}

/// A background job that runs `command`, with `hosts` as its `needs.hosts`.
fn job(command: &str, hosts: &[&str]) -> Value {
    let mut input = json!({ "command": command, "background": true, "max_minutes": 1 });
    if !hosts.is_empty() {
        input["needs"] = json!({ "hosts": hosts });
    }
    input
}

/// A job line that waits until the test writes `fifo`.
fn wait_on(fifo: &Path) -> String {
    format!("cat '{}' > /dev/null", fifo.display())
}

/// Writes one line to `fifo`, which a job reads; it waits for the reader.
async fn release(fifo: &Path) {
    let fifo = fifo.to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
}

/// Connects to `socket` through its dir's descriptor: a daemon test's call dirs make
/// the path longer than a socket address holds.
async fn connect(socket: &Path) -> std::io::Result<UnixStream> {
    let dir = std::fs::File::open(socket.parent().unwrap())?;
    let name = socket.file_name().unwrap().to_string_lossy();
    UnixStream::connect(format!("/proc/self/fd/{}/{name}", dir.as_raw_fd())).await
}

/// The status line of the proxy's answer to `CONNECT host:443`.
async fn status(socket: &Path, host: &str) -> String {
    let mut stream = connect(socket).await.unwrap();
    let head = format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let mut byte = [0_u8; 1];
    while !answer.ends_with(b"\r\n") && stream.read(&mut byte).await.unwrap() == 1 {
        answer.push(byte[0]);
    }
    String::from_utf8_lossy(&answer).trim_end().to_owned()
}

/// The events of `conversation` up to the first that `stop` takes.
async fn until(
    daemon: &TestDaemon,
    conversation: ConversationId,
    stop: impl FnMut(&Event) -> bool,
) -> Vec<EventEnvelope> {
    let client = daemon.client().await.unwrap();
    let mut follow = daemon.follow(&client, conversation).await.unwrap();
    events_until(&mut follow, stop).await.unwrap()
}

/// The `job_ended` of the job `id` of `conversation`, once it comes.
async fn ended(daemon: &TestDaemon, conversation: ConversationId, id: &str) -> Event {
    let events = until(
        daemon,
        conversation,
        |event| matches!(event, Event::JobEnded { job_id, .. } if job_id.as_str() == id),
    )
    .await;
    events.last().unwrap().event.clone()
}

#[tokio::test]
async fn proxy_grant_is_per_call() {
    let setup = Setup::new();
    let (one, two) = (setup.fifo("one"), setup.fifo("two"));
    let steps = vec![Some(job(&wait_on(&one), &["extra.test"])), Some(job(&wait_on(&two), &[]))];
    let daemon = setup.daemon(steps).await;
    let (sent, events) =
        run_turn(&daemon, "fetch from extra.test in two jobs", Mode::Auto, ApprovalDecision::Deny)
            .await;
    assert!(kinds(&events).contains(&"exit_requested"), "{:?}", kinds(&events));
    let specs = setup.specs();
    let host = |spec: &&SandboxSpec| {
        spec.grants.iter().any(|grant| matches!(grant, efr_protocol::Grant::Host { .. }))
    };
    let granted = specs.iter().find(host).and_then(socket_of).unwrap();
    let other = specs.iter().find(|spec| !host(spec)).and_then(socket_of).unwrap();
    let (granted, other) = (&granted, &other);
    // NOTE: the test seams resolve no name: a host that the call may reach gets 502.
    let answer = status(granted, "extra.test").await;
    assert!(answer.contains(" 502 "), "the grant of the first job: {answer}");
    let answer = status(other, "extra.test").await;
    assert!(answer.contains(" 403 "), "no grant in the second job: {answer}");
    release(&one).await;
    release(&two).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    ended(&daemon, sent.conversation_id, "j2").await;
    for socket in [granted, other] {
        assert!(connect(socket).await.is_err(), "the socket of an ended job is gone");
    }
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_socket_lives_as_long_as_the_job() {
    let setup = Setup::new();
    let fifo = setup.fifo("hold");
    let daemon = setup.daemon(vec![Some(job(&wait_on(&fifo), &[]))]).await;
    let (sent, _) = run_turn(&daemon, "wait in a job", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets();
    let [Some(socket)] = sockets.as_slice() else { panic!("{sockets:?}") };
    assert!(
        socket.as_os_str().len() > 107,
        "a daemon test's call dir is deep: {}",
        socket.display()
    );
    let mode = std::fs::metadata(socket).unwrap();
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o777, 0o600);
    assert!(connect(socket).await.is_ok(), "the job's socket serves while it runs");
    assert!(connect(socket).await.is_ok(), "and again");
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    // NOTE: efrd records the end, then removes the job's dirs.
    Wait::new("the job's call dir to go with its socket").until(|| !socket.exists()).await.unwrap();
    assert!(connect(socket).await.is_err());
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_denial_reaches_job_ended() {
    let setup = Setup::new();
    let fifo = setup.fifo("hold");
    let daemon = setup.daemon(vec![Some(job(&wait_on(&fifo), &[]))]).await;
    let (sent, _) = run_turn(&daemon, "wait in a job", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets();
    let [Some(socket)] = sockets.as_slice() else { panic!("{sockets:?}") };
    assert!(status(socket, "extra.test").await.contains(" 403 "));
    assert!(status(socket, "extra.test").await.contains(" 403 "));
    release(&fifo).await;
    let Event::JobEnded { blocked, blocked_more, .. } =
        ended(&daemon, sent.conversation_id, "j1").await
    else {
        unreachable!()
    };
    let [entry] = blocked.as_slice() else { panic!("{blocked:?}") };
    assert_eq!(
        (entry.host.as_str(), entry.port, entry.reason),
        ("extra.test", 443, BlockReason::NotAllowed)
    );
    assert_eq!(entry.count, 2, "one record per host, port and reason");
    assert_eq!(blocked_more, 0);
    let events =
        until(&daemon, sent.conversation_id, |event| matches!(event, Event::JobNotice { .. }))
            .await;
    let Some(Event::JobNotice { text, .. }) = events.last().map(|envelope| &envelope.event) else {
        unreachable!()
    };
    assert!(text.contains("refused extra.test:443"), "{text}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn refused_host_is_a_fact_of_the_next_exit() {
    let setup = Setup::new();
    let fifo = setup.fifo("hold");
    let steps = vec![Some(job(&wait_on(&fifo), &[])), None, Some(job("true", &["extra.test"]))];
    let daemon = setup.daemon(steps).await;
    let (sent, _) = run_turn(&daemon, "wait in a job", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets();
    let [Some(socket)] = sockets.as_slice() else { panic!("{sockets:?}") };
    assert!(status(socket, "extra.test").await.contains(" 403 "));
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    // NOTE: the job's end wakes the model, whose next call names the refused host.
    let events =
        until(&daemon, sent.conversation_id, |event| matches!(event, Event::ExitRequested { .. }))
            .await;
    let Some(Event::ExitRequested { record, .. }) = events.last().map(|envelope| &envelope.event)
    else {
        unreachable!()
    };
    let host = record.facts.hosts.iter().find(|host| host.host == "extra.test");
    let host = host.unwrap_or_else(|| panic!("{:?}", record.facts.hosts));
    assert!(host.refused_by_proxy, "the proxy refused it in the job: {host:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn network_mode_none_keeps_phase1() {
    let setup = Setup::new();
    // NOTE: the line must not reach the host's network, so the fake fails its setup.
    setup.fake.set("fail").unwrap();
    let steps = vec![Some(job("curl https://extra.test/", &[]))];
    let daemon = setup.daemon_with(steps, NetworkMode::None, proxy_ready()).await;
    let (_, events) =
        run_turn(&daemon, "fetch extra.test", Mode::Auto, ApprovalDecision::Deny).await;
    assert!(kinds(&events).contains(&"exit_requested"), "{:?}", kinds(&events));
    let specs = setup.specs();
    let [spec] = specs.as_slice() else { panic!("{specs:?}") };
    assert_eq!(spec.network, NetworkPlan::Open, "a review opens the whole network");
    assert_eq!(spec.grants, [efr_protocol::Grant::OpenNetwork]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn failed_proxy_self_test_falls_back_to_none() {
    let setup = Setup::new();
    let daemon = setup.daemon_with(vec![Some(job("true", &[]))], NetworkMode::Proxy, ready()).await;
    let (sent, events) = run_turn(&daemon, "run a job", Mode::Auto, ApprovalDecision::Deny).await;
    assert!(!kinds(&events).contains(&"exit_requested"), "{:?}", kinds(&events));
    let specs = setup.specs();
    let [spec] = specs.as_slice() else { panic!("{specs:?}") };
    assert_eq!(spec.network, NetworkPlan::None, "never a route-less proxy");
    let status = admin_status(&daemon).await;
    assert_eq!(status.network_mode, NetworkMode::None);
    assert!(
        status.warnings.iter().any(|warning| warning.contains("self-test failed")),
        "{:?}",
        status.warnings
    );
    let output = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, .. } => Some(output.clone()),
        _ => None,
    });
    assert!(output.is_some_and(|output| output.contains("proxy is off")), "the fallback note");
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

/// The sandbox status of `admin.status`.
async fn admin_status(daemon: &TestDaemon) -> SandboxStatus {
    let client = daemon.client().await.unwrap();
    let status: efr_protocol::AdminStatusResult =
        client.call(Method::AdminStatus(efr_protocol::AdminStatus::default())).await.unwrap();
    status.sandbox.unwrap()
}

#[tokio::test]
async fn status_reports_the_proxy() {
    let setup = Setup::new();
    let daemon = setup.daemon(Vec::new()).await;
    assert_eq!(admin_status(&daemon).await.network_mode, NetworkMode::Proxy);
    daemon.stop().await.unwrap();
}

/// Writes `config.toml` with `text` and reloads it.
async fn reload(daemon: &TestDaemon, dirs: &TestDirs, text: &str) {
    std::fs::write(dirs.dirs().config().join("config.toml"), text).unwrap();
    let client = daemon.client().await.unwrap();
    let reloaded: AdminConfigReloadResult =
        client.call(Method::AdminConfigReload(AdminConfigReload::default())).await.unwrap();
    assert!(reloaded.applied, "{reloaded:?}");
}

#[tokio::test]
async fn reload_sends_engine_on_allow_change() {
    let setup = Setup::new();
    let fifo = setup.fifo("hold");
    let daemon = setup.daemon(vec![Some(job(&wait_on(&fifo), &[]))]).await;
    let (sent, _) = run_turn(&daemon, "wait in a job", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets();
    let [Some(socket)] = sockets.as_slice() else { panic!("{sockets:?}") };
    assert!(status(socket, "late.test").await.contains(" 403 "));
    reload(&daemon, &setup.dirs, "[network]\nmode = \"proxy\"\nallow = [\"late.test\"]\n").await;
    let answer = status(socket, "late.test").await;
    assert!(answer.contains(" 502 "), "the new list reaches the running job: {answer}");
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn mode_change_keeps_running_job_listener() {
    let setup = Setup::new();
    let fifo = setup.fifo("hold");
    let steps = vec![Some(job(&wait_on(&fifo), &[])), None, Some(job("true", &[]))];
    let daemon = setup.daemon(steps).await;
    let (sent, _) = run_turn(&daemon, "wait in a job", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets();
    let [Some(socket)] = sockets.as_slice() else { panic!("{sockets:?}") };
    reload(&daemon, &setup.dirs, "[network]\nmode = \"none\"\n").await;
    assert!(status(socket, "extra.test").await.contains(" 403 "), "the job keeps its network");
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    // NOTE: the end of the first job wakes the model, whose next job starts without the
    // proxy.
    ended(&daemon, sent.conversation_id, "j2").await;
    let sockets = setup.sockets();
    assert_eq!(sockets.len(), 2, "{sockets:?}");
    let proxied = sockets.iter().filter(|socket| socket.is_some()).count();
    assert_eq!(proxied, 1, "a new call after the change has no proxy: {sockets:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn explain_works_with_network_mode_proxy() {
    let setup = Setup::new();
    let daemon = setup.daemon(Vec::new()).await;
    let client = daemon.client().await.unwrap();
    let path = setup.dirs.home().join("notes.txt");
    let explained: SandboxExplainResult = client
        .call(Method::SandboxExplain(SandboxExplain { path: path.clone(), cwd: None }))
        .await
        .unwrap();
    assert_eq!(explained.path, path);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_test_hosts_reach_their_server() {
    let setup = Setup::new();
    let server = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let fifo = setup.fifo("hold");
    let env = BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    let model = StepModel::new(vec![Some(job(&wait_on(&fifo), &[]))]);
    let daemon = TestDaemon::builder()
        .dirs(Arc::clone(&setup.dirs))
        .sandbox_launcher(setup.fake.path())
        .probe_override(proxy_ready())
        .network_mode(NetworkMode::Proxy)
        .proxy_hosts(BTreeMap::from([("registry.test".to_owned(), address)]))
        .config(|settings| settings.network.allow = vec!["registry.test".to_owned()])
        .job_env(env)
        .custom_provider(model as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();
    let (sent, _) = run_turn(&daemon, "wait in a job", Mode::Auto, ApprovalDecision::Deny).await;
    let sockets = setup.sockets();
    let [Some(socket)] = sockets.as_slice() else { panic!("{sockets:?}") };
    let accepted = tokio::spawn(async move { server.accept().await.map(|(_, from)| from) });
    let answer = status(socket, "registry.test").await;
    assert!(answer.contains(" 200 "), "the table maps the name to its server: {answer}");
    assert!(accepted.await.unwrap().is_ok(), "the server got the connection");
    assert!(status(socket, "other.test").await.contains(" 403 "));
    release(&fifo).await;
    ended(&daemon, sent.conversation_id, "j1").await;
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_proxy_denial_reaches_the_tool_result() {
    if !crate::support::zsh_enabled("shell_proxy_denial_reaches_the_tool_result") {
        return;
    }
    let setup = Setup::new();
    let fifo = setup.fifo("hold");
    let model = StepModel::new(vec![Some(json!({ "command": wait_on(&fifo) }))]);
    let daemon = TestDaemon::builder()
        .local_pty()
        .dirs(Arc::clone(&setup.dirs))
        .sandbox_launcher(setup.fake.path())
        .probe_override(proxy_ready())
        .network_mode(NetworkMode::Proxy)
        .custom_provider(model as Arc<dyn Provider>)
        .start()
        .await
        .unwrap();
    let turn = run_turn(&daemon, "wait for the fifo", Mode::Auto, ApprovalDecision::Deny);
    let client_side = async {
        let socket = Wait::new("the spec of the foreground call")
            .until_some(|| setup.sockets().into_iter().flatten().next())
            .await
            .unwrap();
        let answer = status(&socket, "extra.test").await;
        release(&fifo).await;
        answer
    };
    let ((_, events), answer) = tokio::join!(turn, client_side);
    assert!(answer.contains(" 403 "), "{answer}");
    let completed = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, sandbox, .. } => Some((output.clone(), sandbox.clone())),
        _ => None,
    });
    let (output, summary) = completed.unwrap();
    let summary = summary.unwrap();
    assert!(summary.proxied, "the call ran with a proxy plan");
    let [entry] = summary.blocked.as_slice() else { panic!("{:?}", summary.blocked) };
    assert_eq!((entry.host.as_str(), entry.reason), ("extra.test", BlockReason::NotAllowed));
    assert!(output.contains("refused extra.test:443: not on the allow list"), "{output}");
    daemon.stop().await.unwrap();
}
