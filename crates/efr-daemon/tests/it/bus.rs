//! The filtered bus (phase E) through efrd at turn level, with the shared fake launcher
//! of `efr-test-daemon`, a fake `xdg-dbus-proxy` and two stand-in bus sockets that no
//! one answers. The probe says the filter is on, so a read form such as
//! `systemctl --failed` gets the view `Grant::BusFilter`. The fake proxy writes its
//! ready byte, prints the `--log` lines of the test (by default one refused
//! `StartUnit`), keeps its pid in the fake's dir, and ends when its sync pipe closes.
//!
//! No line runs `systemctl` on the test's machine: a job's line reads its input from a
//! FIFO that the test never writes, so its shell waits before it runs the program, and
//! the user stops the job; a foreground call finds a fake `systemctl` first on the
//! hidden shell's `PATH`, with the bus addresses on a path that does not exist. No test
//! reaches a real bus.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    ApprovalDecision, BusKind, BusRefusal, ConversationId, Event, EventEnvelope, Grant, JobId,
    JobsStop, JobsStopResult, Launch, Method, Mode, SandboxStatus,
};
use efr_sandbox::SandboxSpec;
use efr_test_daemon::{FakeLauncher, TTY, TestDaemon, command_id, events_until};
use efr_test_support::{TestDirs, Wait};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use crate::sandbox::{REVIEWER_ALLOWS, ScriptedModel, kinds, ready, run_turn};

/// The fake `xdg-dbus-proxy`: `@FAKE@` is its dir. The fake launcher runs it with the
/// sync pipe on fd 3 and its stdout on the stderr pipe that efrd reads.
const PROGRAM: &str = r#"#!/bin/sh
fake='@FAKE@'
if [ "$1" = --version ]; then echo 'xdg-dbus-proxy 0.1.9'; exit 0; fi
echo $$ >> "$fake/pids"
if [ -e "$fake/log" ]; then
    cat "$fake/log"
else
    echo 'C2: -> org.freedesktop.systemd1 call org.freedesktop.systemd1.Manager.StartUnit at /org/freedesktop/systemd1'
    echo '*DENIED* (ping)'
fi
trap '' PIPE
printf x >&3 || exit 1
if [ -e "$fake/crash" ]; then exit 7; fi
while printf . >&3 2> /dev/null; do sleep 0.05; done
exit 0
"#;

/// A fake `systemctl` for the hidden shell's `PATH`: it reaches no bus.
const SYSTEMCTL: &str = "#!/bin/sh\necho 'fake systemctl'\n";

/// The refused call of the fake's default log.
fn start_unit() -> BusRefusal {
    BusRefusal {
        bus: BusKind::System,
        name: "org.freedesktop.systemd1".to_owned(),
        member: "org.freedesktop.systemd1.Manager.StartUnit".to_owned(),
        path: "/org/freedesktop/systemd1".to_owned(),
        count: 1,
    }
}

/// A ready sandbox whose bus filter passed its self-test.
fn bus_ready() -> SandboxStatus {
    SandboxStatus { bus_proxy: true, ..ready() }
}

struct Setup {
    dirs: Arc<TestDirs>,
    launcher: FakeLauncher,
    /// The dir of the fake proxy and the fake `systemctl`.
    fake: PathBuf,
    system: PathBuf,
    session: PathBuf,
    _buses: [UnixListener; 2],
}

