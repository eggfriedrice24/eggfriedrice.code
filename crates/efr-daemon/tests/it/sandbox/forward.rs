//! The port forwards of background jobs (v0.0.4 live items, DL1-DL18) end to end: the
//! model's call, efrd's engine, its job table and its host side, the real `efr-sbx` of
//! `just test-sandbox` (`EFR_TEST_SBX_BIN`) with its forwarder in the job, and a real
//! hidden zsh (`EFR_TEST_ZSH=1`). Here the real forwarder meets the real efrd; the
//! tests of `efr-sbx` (`TestForward`) and of `jobs_forward` (`FakeForwarder`) test each
//! side alone.
//!
//! The dev server of a job is the launcher's own `self-test --echo` on a fixed port of
//! the job's own loopback, in its own network namespace, so no test binds a fixed port
//! of the machine. efrd binds each forwarded port through the test seam, which takes a
//! free port of `127.0.0.1` in its place (`TestDaemon::host_ports`). The ports stay
//! below 32768, so the forwarder counts them after its short delay, not after the delay
//! of the ephemeral range. efrd's forward plan keeps its default timings: a listen
//! shows after about one second. A test waits for an event or a state, never for a
//! fixed time.
//!
//! The tests say `skipped:` without the launcher or zsh, or when the probe says that
//! the sandbox is unavailable; `just test-sandbox` fails on such a line on a ready
//! machine. Their dirs stay below the target dir, not below /tmp, which the sandbox
//! replaces with its own.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    AdminProjectAdd, AdminProjectAddResult, AdminStatus, AdminStatusResult, ApprovalDecision,
    ConversationId, Event, EventEnvelope, ForwardOutcome, JobId, JobInfo, JobState, JobsList,
    JobsListResult, JobsStop, JobsStopResult, Launch, Method, Mode, NetworkMode, PortForward,
    PromptSendResult,
};
use efr_test_daemon::{TTY, TestDaemon, command_id, events_until};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::netview::target_dirs;
use super::{real_launcher, skip, with_zsh};
use crate::jobs_wake::{Answer, Recorder, turn};
use crate::support::zsh_enabled;

/// The port of the job's IPv4 server, in the job's own network namespace.
const V4_PORT: u16 = 20080;

/// The port of the job's IPv6 server, in the job's own network namespace.
const V6_PORT: u16 = 20081;

/// The process name of the forwarder in a job.
const FORWARDER: &str = "efr-forward";

/// A background `shell` call that runs `command`.
fn job(command: &str) -> Answer {
    Some(("shell", json!({ "command": command, "background": true })))
}

/// The line of a dev server in a job: the launcher inside the sandbox echoes each
/// connection on `address` (quoted: zsh reads `[::1]` as a pattern).
fn echo_server(address: &str) -> String {
    format!("\"$_efr_child_dir/efr-sbx\" self-test --echo '{address}'")
}