impl Setup {
    fn new() -> Setup {
        let dirs = Arc::new(TestDirs::new().unwrap());
        let launcher = FakeLauncher::write(&dirs.root().join("launcher")).unwrap();
        let fake = dirs.root().join("fake");
        std::fs::create_dir_all(fake.join("bin")).unwrap();
        let write = |path: PathBuf, text: &str| {
            std::fs::write(&path, text.replace("@FAKE@", &fake.to_string_lossy())).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write(fake.join("xdg-dbus-proxy"), PROGRAM);
        write(fake.join("bin/systemctl"), SYSTEMCTL);
        let buses = dirs.root().join("buses");
        std::fs::create_dir_all(&buses).unwrap();
        let (system, session) = (buses.join("sys.sock"), buses.join("sess.sock"));
        let listeners =
            [UnixListener::bind(&system).unwrap(), UnixListener::bind(&session).unwrap()];
        Setup { dirs, launcher, fake, system, session, _buses: listeners }
    }

    /// A daemon with the bus filter on and a model that calls `shell` with `input`;
    /// the reviewer allows every exit. `zsh` runs a real hidden shell, whose `PATH`
    /// finds the fake `systemctl` first.
    async fn daemon(&self, input: Value, zsh: bool) -> (TestDaemon, Arc<ScriptedModel>) {
        let model = ScriptedModel::reviewed(vec![("shell", input)], REVIEWER_ALLOWS);
        let job_env = BTreeMap::from([
            ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), self.dirs.home().display().to_string()),
        ]);
        let mut builder = TestDaemon::builder()
            .dirs(Arc::clone(&self.dirs))
            .sandbox_launcher(self.launcher.path())
            .probe_override(bus_ready())
            .bus_proxy(Some(self.system.clone()), Some(self.session.clone()))
            .bus_proxy_program(self.fake.join("xdg-dbus-proxy"))
            .job_env(job_env)
            .custom_provider(Arc::clone(&model) as Arc<dyn efr_provider::Provider>);
        if zsh {
            let nowhere = format!("unix:path={}", self.dirs.root().join("no-bus").display());
            let path = format!("{}:/usr/bin:/bin", self.fake.join("bin").display());
            let shell_env = BTreeMap::from([
                ("HOME".to_owned(), self.dirs.home().display().to_string()),
                ("PATH".to_owned(), path),
                ("LANG".to_owned(), "C.UTF-8".to_owned()),
                ("DBUS_SYSTEM_BUS_ADDRESS".to_owned(), nowhere.clone()),
                ("DBUS_SESSION_BUS_ADDRESS".to_owned(), nowhere),
            ]);
            builder = builder.local_pty().shell_env(shell_env);
        }
        (builder.start().await.unwrap(), model)
    }

    /// A FIFO below the test's root that no one writes: a job that reads it waits
    /// until it is stopped.
    fn fifo(&self) -> PathBuf {
        let path = self.dirs.root().join("never");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        path
    }

    /// The pid of each fake proxy that started, in order.
    fn pids(&self) -> Vec<u32> {
        std::fs::read_to_string(self.fake.join("pids"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.parse().ok())
            .collect()
    }

    /// The grants of each spec that the fake launcher kept.
    fn spec_grants(&self) -> Vec<Vec<Grant>> {
        std::fs::read_dir(self.launcher.dir())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("spec-"))
            .map(|entry| SandboxSpec::from_json(&std::fs::read(entry.path()).unwrap()).unwrap())
            .map(|spec| spec.grants)
            .collect()
    }
}

/// A background job that runs `line`.
fn job(line: &str) -> Value {
    json!({ "command": line, "background": true, "max_minutes": 1 })
}

/// True while the process `pid` runs (a zombie has ended).
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| !rest.starts_with('Z')))
        .unwrap_or(false)
}