/// The daemon of a test: the real launcher, the probe's own result, a real hidden zsh
/// and `network.mode` set to `network`, in its settings and in `config.toml`, so the
/// reload of the project's registration keeps it. Its working directory,
/// `<home>/project`, is a registered project. A `persistent` daemon keeps its database
/// over a restart. `None` after a `skipped:` line.
async fn real_daemon(
    test: &str,
    dirs: &Arc<TestDirs>,
    model: Arc<Recorder>,
    network: NetworkMode,
    persistent: bool,
) -> Option<TestDaemon> {
    if !zsh_enabled(test) {
        return None;
    }
    let launcher = real_launcher(test)?;
    let project = dirs.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let user_runtime = dirs.create_dir("xrt").unwrap();
    let env = BTreeMap::from([
        ("HOME".to_owned(), dirs.home().display().to_string()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("XDG_RUNTIME_DIR".to_owned(), user_runtime.display().to_string()),
    ]);
    let mode = match network {
        NetworkMode::Proxy => "proxy",
        _ => "none",
    };
    // NOTE: a job notice would start a wake turn when a job ends; no test here needs
    // one.
    crate::full::write_config(
        dirs,
        &format!(
            "[permissions]\nmode = \"auto\"\n\n[network]\nmode = \"{mode}\"\n\n\
             [jobs]\nmax_wakes_per_hour = 0\n"
        ),
    );
    // NOTE: the clock starts after the files of the test were written, so no file of
    // the test counts as a program that changed in the turn (which asks the user).
    let start = jiff::Timestamp::now().checked_add(jiff::SignedDuration::from_mins(1)).unwrap();
    let mut builder = with_zsh(TestDaemon::builder())
        .clock(efr_test_support::TestClock::starting_at(start))
        .dirs(Arc::clone(dirs))
        .shell_env(env)
        .sandbox_launcher(&launcher)
        .network_mode(network)
        .config(|config| {
            config.permissions.mode = Mode::Auto;
            config.jobs.max_wakes_per_hour = 0;
        })
        .custom_provider(model);
    if persistent {
        builder = builder.persistent();
    }
    let daemon = builder.start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let status: AdminStatusResult = client.call(Method::AdminStatus(AdminStatus {})).await.unwrap();
    let sandbox = status.sandbox.unwrap();
    if !sandbox.available {
        // NOTE: just test-sandbox fails when this says skipped on a ready machine.
        skip(test, &format!("the probe says the sandbox is unavailable: {:?}", sandbox.reason));
        drop(client);
        daemon.stop().await.unwrap();
        return None;
    }
    if network == NetworkMode::Proxy {
        assert!(sandbox.proxy, "the proxy self-test failed: {:?}", sandbox.warnings);
    }
    let add =
        Method::AdminProjectAdd(AdminProjectAdd { path: project, name: None, git_root: false });
    let _: AdminProjectAddResult = client.call(add).await.unwrap();
    drop(client);
    Some(daemon)
}

/// The turn of `text` in `auto`; no question comes.
async fn auto_turn(
    daemon: &TestDaemon,
    n: u128,
    text: &str,
) -> (PromptSendResult, Vec<EventEnvelope>) {
    let (sent, events) = turn(daemon, n, text, Mode::Auto, ApprovalDecision::Deny).await;
    assert!(
        !events.iter().any(|envelope| matches!(envelope.event, Event::ApprovalRequested { .. })),
        "nobody was asked"
    );
    (sent, events)
}

/// The one job that `events` started; it runs contained.
fn one_job(events: &[EventEnvelope]) -> JobInfo {
    let jobs: Vec<JobInfo> = events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobStarted { job, .. } => Some(job.clone()),
            _ => None,
        })
        .collect();
    let [job] = jobs.try_into().unwrap_or_else(|jobs| panic!("{jobs:?}"));
    assert!(matches!(job.launch, Launch::Contained { .. }), "{:?}", job.launch);
    job
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

/// Each `job_port_forwarded` of `events`: the job, the address, the port and the
/// outcome.
fn forwards(events: &[EventEnvelope]) -> Vec<(String, String, u16, ForwardOutcome)> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.event {
            Event::JobPortForwarded { job_id, listen, port, outcome, .. } => {
                Some((job_id.to_string(), listen.clone(), *port, *outcome))
            }
            _ => None,
        })
        .collect()
}

/// The first `job_port_forwarded` of `conversation` for `port`, once it came.
async fn forwarded(
    daemon: &TestDaemon,
    conversation: ConversationId,
    port: u16,
) -> (String, String, u16, ForwardOutcome) {
    let events = until(
        daemon,
        conversation,
        |event| matches!(event, Event::JobPortForwarded { port: p, .. } if *p == port),
    )
    .await;
    forwards(&events).pop().unwrap()
}

/// The end of the job `id` of `conversation`, once it came: its state.
async fn ended(daemon: &TestDaemon, conversation: ConversationId, id: &str) -> JobState {
    let events = until(
        daemon,
        conversation,
        |event| matches!(event, Event::JobEnded { job_id, .. } if job_id.as_str() == id),
    )
    .await;
    match &events.last().unwrap().event {
        Event::JobEnded { state, .. } => *state,
        _ => unreachable!(),
    }
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

/// Sends `bytes` through `127.0.0.1:port` and returns what came back after a half close.
async fn through(port: u16, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
    stream.write_all(bytes).await?;
    stream.shutdown().await?;
    let mut back = Vec::new();
    stream.read_to_end(&mut back).await?;
    Ok(back)
}

/// 64 KiB of bytes that are not all the same.
fn payload() -> Vec<u8> {
    (0..64 * 1024).map(|n: u32| (n % 251) as u8).collect()
}

/// True when nothing listens on `127.0.0.1:port`.
fn refused(port: u16) -> bool {
    std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
}

/// The command name and the parent of the process `pid`, `None` when it is gone.
fn name_and_parent(pid: u32) -> Option<(String, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let name = stat[stat.find('(')? + 1..stat.rfind(')')?].to_owned();
    let parent = stat[stat.rfind(')')? + 1..].split_whitespace().nth(1)?.parse().ok()?;
    Some((name, parent))
}

/// The forwarders below this test's process, which runs efrd, its hidden shells and
/// its jobs. A forwarder of another test's process is never below it.
fn forwarders() -> Vec<u32> {
    let own = std::process::id();
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| name_and_parent(*pid).is_some_and(|(name, _)| name == FORWARDER))
        .filter(|pid| {
            let mut next = *pid;
            // NOTE: a bound on the walk, as a process can end while the test reads.
            for _ in 0..64 {
                match name_and_parent(next) {
                    Some((_, parent)) if parent == own => return true,
                    Some((_, parent)) if parent > 1 => next = parent,
                    _ => return false,
                }
            }
            false
        })
        .collect()
}