/// The launch of the job that `events` started.
fn job_launch(events: &[EventEnvelope]) -> Option<Launch> {
    events.iter().find_map(|envelope| match &envelope.event {
        Event::JobStarted { job, .. } => Some(job.launch.clone()),
        _ => None,
    })
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

/// The `job_ended` and the text of the `job_notice` of the first job of `conversation`.
async fn job_end(daemon: &TestDaemon, conversation: ConversationId) -> (Event, String) {
    let events =
        until(daemon, conversation, |event| matches!(event, Event::JobNotice { .. })).await;
    let ended = events
        .iter()
        .find(|envelope| matches!(envelope.event, Event::JobEnded { .. }))
        .map(|envelope| envelope.event.clone())
        .unwrap();
    let Some(Event::JobNotice { text, .. }) = events.last().map(|envelope| &envelope.event) else {
        unreachable!()
    };
    (ended, text.clone())
}

/// The user stops the first job of `conversation`.
async fn stop_job(daemon: &TestDaemon, conversation: ConversationId) {
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let stop = Method::JobsStop(JobsStop {
        command_id: command_id(60),
        conversation_id: Some(conversation),
        job_id: JobId::nth(1),
    });
    let _: JobsStopResult = client.call(stop).await.unwrap();
}

#[tokio::test]
async fn bus_refusals_reach_the_summary_and_the_note() {
    let setup = Setup::new();
    let fifo = setup.fifo();
    let line = format!("systemctl --failed --no-pager < '{}'", fifo.display());
    let (daemon, model) = setup.daemon(job(&line), false).await;

    let (sent, events) =
        run_turn(&daemon, "show the failed units", Mode::Auto, ApprovalDecision::Deny).await;

    assert_eq!(model.reviews(), 0, "a read form runs with no review: {:?}", kinds(&events));
    let filter = Grant::BusFilter { bus: BusKind::System };
    assert_eq!(job_launch(&events), Some(Launch::Contained { grants: vec![filter.clone()] }));
    assert_eq!(setup.spec_grants(), [vec![filter]]);
    stop_job(&daemon, sent.conversation_id).await;
    let (ended, text) = job_end(&daemon, sent.conversation_id).await;
    let Event::JobEnded { bus_refused, bus_refused_more, .. } = ended else { unreachable!() };
    assert_eq!(bus_refused, [start_unit()]);
    assert_eq!(bus_refused_more, 0);
    assert!(
        text.contains(
            "[efr: the bus filter refused a call: bus=system name=org.freedesktop.systemd1 \
             member=org.freedesktop.systemd1.Manager.StartUnit path=/org/freedesktop/systemd1. \
             For the whole bus for one command, call shell again with needs.bus.]"
        ),
        "{text}"
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn bus_proxy_of_a_job_lives_with_the_job() {
    let setup = Setup::new();
    let fifo = setup.fifo();
    std::fs::write(setup.fake.join("log"), b"").unwrap();
    let line = format!("systemctl --failed --no-pager < '{}'", fifo.display());
    let (daemon, _) = setup.daemon(job(&line), false).await;

    let (sent, events) =
        run_turn(&daemon, "watch the failed units", Mode::Auto, ApprovalDecision::Deny).await;

    let filter = Grant::BusFilter { bus: BusKind::System };
    assert_eq!(job_launch(&events), Some(Launch::Contained { grants: vec![filter] }));
    let pids = setup.pids();
    let [pid] = pids.as_slice() else { panic!("one filter for the job: {pids:?}") };
    assert!(alive(*pid), "the filter runs while the job runs, after its turn ended");
    stop_job(&daemon, sent.conversation_id).await;
    let (ended, text) = job_end(&daemon, sent.conversation_id).await;
    let Event::JobEnded { bus_refused, .. } = ended else { unreachable!() };
    assert!(bus_refused.is_empty(), "{bus_refused:?}");
    assert!(!text.contains("bus filter"), "{text}");
    Wait::new("the filter's end with its job").until(|| !alive(*pid)).await.unwrap();
    assert_eq!(setup.pids().len(), 1, "no other filter started");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn bus_proxy_crash_gives_a_note() {
    let setup = Setup::new();
    let fifo = setup.fifo();
    std::fs::write(setup.fake.join("crash"), b"").unwrap();
    let line = format!("systemctl --failed --no-pager < '{}'", fifo.display());
    let (daemon, _) = setup.daemon(job(&line), false).await;

    let (sent, _) =
        run_turn(&daemon, "watch the failed units", Mode::Auto, ApprovalDecision::Deny).await;
    let pids = setup.pids();
    let [pid] = pids.as_slice() else { panic!("{pids:?}") };
    Wait::new("the crash of the filter").until(|| !alive(*pid)).await.unwrap();
    stop_job(&daemon, sent.conversation_id).await;

    let (ended, text) = job_end(&daemon, sent.conversation_id).await;
    let Event::JobEnded { bus_refused, .. } = ended else { unreachable!() };
    assert_eq!(bus_refused, [start_unit()], "what it refused before it stopped");
    assert!(
        text.contains(
            "[efr: the bus filter stopped during the command, so its bus calls after that \
             failed.]"
        ),
        "{text}"
    );
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_real_bus_grant_wins_over_bus_filter() {
    if !crate::support::zsh_enabled("shell_real_bus_grant_wins_over_bus_filter") {
        return;
    }
    let setup = Setup::new();
    let input = json!({ "command": "systemctl --failed --no-pager", "needs": { "bus": "system" } });
    let (daemon, model) = setup.daemon(input, true).await;

    let (_, events) =
        run_turn(&daemon, "start my unit if it failed", Mode::Auto, ApprovalDecision::Deny).await;

    assert_eq!(model.reviews(), 1, "the reviewer judged the whole bus: {:?}", kinds(&events));
    let completed = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, sandbox, .. } => Some((output.clone(), sandbox.clone())),
        _ => None,
    });
    let (output, summary) = completed.unwrap();
    assert!(output.contains("fake systemctl"), "{output}");
    assert_eq!(setup.spec_grants(), [vec![Grant::Bus { bus: BusKind::System }]]);
    assert!(setup.pids().is_empty(), "the whole bus starts no filter");
    assert!(summary.unwrap().bus_refused.is_empty());
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn shell_bus_refusals_reach_the_tool_result() {
    if !crate::support::zsh_enabled("shell_bus_refusals_reach_the_tool_result") {
        return;
    }
    let setup = Setup::new();
    let (daemon, model) =
        setup.daemon(json!({ "command": "systemctl --failed --no-pager" }), true).await;

    let (_, events) =
        run_turn(&daemon, "show the failed units", Mode::Auto, ApprovalDecision::Deny).await;

    assert_eq!(model.reviews(), 0, "{:?}", kinds(&events));
    let completed = events.iter().find_map(|envelope| match &envelope.event {
        Event::ToolCallCompleted { output, sandbox, .. } => Some((output.clone(), sandbox.clone())),
        _ => None,
    });
    let (output, summary) = completed.unwrap();
    assert!(output.contains("fake systemctl"), "{output}");
    let summary = summary.unwrap();
    assert_eq!(summary.bus_refused, [start_unit()]);
    assert_eq!(summary.bus_refused_more, 0);
    assert!(
        output.contains(
            "[efr: the bus filter refused a call: bus=system name=org.freedesktop.systemd1"
        ),
        "{output}"
    );
    assert_eq!(setup.spec_grants(), [vec![Grant::BusFilter { bus: BusKind::System }]]);
    let pids = setup.pids();
    let [pid] = pids.as_slice() else { panic!("{pids:?}") };
    Wait::new("the filter's end with its call").until(|| !alive(*pid)).await.unwrap();
    daemon.stop().await.unwrap();
}