/// Every file named `forward.sock` below `dir`.
fn forward_sockets(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(next) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                todo.push(entry.path());
            } else if entry.file_name() == efr_sandbox::FORWARD_SOCKET_FILE {
                found.push(entry.path());
            }
        }
    }
    found
}

/// Starts the job of `line` in `daemon` and waits for the forward of `port`: the
/// conversation and the free port that stands for `port` on the host.
async fn forwarded_job(
    daemon: &TestDaemon,
    line: &str,
    port: u16,
    listen: &str,
) -> (ConversationId, u16) {
    let (sent, events) = auto_turn(daemon, 1, &format!("start a dev server: {line}")).await;
    let info = one_job(&events);
    let conversation = sent.conversation_id;
    let seen = forwarded(daemon, conversation, port).await;
    assert_eq!(
        seen,
        (info.id.to_string(), listen.to_owned(), port, ForwardOutcome::Forwarded),
        "{}",
        std::fs::read_to_string(&info.output).unwrap_or_default()
    );
    let real = daemon.host_ports().real_port(port).expect("efrd bound the port");
    (conversation, real)
}

#[tokio::test]
async fn shell_dev_server_job_is_reachable_from_the_host() {
    let test = "shell_dev_server_job_is_reachable_from_the_host";
    let dirs = target_dirs();
    let line = echo_server(&format!("127.0.0.1:{V4_PORT}"));
    let model = Recorder::new(vec![job(&line), None]);
    let Some(daemon) = real_daemon(test, &dirs, model, NetworkMode::None, false).await else {
        return;
    };
    let (conversation, real) = forwarded_job(&daemon, &line, V4_PORT, "127.0.0.1").await;
    let bytes = payload();
    assert_eq!(through(real, &bytes).await.unwrap(), bytes, "64 KiB both ways, then a half close");
    assert_eq!(through(real, b"again").await.unwrap(), b"again");
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!(job.state, JobState::Running);
    assert_eq!(job.forwards, [PortForward { listen: "127.0.0.1".to_owned(), port: V4_PORT }]);
    assert_eq!(forwarders().len(), 1, "the job's forwarder runs below efrd's launcher");
    let sockets = forward_sockets(dirs.dirs().runtime());
    assert_eq!(sockets.len(), 1, "the job's call dir holds its forward socket: {sockets:?}");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_v6_only_server_is_reachable() {
    let test = "shell_v6_only_server_is_reachable";
    let dirs = target_dirs();
    let line = echo_server(&format!("[::1]:{V6_PORT}"));
    let model = Recorder::new(vec![job(&line), None]);
    let Some(daemon) = real_daemon(test, &dirs, model, NetworkMode::None, false).await else {
        return;
    };
    // NOTE: efrd listens on the host's 127.0.0.1 only; the forwarder connects to the
    // job's ::1.
    let (conversation, real) = forwarded_job(&daemon, &line, V6_PORT, "::1").await;
    assert_eq!(through(real, b"over v6").await.unwrap(), b"over v6");
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!(job.forwards, [PortForward { listen: "::1".to_owned(), port: V6_PORT }]);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_proxy_job_and_none_job_both_forward() {
    let test = "shell_proxy_job_and_none_job_both_forward";
    for network in [NetworkMode::None, NetworkMode::Proxy] {
        let dirs = target_dirs();
        let line = echo_server(&format!("127.0.0.1:{V4_PORT}"));
        let model = Recorder::new(vec![job(&line), None]);
        let Some(daemon) = real_daemon(test, &dirs, model, network, false).await else {
            return;
        };
        let (conversation, real) = forwarded_job(&daemon, &line, V4_PORT, "127.0.0.1").await;
        assert_eq!(through(real, b"hello").await.unwrap(), b"hello", "{network:?}");
        // NOTE: the bridge of a proxy job listens on the job's 3128 from its start, more
        // than a poll before the job's own server: the forwarder never reports it.
        let client = daemon.client().await.unwrap();
        let stored = daemon.events(&client, conversation).await.unwrap();
        assert_eq!(
            forwards(&stored),
            [("j1".to_owned(), "127.0.0.1".to_owned(), V4_PORT, ForwardOutcome::Forwarded)],
            "{network:?}"
        );
        drop(client);
        daemon.stop().await.unwrap();
    }
}

#[tokio::test]
async fn shell_job_stop_frees_the_port() {
    let test = "shell_job_stop_frees_the_port";
    let dirs = target_dirs();
    let line = echo_server(&format!("127.0.0.1:{V4_PORT}"));
    let model = Recorder::new(vec![job(&line), None]);
    let Some(daemon) = real_daemon(test, &dirs, model, NetworkMode::None, false).await else {
        return;
    };
    let (conversation, real) = forwarded_job(&daemon, &line, V4_PORT, "127.0.0.1").await;
    assert_eq!(through(real, b"before").await.unwrap(), b"before");
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let stop = Method::JobsStop(JobsStop {
        command_id: command_id(60),
        conversation_id: Some(conversation),
        job_id: JobId::nth(1),
    });
    let stopped: JobsStopResult = client.call(stop).await.unwrap();
    assert_eq!(stopped.job.state, JobState::Stopped);
    assert_eq!(ended(&daemon, conversation, "j1").await, JobState::Stopped);
    Wait::new("the host port is free").until(|| refused(real)).await.unwrap();
    Wait::new("the forwarder ended").until(|| forwarders().is_empty()).await.unwrap();
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!((job.state, job.forwards), (JobState::Stopped, Vec::new()));
    drop(client);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_restart_leaves_no_listener() {
    let test = "shell_restart_leaves_no_listener";
    let dirs = target_dirs();
    let line = echo_server(&format!("127.0.0.1:{V4_PORT}"));
    let model = Recorder::new(vec![job(&line), None]);
    let Some(mut daemon) = real_daemon(test, &dirs, model, NetworkMode::None, true).await else {
        return;
    };
    let (conversation, real) = forwarded_job(&daemon, &line, V4_PORT, "127.0.0.1").await;
    daemon.restart().await.unwrap();
    assert!(refused(real), "the listener closed with the old efrd");
    assert_eq!(daemon.host_ports().real_port(V4_PORT), None, "the new efrd bound nothing");
    Wait::new("the forwarder ended").until(|| forwarders().is_empty()).await.unwrap();
    let [job] = jobs(&daemon, conversation).await.try_into().unwrap();
    assert_eq!((job.state, job.forwards), (JobState::Stopped, Vec::new()), "{:?}", job.stopped_by);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_foreground_call_gets_no_forwarder() {
    let test = "shell_foreground_call_gets_no_forwarder";
    let dirs = target_dirs();
    let gate = dirs.create_dir("home/gate").unwrap();
    let fifo = gate.join("go");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
    // NOTE: the call listens on its own loopback for a short time, marks that it runs,
    // and waits for the test, which looks for a forwarder and a forward socket
    // meanwhile.
    let line = format!(
        "{} --echo-ms 1000 & touch started; read answer < ~/gate/go; \
         if [ -e \"${{_efr_child_dir:h}}/efr-forward.sock\" ]; then echo socket-seen; \
         else echo socket-absent; fi; wait",
        echo_server(&format!("127.0.0.1:{V4_PORT}"))
    );
    let model = Recorder::new(vec![Some(("shell", json!({ "command": line }))), None]);
    let Some(daemon) = real_daemon(test, &dirs, model, NetworkMode::None, false).await else {
        return;
    };
    let marker = daemon.cwd().join("started");
    let runtime = dirs.dirs().runtime().to_path_buf();
    let look = async {
        Wait::new("the call runs").until(|| marker.exists()).await.unwrap();
        let seen = (forwarders(), forward_sockets(&runtime));
        let fifo = fifo.clone();
        tokio::task::spawn_blocking(move || std::fs::write(&fifo, b"go\n")).await.unwrap().unwrap();
        seen
    };
    let ((sent, events), (running, sockets)) = tokio::join!(auto_turn(&daemon, 1, "listen"), look);
    assert_eq!(running, Vec::<u32>::new(), "no forwarder beside a foreground call");
    assert_eq!(sockets, Vec::<PathBuf>::new(), "no forward socket in a call dir");
    let output = events
        .iter()
        .find_map(|envelope| match &envelope.event {
            Event::ToolCallCompleted { output, .. } => Some(output.clone()),
            _ => None,
        })
        .unwrap();
    assert!(output.contains("socket-absent"), "{output}");
    let client = daemon.client().await.unwrap();
    let stored = daemon.events(&client, sent.conversation_id).await.unwrap();
    assert_eq!(forwards(&stored), [], "no record");
    drop(client);
    daemon.stop().await.unwrap();
}
